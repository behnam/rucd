use clap::{App, AppSettings, Arg, SubCommand};

const TEMPLATE: &'static str = "\
{bin} {version}
{author}
{about}

USAGE:
    {usage}

SUBCOMMANDS:
{subcommands}

OPTIONS:
{unified}";

const TEMPLATE_SUB: &'static str = "\
{before-help}
USAGE:
    {usage}

ARGS:
{positionals}

OPTIONS:
{unified}";

const ABOUT: &'static str = "
ucd-generate is a tool that generates Rust source files containing various
Unicode tables.

Unicode tables are typically represented by finite state transducers, which
permits fast searching while simultaneously compressing the table.

Project home page: https://github.com/BurntSushi/rucd";

const ABOUT_GENERAL_CATEGORY: &'static str = "\
general-category produces one table of Unicode codepoint ranges for each
possible General_Category value.
";

const ABOUT_JAMO_SHORT_NAME: &'static str = "\
jamo-short-name parses the UCD's Jamo.txt file and emits its contents as a
slice table. The slice consists of a sorted sequences of pairs, where each
pair corresponds to the codepoint and the Jamo_Short_Name property value.

When emitted as an FST table, the FST corresponds to a map from a Unicode
codepoint (encoded as a big-endian u32) to a u64, where the u64 contains the
Jamo_Short_Name property value. The value is encoded in the least significant
bytes (up to 3).

Since the table is so small, the slice table is faster to search.
";

const ABOUT_NAMES: &'static str = "\
names emits a table of all character names in the UCD, including aliases and
names that are algorithmically generated such as Hangul syllables and
ideographs.
";

const ABOUT_TEST_UNICODE_DATA: &'static str = "\
test-unicode-data parses the UCD's UnicodeData.txt file and emits its contents
on stdout. The purpose of this command is to diff the output with the input and
confirm that they are identical. This is a sanity test on the UnicodeData.txt
parser.
";

/// Build a clap application.
pub fn app() -> App<'static, 'static> {
    // Various common flags and arguments.
    let flag_name = |default| {
        Arg::with_name("name")
            .help("Set the name of the table in the emitted code.")
            .long("name")
            .takes_value(true)
            .default_value(default)
    };
    let flag_rust_fst = Arg::with_name("rust-fst")
        .long("rust-fst")
        .help("Emit the table as a FST in Rust source codeto stdout.");
    let flag_raw_fst = Arg::with_name("raw-fst")
        .long("raw-fst")
        .help("Emit the table as a raw FST to stdout.\n\
               Pro-tip: Run `cargo install fst-bin` to install the `fst` \
               command line tool, which can be used to search the FST.");
    let flag_rust_slice = Arg::with_name("rust-slice")
        .long("rust-slice")
        .help("Emit the table as a static slice that can be binary searched.");
    let ucd_dir = Arg::with_name("ucd-dir")
        .required(true)
        .help("Directory containing the Unicode character database files.");

    // Subcommands.
    let cmd_general_category = SubCommand::with_name("general-category")
        .author(crate_authors!())
        .version(crate_version!())
        .template(TEMPLATE_SUB)
        .about("Create the General_Category property tables.")
        .before_help(ABOUT_GENERAL_CATEGORY)
        .arg(ucd_dir.clone())
        .arg(flag_rust_slice.clone())
        .arg(flag_rust_fst.clone())
        .arg(flag_name("GENERAL_CATEGORY"))
        .arg(Arg::with_name("chars")
            .long("chars")
            .help("Write codepoints as character literals. If a codepoint \
                   cannot be written as a character literal, then it is \
                   silently dropped."))
        .arg(Arg::with_name("no-unassigned")
            .long("no-unassigned")
            .help("Don't emit the Unassigned general category."));
    let cmd_jamo_short_name = SubCommand::with_name("jamo-short-name")
        .author(crate_authors!())
        .version(crate_version!())
        .template(TEMPLATE_SUB)
        .about("Create the Jamo_Short_Name property table.")
        .before_help(ABOUT_JAMO_SHORT_NAME)
        .arg(ucd_dir.clone())
        .arg(flag_rust_slice.clone())
        .arg(flag_rust_fst.clone())
        .arg(flag_raw_fst.clone())
        .arg(flag_name("JAMO_SHORT_NAME"));
    let cmd_names = SubCommand::with_name("names")
        .author(crate_authors!())
        .version(crate_version!())
        .template(TEMPLATE_SUB)
        .about("Create a mapping from character name to codepoint.")
        .before_help(ABOUT_NAMES)
        .arg(ucd_dir.clone())
        .arg(flag_rust_slice.clone())
        .arg(flag_rust_fst.clone())
        .arg(flag_raw_fst.clone())
        .arg(flag_name("NAMES"))
        .arg(Arg::with_name("no-aliases")
            .long("no-aliases")
            .help("Ignore all character name aliases. When used, every name \
                   maps to exactly one codepoint."))
        .arg(Arg::with_name("no-ideograph")
            .long("no-ideograph")
            .help("Do not include algorithmically generated ideograph names."))
        .arg(Arg::with_name("no-hangul")
            .long("no-hangul")
            .help("Do not include algorithmically generated Hangul syllable \
                   names."))
        .arg(Arg::with_name("tagged")
             .long("tagged")
             .help("Tag each codepoint with how the name was derived. \
                    The lower 32 bits corresponds to the codepoint. Bit 33 \
                    indicates the name was explicitly provided in \
                    UnicodeData.txt. Bit 34 indicates the name is from \
                    NameAliases.txt. \
                    Bit 35 indicates the name is a Hangul syllable. Bit 36 \
                    indicates the name is an ideograph."))
        .arg(Arg::with_name("normalize")
            .long("normalize")
            .help("Normalize all character names according to UAX44-LM2."));

    let cmd_test_unicode_data = SubCommand::with_name("test-unicode-data")
        .author(crate_authors!())
        .version(crate_version!())
        .template(TEMPLATE_SUB)
        .about("Test the UnicodeData.txt parser.")
        .before_help(ABOUT_TEST_UNICODE_DATA)
        .arg(ucd_dir.clone());

    // The actual App.
    App::new("ucd-generate")
        .author(crate_authors!())
        .version(crate_version!())
        .about(ABOUT)
        .template(TEMPLATE)
        .max_term_width(100)
        .setting(AppSettings::UnifiedHelpMessage)
        .subcommand(cmd_general_category)
        .subcommand(cmd_jamo_short_name)
        .subcommand(cmd_names)
        .subcommand(cmd_test_unicode_data)
}
