use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::env::{ExpandError, Lookup, Template};
use crate::error::Error;
use crate::profile::Resolved;

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
    let argv = resolved.args.merged_argv(extra_args);
    let mut cmd = build_command(&argv, &env, std::env::vars_os().map(|(name, _)| name));
    Err(Error::ExecFailed(cmd.exec()))
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

    fn templates(pairs: &[(&str, &str)]) -> BTreeMap<String, Template> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Template::parse(v).unwrap()))
            .collect()
    }
}
