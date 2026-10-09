use std::io::Write;
use std::process::{Command, Stdio};

use crate::ansi::{BOLD, CYAN, DIM, GREEN, RED, YELLOW, paint};
use crate::config::Config;
use crate::error::Error;

// MARK: - Preview builder

/// Build the ANSI-coloured preview text for a profile.
/// fzf renders this in the preview panel via `--ansi`; clx prints it from the
/// hidden `--preview` subcommand. `extra_args` are this invocation's
/// passthrough, so the shown command line is the one that would actually run.
pub fn preview_text(config: &Config, name: &str, extra_args: &[String]) -> String {
    let entry = match config.profile(name) {
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

    // Env values stay as written: a `$VAR` reference stays opaque here, so a
    // secret routed through one is never shown. A literal value is shown as
    // written — the config file holds it in the clear anyway, so references are
    // the way to keep secrets out of sight.
    match config.resolve(name) {
        Ok(resolved) => {
            if resolved.env.is_empty() {
                lines.push(paint(DIM, "  (no env overrides — bare claude launch)"));
            } else {
                for (k, v) in &resolved.env {
                    lines.push(format!("  {}={}", paint(DIM, k), paint(GREEN, v.as_str())));
                }
            }
            let mut launch_line = format!("  {}", paint(BOLD, "claude"));
            for arg in resolved.args.merged_argv(extra_args) {
                launch_line.push(' ');
                launch_line.push_str(&paint(GREEN, &shell_quote(&arg)));
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
pub fn select_profile(config: &Config, extra_args: &[String]) -> Result<Selection, Error> {
    // Feed fzf one profile name per line. Only names are listed and searched;
    // each profile's description is shown in the preview panel instead.
    let stdin_lines: Vec<String> = config.profiles.iter().map(|p| p.name.clone()).collect();

    // Re-invoke ourselves as the preview command: fzf substitutes the selected
    // line (the profile name) for `{}`, already shell-quoted. This invocation's
    // passthrough rides along so the preview shows the real command line.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "clx".to_string());
    let mut preview_cmd = format!("{} --preview {{}}", shell_quote(&exe));
    if !extra_args.is_empty() {
        preview_cmd.push_str(" --");
        for arg in extra_args {
            preview_cmd.push(' ');
            // fzf substitutes `{...}` placeholders wherever it finds them, even
            // inside our quotes; break the placeholder syntax with a backslash,
            // which fzf strips again before the shell sees it.
            preview_cmd.push_str(&shell_quote(arg).replace('{', "\\{"));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Global, Profile};
    use crate::env::Template;

    #[test]
    fn preview_shows_references_verbatim_and_the_final_command_line() {
        let config = Config {
            global: Global::default(),
            profiles: vec![Profile {
                name: "work".into(),
                env: [(
                    "ANTHROPIC_AUTH_TOKEN".to_string(),
                    Template::parse("$TOKEN").unwrap(),
                )]
                .into_iter()
                .collect(),
                args: vec!["--model".into(), "opus".into()],
                ..Default::default()
            }],
        };

        let text = preview_text(&config, "work", &["--model".into(), "sonnet".into()]);

        assert!(text.contains("ANTHROPIC_AUTH_TOKEN"), "{text}");
        assert!(text.contains("$TOKEN"), "{text}"); // never expanded here
        assert!(text.contains("'--model'"), "{text}");
        assert!(text.contains("'sonnet'"), "{text}");
        assert!(!text.contains("'opus'"), "{text}"); // taken over by passthrough
    }
}
