use std::io::Write;
use std::process::{Command, Stdio};

use crate::ansi::{BOLD, CYAN, DIM, GREEN, RED, YELLOW, paint};
use crate::config::{Config, find_profile};
use crate::error::Error;
use crate::launcher::compute_preview_env;
use crate::profile::resolve_profile;

// MARK: - Preview builder

/// Build the ANSI-coloured preview text for a profile.
/// fzf renders this in the preview panel via `--ansi`; clx prints it from the
/// hidden `--preview` subcommand.
pub fn preview_text(config: &Config, name: &str) -> String {
    let entry = match find_profile(config, name) {
        Some(e) => e,
        None => return String::new(),
    };

    let mut lines: Vec<String> = Vec::new();

    // Profile name in bright cyan.
    lines.push(paint(CYAN, name));

    // Description in yellow.
    if let Some(ref desc) = entry.description {
        lines.push(paint(YELLOW, desc));
    }

    lines.push(String::new());

    // Resolved env vars (placeholder mode — no real tokens).
    match resolve_profile(config, name) {
        Ok(resolved) => {
            let env_vars = compute_preview_env(&resolved);
            if env_vars.is_empty() {
                lines.push(paint(DIM, "  (no env overrides — bare claude launch)"));
            } else {
                for (k, v) in &env_vars {
                    lines.push(format!(
                        "  {}={} {}",
                        paint(DIM, k),
                        paint(GREEN, v),
                        paint(DIM, "\\")
                    ));
                }
            }
            let mut launch_line = format!("  {}", paint(BOLD, "claude"));
            if resolved.skip_permissions.unwrap_or(false) {
                launch_line.push_str(" --dangerously-skip-permissions");
            }
            lines.push(launch_line);
        }
        Err(e) => {
            lines.push(paint(RED, &format!("resolve error: {e}")));
        }
    }

    lines.join("\n")
}

/// Single-quote a string for safe embedding in a shell command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// MARK: - Public API

/// Outcome of an interactive profile selection.
pub enum Selection {
    /// The user picked a profile.
    Picked(String),
    /// The user cancelled (Esc / Ctrl-C) or matched nothing.
    Cancelled,
    /// fzf is not installed — the caller should fall back to a manual flow.
    FzfMissing,
}

/// Launch fzf to let the user pick a profile interactively.
///
/// Returns [`Selection::FzfMissing`] when fzf is not installed (so the caller
/// can fall back), [`Selection::Cancelled`] if the user aborted, or
/// [`Selection::Picked`] with the chosen name. Errors only on an unexpected I/O
/// failure while running fzf.
pub fn select_profile(config: &Config) -> Result<Selection, Error> {
    // Feed fzf one profile name per line. Only names are listed and searched;
    // each profile's description is shown in the preview panel instead.
    let stdin_lines: Vec<String> = config.profiles.iter().map(|p| p.name.clone()).collect();

    // Re-invoke ourselves as the preview command: fzf substitutes the selected
    // line (the profile name) for `{}`, already shell-quoted.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "clx".to_string());
    let preview_cmd = format!("{} --preview {{}}", shell_quote(&exe));

    let spawned = Command::new("fzf")
        .args([
            "--height=40%",
            "--no-sort",
            "--reverse",
            "--ansi",
            "--preview",
            &preview_cmd,
            "--preview-window=right:50%:wrap",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        // fzf not on PATH — signal the caller to fall back instead of erroring.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Selection::FzfMissing),
        Err(e) => return Err(Error::Fzf(e)),
    };

    // Write the profile list to fzf's stdin, then drop the handle: closing the
    // pipe is how fzf knows the input is complete.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_lines.join("\n").as_bytes());
    }

    let output = child.wait_with_output().map_err(Error::Fzf)?;

    match output.status.code() {
        Some(0) => {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(if name.is_empty() {
                Selection::Cancelled
            } else {
                Selection::Picked(name)
            })
        }
        // Exit codes 1, 130, etc. mean "no match" or "user cancelled".
        _ => Ok(Selection::Cancelled),
    }
}
