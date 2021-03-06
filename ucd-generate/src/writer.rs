#![allow(dead_code)]

// BREADCRUMBS:
//
// Move all output decisions into the Writer. Basic idea is that we specify
// common data structures and do the FST conversion inside the writer.
//
// Remove --raw-fst and make --rust-fst always emit FSTs to separate files
// They use much less space on disk, i.e., by avoiding encoding bytes in a
// byte literal.
//
// Problem: --rust-slice writes directly to a single file and it make sense
// to emit it to stdout. But --rust-fst wants to write at least two files:
// one for Rust source and another for the FST. An obvious solution is to leave
// --rust-slice untouched but require a directory argument for --rust-fst.
// It's a bit incongruous, which is unfortunate, but probably makes the most
// sense. However, if --rust-slice is always the default (which seems
// reasonable), then we could just remove --rust-slice altogether and that,
// I think, removes some of the incongruity.

use std::ascii;
use std::char;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str;

use byteorder::{ByteOrder, BigEndian as BE};
use fst::{Map, MapBuilder, Set, SetBuilder};
use fst::raw::Fst;
use ucd_parse::Codepoint;

use error::Result;
use util;

#[derive(Clone, Debug)]
pub struct WriterBuilder(WriterOptions);

#[derive(Clone, Debug)]
struct WriterOptions {
    name: String,
    columns: u64,
    char_literals: bool,
    fst_dir: Option<PathBuf>,
}

impl WriterBuilder {
    /// Create a new builder Unicode writers.
    ///
    /// The name given corresponds to the Rust module name to use when
    /// applicable.
    pub fn new(name: &str) -> WriterBuilder {
        WriterBuilder(WriterOptions {
            name: name.to_string(),
            columns: 79,
            char_literals: false,
            fst_dir: None,
        })
    }

    /// Create a new Unicode writer from this builder's configuration.
    pub fn from_writer<W: io::Write + 'static>(&self, wtr: W) -> Writer {
        Writer {
            wtr: LineWriter::new(Box::new(wtr)),
            wrote_header: false,
            opts: self.0.clone(),
        }
    }

    /// Create a new Unicode writer that writes to stdout.
    pub fn from_stdout(&self) -> Writer {
        self.from_writer(io::stdout())
    }

    /// Create a new Unicode writer that writes FSTs to a directory.
    pub fn from_fst_dir<P: AsRef<Path>>(&self, fst_dir: P) -> Result<Writer> {
        let mut opts = self.0.clone();
        opts.fst_dir = Some(fst_dir.as_ref().to_path_buf());
        let mut fpath = fst_dir.as_ref().join(rust_module_name(&opts.name));
        fpath.set_extension("rs");
        Ok(Writer {
            wtr: LineWriter::new(Box::new(File::create(fpath)?)),
            wrote_header: false,
            opts: opts,
        })
    }

    /// Set the column limit to use when writing Rust source code.
    ///
    /// Note that this is adhered to on a "best effort" basis.
    pub fn columns(&mut self, columns: u64) -> &mut WriterBuilder {
        self.0.columns = columns;
        self
    }

    /// When printing Rust source code, emit `char` literals instead of `u32`
    /// literals. Any codepoints that aren't Unicode scalar values (i.e.,
    /// surrogate codepoints) are silently dropped when writing.
    pub fn char_literals(&mut self, yes: bool) -> &mut WriterBuilder {
        self.0.char_literals = yes;
        self
    }

    /// Emit codepoints as a finite state transducer.
    ///
    /// The directory given is where both the Rust source file and the FST
    /// files are written. The Rust source file includes the FSTs using the
    /// `include_bytes!` macro.
    pub fn fst_dir<P: AsRef<Path>>(
        &mut self,
        fst_dir: Option<P>,
    ) -> &mut WriterBuilder {
        self.0.fst_dir = fst_dir.map(|p| p.as_ref().to_path_buf());
        self
    }
}

/// A writer of various kinds of Unicode data.
///
/// A writer takes as input various forms of Unicode data and writes that data
/// in a number of different output formats.
pub struct Writer {
    wtr: LineWriter<Box<io::Write + 'static>>,
    wrote_header: bool,
    opts: WriterOptions,
}

impl Writer {
    /// Write a sorted sequence of codepoints.
    ///
    /// Note that the specific representation of ranges may differ with the
    /// output format. For example, if the output format is a slice, then a
    /// straight-forward slice of sorted codepoint ranges is emitted. But if
    /// the output format is an FST or similar, then all codepoints are
    /// explicitly represented.
    pub fn ranges(
        &mut self,
        name: &str,
        codepoints: &BTreeSet<u32>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        let name = rust_const_name(name);
        if self.opts.fst_dir.is_some() {
            let mut builder = SetBuilder::memory();
            builder.extend_iter(codepoints.iter().cloned().map(u32_key))?;
            let set = Set::from_bytes(builder.into_inner()?)?;
            self.fst(&name, set.as_fst(), false)?;
        } else {
            let ranges = util::to_ranges(codepoints.iter().cloned());
            self.ranges_slice(&name, &ranges)?;
        }
        self.wtr.flush()?;
        Ok(())
    }

    fn ranges_slice(
        &mut self,
        name: &str,
        table: &[(u32, u32)],
    ) -> Result<()> {
        let ty = self.rust_codepoint_type();
        writeln!(
            self.wtr,
            "pub const {}: &'static [({}, {})] = &[",
            name, ty, ty)?;
        for &(start, end) in table {
            let range = (self.rust_codepoint(start), self.rust_codepoint(end));
            if let (Some(start), Some(end)) = range {
                self.wtr.write_str(&format!("({}, {}), ", start, end))?;
            }
        }
        writeln!(self.wtr, "];")?;
        Ok(())
    }

    /// Write a map that associates codepoint ranges to a single value in an
    /// enumeration. This usually emits two items: a map from codepoint range
    /// to index and a map from index to one of the enum variants.
    ///
    /// The given map should be a map from the enum variant value to the set
    /// of codepoints that have that value.
    pub fn ranges_to_enum(
        &mut self,
        name: &str,
        enum_map: &BTreeMap<String, BTreeSet<u32>>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        writeln!(
            self.wtr,
            "pub const {}_ENUM: &'static [&'static str] = &[",
            rust_const_name(name))?;
        for variant in enum_map.keys() {
            self.wtr.write_str(&format!("{:?}, ", variant))?;
        }
        writeln!(self.wtr, "];")?;

        let mut map = BTreeMap::new();
        for (i, (_, ref set)) in enum_map.iter().enumerate() {
            map.extend(set.iter().cloned().map(|cp| (cp, i as u64)));
        }
        self.ranges_to_unsigned_integer(name, &map)?;
        self.wtr.flush()?;
        Ok(())
    }

    /// Write a map that associates ranges of codepoints with an arbitrary
    /// integer.
    ///
    /// The smallest numeric type is used when applicable.
    pub fn ranges_to_unsigned_integer(
        &mut self,
        name: &str,
        map: &BTreeMap<u32, u64>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        let name = rust_const_name(name);
        if self.opts.fst_dir.is_some() {
            let mut builder = MapBuilder::memory();
            for (&k, &v) in map {
                builder.insert(u32_key(k), v)?;
            }
            let map = Map::from_bytes(builder.into_inner()?)?;
            self.fst(&name, map.as_fst(), true)?;
        } else {
            let ranges = util::to_range_values(
                map.iter().map(|(&k, &v)| (k, v)));
            self.ranges_to_unsigned_integer_slice(&name, &ranges)?;
        }
        self.wtr.flush()?;
        Ok(())
    }

    fn ranges_to_unsigned_integer_slice(
        &mut self,
        name: &str,
        table: &[(u32, u32, u64)],
    ) -> Result<()> {
        let cp_ty = self.rust_codepoint_type();
        let num_ty = match table.iter().map(|&(_, _, n)| n).max() {
            None => "u8",
            Some(max_num) => smallest_unsigned_type(max_num),
        };

        writeln!(
            self.wtr,
            "pub const {}: &'static [({}, {}, {})] = &[",
            name, cp_ty, cp_ty, num_ty)?;
        for &(start, end, num) in table {
            let range = (self.rust_codepoint(start), self.rust_codepoint(end));
            if let (Some(start), Some(end)) = range {
                let src = format!("({}, {}, {}), ", start, end, num);
                self.wtr.write_str(&src)?;
            }
        }
        writeln!(self.wtr, "];")?;
        Ok(())
    }

    /// Write a map that associates codepoints to strings.
    ///
    /// When the output format is an FST, then the FST map emitted is from
    /// codepoint to u64, where the string is encoded into the u64. The least
    /// significant byte of the u64 corresponds to the first byte in the
    /// string. The end of a string is delimited by the zero byte. If a string
    /// is more than 8 bytes or contains a `NUL` byte, then an error is
    /// returned.
    pub fn codepoint_to_string(
        &mut self,
        name: &str,
        map: &BTreeMap<u32, String>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        let name = rust_const_name(name);
        if self.opts.fst_dir.is_some() {
            let mut builder = MapBuilder::memory();
            for (&k, v) in map {
                let v = pack_str(v)?;
                builder.insert(u32_key(k), v)?;
            }
            let map = Map::from_bytes(builder.into_inner()?)?;
            self.fst(&name, map.as_fst(), true)?;
        } else {
            let table: Vec<(u32, &str)> =
                map.iter().map(|(&k, v)| (k, &**v)).collect();
            self.codepoint_to_string_slice(&name, &table)?;
        }
        self.wtr.flush()?;
        Ok(())
    }

    fn codepoint_to_string_slice(
        &mut self,
        name: &str,
        table: &[(u32, &str)],
    ) -> Result<()> {
        let ty = self.rust_codepoint_type();
        writeln!(
            self.wtr,
            "pub const {}: &'static [({}, &'static str)] = &[",
            name, ty)?;
        for &(cp, ref s) in table {
            if let Some(cp) = self.rust_codepoint(cp) {
                self.wtr.write_str(&format!("({}, {:?}), ", cp, s))?;
            }
        }
        writeln!(self.wtr, "];")?;
        Ok(())
    }

    /// Write a map that associates strings to codepoints.
    pub fn string_to_codepoint(
        &mut self,
        name: &str,
        map: &BTreeMap<String, u32>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        let name = rust_const_name(name);
        if self.opts.fst_dir.is_some() {
            let mut builder = MapBuilder::memory();
            for (k, &v) in map {
                builder.insert(k.as_bytes(), v as u64)?;
            }
            let map = Map::from_bytes(builder.into_inner()?)?;
            self.fst(&name, map.as_fst(), true)?;
        } else {
            let table: Vec<(&str, u32)> =
                map.iter().map(|(k, &v)| (&**k, v)).collect();
            self.string_to_codepoint_slice(&name, &table)?;
        }
        self.wtr.flush()?;
        Ok(())
    }

    fn string_to_codepoint_slice(
        &mut self,
        name: &str,
        table: &[(&str, u32)],
    ) -> Result<()> {
        let ty = self.rust_codepoint_type();
        writeln!(
            self.wtr,
            "pub const {}: &'static [(&'static str, {})] = &[",
            name, ty)?;
        for &(ref s, cp) in table {
            if let Some(cp) = self.rust_codepoint(cp) {
                self.wtr.write_str(&format!("({:?}, {}), ", s, cp))?;
            }
        }
        writeln!(self.wtr, "];")?;
        Ok(())
    }

    /// Write a map that associates strings to `u64` values.
    pub fn string_to_u64(
        &mut self,
        name: &str,
        map: &BTreeMap<String, u64>,
    ) -> Result<()> {
        self.header()?;
        self.separator()?;

        let name = rust_const_name(name);
        if self.opts.fst_dir.is_some() {
            let mut builder = MapBuilder::memory();
            for (k, &v) in map {
                builder.insert(k.as_bytes(), v)?;
            }
            let map = Map::from_bytes(builder.into_inner()?)?;
            self.fst(&name, map.as_fst(), true)?;
        } else {
            let table: Vec<(&str, u64)> =
                map.iter().map(|(k, &v)| (&**k, v)).collect();
            self.string_to_u64_slice(&name, &table)?;
        }
        self.wtr.flush()?;
        Ok(())
    }

    fn string_to_u64_slice(
        &mut self,
        name: &str,
        table: &[(&str, u64)],
    ) -> Result<()> {
        writeln!(
            self.wtr,
            "pub const {}: &'static [(&'static str, u64)] = &[",
            name)?;
        for &(ref s, n) in table {
            self.wtr.write_str(&format!("({:?}, {}), ", s, n))?;
        }
        writeln!(self.wtr, "];")?;
        Ok(())
    }

    fn fst(
        &mut self,
        const_name: &str,
        fst: &Fst,
        map: bool,
    ) -> Result<()> {
        let fst_dir = self.opts.fst_dir.as_ref().unwrap();
        let fst_file_name = format!("{}.fst", rust_module_name(const_name));
        let fst_file_path = fst_dir.join(&fst_file_name);
        File::create(fst_file_path)?.write_all(&fst.to_vec())?;

        let ty = if map { "Map" } else { "Set" };
        writeln!(self.wtr, "lazy_static! {{")?;
        writeln!(
            self.wtr,
            "  pub static ref {}: ::fst::{} = ", const_name, ty)?;
        writeln!(
            self.wtr,
            "    ::fst::{}::from(::fst::raw::Fst::from_static_slice(", ty)?;
        writeln!(
            self.wtr,
            "      include_bytes!({:?})).unwrap());", fst_file_name)?;
        writeln!(self.wtr, "}}")?;
        Ok(())
    }

    fn header(&mut self) -> Result<()> {
        if self.wrote_header {
            return Ok(());
        }
        let mut argv = vec![];
        argv.push(
            env::current_exe()?
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned());
        for arg in env::args_os().skip(1) {
            let x = arg.to_string_lossy();
            argv.push(x.into_owned());
        }
        writeln!(self.wtr, "#![allow(dead_code)]")?;
        writeln!(self.wtr, "")?;
        writeln!(self.wtr, "// DO NOT EDIT THIS FILE. \
                               IT WAS AUTOMATICALLY GENERATED BY:")?;
        writeln!(self.wtr, "//")?;
        writeln!(self.wtr, "//  {}", argv.join(" "))?;
        writeln!(self.wtr, "//")?;
        writeln!(self.wtr, "// ucd-generate is available on crates.io.")?;
        self.wrote_header = true;
        Ok(())
    }

    fn separator(&mut self) -> Result<()> {
        write!(self.wtr, "\n")?;
        Ok(())
    }

    /// Return valid Rust source code that represents the given codepoint.
    ///
    /// The source code returned is either a u32 literal or a char literal,
    /// depending on the configuration. If the configuration demands a char
    /// literal and the given codepoint is a surrogate, then return None.
    fn rust_codepoint(&self, cp: u32) -> Option<String> {
        if self.opts.char_literals {
            char::from_u32(cp).map(|c| format!("{:?}", c))
        } else {
            Some(cp.to_string())
        }
    }

    /// Return valid Rust source code indicating the type of the codepoint
    /// that we emit based on this writer's configuration.
    fn rust_codepoint_type(&self) -> &'static str {
        if self.opts.char_literals {
            "char"
        } else {
            "u32"
        }
    }
}

#[derive(Debug)]
struct LineWriter<W> {
    wtr: W,
    line: String,
    columns: usize,
    indent: String,
}

impl<W: io::Write> LineWriter<W> {
    fn new(wtr: W) -> LineWriter<W> {
        LineWriter {
            wtr: wtr,
            line: String::new(),
            columns: 79,
            indent: "  ".to_string(),
        }
    }

    fn write_str(&mut self, s: &str) -> io::Result<()> {
        if self.line.len() + s.len() > self.columns {
            self.flush_line()?;
        }
        if self.line.is_empty() {
            self.line.push_str(&self.indent);
        }
        self.line.push_str(s);
        Ok(())
    }

    fn indent(&mut self, s: &str) {
        self.indent = s.to_string();
    }

    fn flush_line(&mut self) -> io::Result<()> {
        if self.line.is_empty() {
            return Ok(());
        }
        self.wtr.write_all(self.line.trim_right().as_bytes())?;
        self.wtr.write_all(b"\n")?;
        self.line.clear();
        Ok(())
    }
}

impl<W: io::Write> io::Write for LineWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.flush_line()?;
        self.wtr.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_line()?;
        self.wtr.flush()
    }
}

/// Return the given byte as its escaped string form.
fn escape_input(b: u8) -> String {
    String::from_utf8(ascii::escape_default(b).collect::<Vec<_>>()).unwrap()
}

/// Heuristically produce an appropriate constant Rust name.
fn rust_const_name(s: &str) -> String {
    use std::ascii::AsciiExt;

    // Property names/values seem pretty uniform, particularly the
    // "canonical" variants we use to produce variable names. So we
    // don't need to do much.
    let mut s = s.to_string();
    s.make_ascii_uppercase();
    s
}

/// Heuristically produce an appropriate module Rust name.
fn rust_module_name(s: &str) -> String {
    use std::ascii::AsciiExt;

    // Property names/values seem pretty uniform, particularly the
    // "canonical" variants we use to produce variable names. So we
    // don't need to do much.
    let mut s = s.to_string();
    s.make_ascii_lowercase();
    s
}

/// Return the given codepoint encoded in big-endian.
pub fn codepoint_key(cp: Codepoint) -> [u8; 4] {
    u32_key(cp.value())
}

/// Return the given u32 encoded in big-endian.
pub fn u32_key(cp: u32) -> [u8; 4] {
    let mut key = [0; 4];
    BE::write_u32(&mut key, cp);
    key
}

/// Convert the given string into a u64, where the least significant byte of
/// the u64 is the first byte of the string.
///
/// If the string contains any `NUL` bytes or has more than 8 bytes, then an
/// error is returned.
fn pack_str(s: &str) -> Result<u64> {
    if s.len() > 8 {
        return err!("cannot encode string {:?} (too long)", s);
    }
    if s.contains('\x00') {
        return err!("cannot encode string {:?} (contains NUL byte)", s);
    }
    let mut value = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        assert!(i <= 7);
        value |= (b as u64) << (8 * i as u64);
    }
    Ok(value)
}

/// Return a string representing the smallest unsigned integer type for the
/// given value.
fn smallest_unsigned_type(n: u64) -> &'static str {
    if n <= ::std::u8::MAX as u64 {
        "u8"
    } else if n <= ::std::u16::MAX as u64 {
        "u16"
    } else if n <= ::std::u32::MAX as u64 {
        "u32"
    } else {
        "u64"
    }
}

#[cfg(test)]
mod tests {
    use super::pack_str;

    fn unpack_str(mut encoded: u64) -> String {
        let mut value = String::new();
        while encoded != 0 {
            value.push((encoded & 0xFF) as u8 as char);
            encoded = encoded >> 8;
        }
        value
    }

    #[test]
    fn packed() {
        assert_eq!("G", unpack_str(pack_str("G").unwrap()));
        assert_eq!("GG", unpack_str(pack_str("GG").unwrap()));
        assert_eq!("YEO", unpack_str(pack_str("YEO").unwrap()));
        assert_eq!("ABCDEFGH", unpack_str(pack_str("ABCDEFGH").unwrap()));
        assert_eq!("", unpack_str(pack_str("").unwrap()));

        assert!(pack_str("ABCDEFGHI").is_err());
        assert!(pack_str("AB\x00CD").is_err());
    }
}
