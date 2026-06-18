mod ansi;
mod cli;
mod config;
mod error;
mod launcher;
mod profile;
mod ui;

use std::io::IsTerminal;

use clap::Parser;

use crate::ansi::{CYAN, DIM, RED, paint_if};
use crate::cli::Cli;
use crate::error::Error;

fn main() {
    if let Err(e) = run() {
        eprintln!("{} {e}", paint_if(RED, "error:", stderr_tty()));
        std::process::exit(1);
    }
}

/// Whether stdout is a terminal — gate colour so piped output stays plain.
fn stdout_tty() -> bool {
    std::io::stdout().is_terminal()
}

/// Whether stderr is a terminal — gate colour for diagnostics.
fn stderr_tty() -> bool {
    std::io::stderr().is_terminal()
}

fn run() -> Result<(), Error> {
    // Parse first: clap handles `--help`/`--version` (printing and exiting)
    // before we touch the config, so they work without a config file.
    let cli = Cli::parse();
    let cfg = config::load_config()?;

    // Hidden preview subcommand — print and exit (used by fzf).
    if let Some(name) = cli.preview {
        println!("{}", ui::preview_text(&cfg, &name));
        return Ok(());
    }

    // `--list` — print available profiles (name<TAB>description) and exit.
    if cli.list {
        print_profiles(&mut std::io::stdout(), &cfg, stdout_tty());
        return Ok(());
    }

    // Resolve profile name — interactive or from CLI.
    let profile_name = match cli.profile {
        Some(name) => name,
        None => match ui::select_profile(&cfg)? {
            ui::Selection::Picked(name) => name,
            ui::Selection::Cancelled => {
                eprintln!(
                    "{}",
                    paint_if(DIM, "no profile selected, exiting", stderr_tty())
                );
                return Ok(());
            }
            // fzf unavailable: list the profiles and point at direct mode.
            ui::Selection::FzfMissing => {
                eprintln!(
                    "{}",
                    paint_if(DIM, "fzf not found. available profiles:", stderr_tty())
                );
                print_profiles(&mut std::io::stderr(), &cfg, stderr_tty());
                eprintln!("{}", paint_if(DIM, "run: clx <profile>", stderr_tty()));
                return Ok(());
            }
        },
    };

    // Resolve inheritance chain.
    let resolved = profile::resolve_profile(&cfg, &profile_name)?;

    // Replace current process with claude — execve never returns on success.
    launcher::launch(&resolved, &cli.passthrough)?;
    unreachable!("execve should have replaced the process");
}

/// Print each profile as `name<TAB>description` to `out`, colouring when `color`
/// is set. Shared by `--list` (stdout) and the fzf-missing fallback (stderr).
fn print_profiles(out: &mut impl std::io::Write, cfg: &config::Config, color: bool) {
    for p in &cfg.profiles {
        let name = paint_if(CYAN, &p.name, color);
        let _ = match &p.description {
            Some(d) => writeln!(out, "{name}\t{}", paint_if(DIM, d, color)),
            None => writeln!(out, "{name}"),
        };
    }
}
