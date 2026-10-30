use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::env::{ExpandError, Lookup, Template};
use crate::error::Error;
use crate::profile::Resolved;
use crate::shell;

/// Variable namespaces that belong to claude's own configuration. Everything
/// inherited under these prefixes is wiped, so the profile is the only source
/// for how claude authenticates, routes, and configures itself — a leftover
/// `ANTHROPIC_API_KEY`, for instance, would beat a profile's
/// `ANTHROPIC_AUTH_TOKEN` inside claude. Names outside the prefixes pass
/// through unless the profile declares them.
const MANAGED_PREFIXES: &[&str] = &["ANTHROPIC_", "CLAUDE_CODE_"];

/// Whether clx keeps this variable out of claude's inherited environment.
fn is_managed(name: &OsStr) -> bool {
    MANAGED_PREFIXES
        .iter()
        .any(|prefix| name.as_encoded_bytes().starts_with(prefix.as_bytes()))
}

/// Resolve `$VAR` references in the profile's env values.
///
/// Lookups read clx's own environment: the wipe in [`build_command`] only
/// affects the child, so a value may reference a variable the profile itself
/// overrides.
fn expand_env(
    env: &BTreeMap<String, Template>,
    lookup: &impl Fn(&str) -> Lookup,
) -> Result<BTreeMap<String, String>, Error> {
    env.iter()
        .map(|(key, template)| {
            let value = template.expand(lookup).map_err(|e| match e {
                ExpandError::Missing(var) => Error::MissingEnvVar {
                    key: key.clone(),
                    var,
                },
                ExpandError::NotUnicode(var) => Error::NotUnicodeEnvVar {
                    key: key.clone(),
                    var,
                },
            })?;
            Ok((key.clone(), value))
        })
        .collect()
}

/// Look a variable up in clx's own environment, keeping "unset" and "set but
/// not valid Unicode" distinguishable.
fn lookup_env(name: &str) -> Lookup {
    match std::env::var_os(name) {
        None => Lookup::Missing,
        Some(value) => match value.into_string() {
            Ok(value) => Lookup::Value(value),
            Err(_) => Lookup::NotUnicode,
        },
    }
}

/// The exact process clx execs: managed namespaces wiped from the inherited
/// environment, then the profile's env, then the args. Wipe before apply:
/// `Command` keeps one entry per key and the last write wins.
fn build_command(
    argv: &[String],
    env: &BTreeMap<String, String>,
    inherited: impl Iterator<Item = OsString>,
) -> Command {
    let mut cmd = Command::new("claude");
    for name in inherited {
        if is_managed(&name) {
            cmd.env_remove(name);
        }
    }
    cmd.envs(env);
    cmd.args(argv);
    cmd
}

/// Flags whose text clx owns through profile fields. Seeing one in `args`
/// means two sources compete for a flag claude reads once, so clx rejects it
/// instead of letting one silently win.
const MANAGED_FLAGS: &[&str] = &[
    "--append-system-prompt",
    "--append-system-prompt-file",
    "--agents",
];

/// Add the flags clx owns from the resolved profile fields and reject
/// hand-written ones in `args`.
fn with_managed_flags(mut argv: Vec<String>, resolved: &Resolved) -> Result<Vec<String>, Error> {
    for arg in &argv {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if MANAGED_FLAGS.contains(&flag) {
            return Err(Error::ReservedFlag(flag.to_owned()));
        }
    }
    if !resolved.append_system_prompt.is_empty() {
        inject_flag(
            &mut argv,
            "--append-system-prompt",
            &resolved.append_system_prompt.join("\n\n"),
        );
    }
    if let Some(agents) = &resolved.agents {
        inject_flag(&mut argv, "--agents", agents);
    }
    Ok(argv)
}

/// Insert `--flag <value>` before any `--` separator, so it stays a flag
/// instead of becoming positional text.
fn inject_flag(argv: &mut Vec<String>, flag: &str, value: &str) {
    let at = argv
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(argv.len());
    argv.splice(at..at, [flag.to_owned(), value.to_owned()]);
}

/// Replace the current process with `claude`, passing through extra args.
///
/// `claude` inherits clx's full environment except the managed namespaces (see
/// [`MANAGED_PREFIXES`]), so `TERM`, locale, display, etc. all pass through; a
/// variable the profile declares wins over whatever was inherited.
///
/// Uses `.exec()` which replaces the current process on Unix — this function
/// never returns on success.
pub fn launch(resolved: &Resolved, extra_args: &[String]) -> Result<(), Error> {
    let env = expand_env(&resolved.env, &lookup_env)?;
    let argv = with_managed_flags(resolved.args.merged_argv(extra_args), resolved)?;
    let mut cmd = build_command(&argv, &env, std::env::vars_os().map(|(name, _)| name));
    Err(Error::ExecFailed(cmd.exec()))
}

/// The command `launch` would run: expanded env assignments, then the argv,
/// quoted for a POSIX shell. `$VAR` references are expanded here, so the
/// output can contain secrets.
pub fn render(resolved: &Resolved, extra_args: &[String]) -> Result<String, Error> {
    let env = expand_env(&resolved.env, &lookup_env)?;
    let argv = with_managed_flags(resolved.args.merged_argv(extra_args), resolved)?;

    let mut out = String::new();
    for (key, value) in &env {
        out.push_str(key);
        out.push('=');
        out.push_str(&shell::quote(value));
        out.push_str(" \\\n");
    }
    out.push_str("claude");
    for arg in &argv {
        out.push(' ');
        out.push_str(&shell::quote(arg));
    }
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn command_env(cmd: &Command) -> BTreeMap<String, Option<String>> {
        cmd.get_envs()
            .map(|(k, v)| {
                (
                    k.to_str().unwrap().to_owned(),
                    v.map(|v| v.to_str().unwrap().to_owned()),
                )
            })
            .collect()
    }

    #[test]
    fn managed_prefixes_are_exact_namespaces() {
        for managed in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_SESSION_ID",
            "ANTHROPIC_",
        ] {
            assert!(is_managed(OsStr::new(managed)), "{managed}");
        }
        for untouched in [
            "ANTHROPICISH",
            "CLAUDE_CONFIG_DIR", // deliberately not under CLAUDE_CODE_
            "CLAUDECODE",
            "PATH",
            "FOO_ANTHROPIC_BAR",
        ] {
            assert!(!is_managed(OsStr::new(untouched)), "{untouched}");
        }
    }

    #[test]
    fn build_command_wipes_managed_namespaces_then_applies_the_profile() {
        let env = strings(&[("ANTHROPIC_MODEL", "opus")]);
        let inherited = [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_SESSION_ID",
            "PATH",
            "HTTP_PROXY",
        ]
        .into_iter()
        .map(OsString::from);

        let cmd = build_command(&strs(&["--model", "opus"]), &env, inherited);

        let got = command_env(&cmd);
        assert_eq!(got["ANTHROPIC_API_KEY"], None); // wiped, never set
        assert_eq!(got["CLAUDE_CODE_SESSION_ID"], None); // wiped, never set
        assert_eq!(got["ANTHROPIC_MODEL"], Some("opus".into())); // wiped then set
        // Variables outside the managed namespaces are not touched at all.
        assert!(!got.contains_key("PATH"));
        assert!(!got.contains_key("HTTP_PROXY"));
        assert_eq!(
            cmd.get_args()
                .map(|a| a.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["--model", "opus"]
        );
    }

    #[test]
    fn expand_env_reads_the_lookup() {
        let env = templates(&[("ANTHROPIC_AUTH_TOKEN", "$TOKEN")]);
        let expanded = expand_env(&env, &|name| {
            if name == "TOKEN" {
                Lookup::Value("sk-x".to_string())
            } else {
                Lookup::Missing
            }
        })
        .unwrap();
        assert_eq!(expanded["ANTHROPIC_AUTH_TOKEN"], "sk-x");
    }

    #[test]
    fn expand_env_reports_missing_variables_with_their_key() {
        let env = templates(&[("ANTHROPIC_AUTH_TOKEN", "$TOKEN")]);
        match expand_env(&env, &|_| Lookup::Missing).unwrap_err() {
            Error::MissingEnvVar { key, var } => {
                assert_eq!(key, "ANTHROPIC_AUTH_TOKEN");
                assert_eq!(var, "TOKEN");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn prompts_from_the_chain_are_joined_before_the_separator() {
        let resolved = Resolved {
            append_system_prompt: vec!["base".into(), "child".into()],
            ..Default::default()
        };
        let argv = with_managed_flags(strs(&["--model", "opus", "--", "do the thing"]), &resolved)
            .unwrap();
        assert_eq!(
            argv,
            strs(&[
                "--model",
                "opus",
                "--append-system-prompt",
                "base\n\nchild",
                "--",
                "do the thing",
            ])
        );
    }

    #[test]
    fn a_hand_written_prompt_flag_in_args_is_rejected() {
        for flag in [
            "--append-system-prompt",
            "--append-system-prompt-file",
            "--agents",
        ] {
            let err = with_managed_flags(strs(&[flag, "text"]), &Resolved::default()).unwrap_err();
            assert!(matches!(err, Error::ReservedFlag(_)), "{flag}");
        }
        let err = with_managed_flags(strs(&["--append-system-prompt=text"]), &Resolved::default())
            .unwrap_err();
        assert!(matches!(err, Error::ReservedFlag(_)));
    }

    #[test]
    fn agents_json_becomes_an_agents_flag() {
        let resolved = Resolved {
            agents: Some(r#"{"worker":{"model":"haiku"}}"#.into()),
            ..Default::default()
        };
        let argv = with_managed_flags(strs(&["--model", "opus"]), &resolved).unwrap();
        assert_eq!(
            argv,
            strs(&[
                "--model",
                "opus",
                "--agents",
                r#"{"worker":{"model":"haiku"}}"#
            ])
        );
    }

    #[test]
    fn render_prints_env_and_the_full_command() {
        let resolved = Resolved {
            env: templates(&[("ANTHROPIC_MODEL", "m")]),
            append_system_prompt: vec!["one".into(), "two".into()],
            agents: Some(r#"{"worker":{}}"#.into()),
            ..Default::default()
        };
        let out = render(&resolved, &[]).unwrap();
        assert!(out.contains("ANTHROPIC_MODEL='m' \\\n"), "{out}");
        assert!(out.contains("--append-system-prompt"), "{out}");
        assert!(out.contains("'one\n\ntwo'"), "{out}");
        assert!(out.contains("--agents' '{\"worker\":{}}'"), "{out}");
    }

    #[test]
    fn no_prompt_fields_adds_no_flags() {
        let argv = with_managed_flags(strs(&["--model", "opus"]), &Resolved::default()).unwrap();
        assert_eq!(argv, strs(&["--model", "opus"]));
    }

    fn templates(pairs: &[(&str, &str)]) -> BTreeMap<String, Template> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Template::parse(v).unwrap()))
            .collect()
    }
}
