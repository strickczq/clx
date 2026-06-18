use clap::Parser;

/// Command-line invocation, parsed by clap.
///
/// `--help`/`--version` are handled by clap (it prints and exits inside
/// `parse`). The remaining fields are dispatched on in `main`.
#[derive(Parser, Debug)]
#[command(
    name = "clx",
    version,
    about = "clx — CLaude eXecutor",
    long_about = "Interactive profile launcher for Claude Code.\n\n\
        Run without a profile for the interactive fuzzy-picker, pass a profile \
        name to launch it directly, or forward extra args to claude after `--`."
)]
pub struct Cli {
    /// List available profiles and exit
    #[arg(short, long)]
    pub list: bool,

    /// Hidden: print a profile's preview text (invoked by fzf)
    #[arg(long, hide = true, value_name = "NAME")]
    pub preview: Option<String>,

    /// Profile to launch (omit for the interactive picker)
    #[arg(value_name = "PROFILE")]
    pub profile: Option<String>,

    /// Extra args forwarded verbatim to claude (everything after `--`)
    #[arg(last = true, allow_hyphen_values = true, value_name = "ARGS")]
    pub passthrough: Vec<String>,
}
