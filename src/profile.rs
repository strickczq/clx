use std::collections::BTreeMap;

use crate::args::ArgList;
use crate::config::{Config, Profile};
use crate::env::Template;
use crate::error::Error;

/// A profile with `[global]` and the whole `extends` chain folded in — the
/// values that actually reach the launcher.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    /// Templates kept unexpanded: `$` references resolve at launch, so the
    /// picker preview can show `$TOKEN` rather than a secret.
    pub env: BTreeMap<String, Template>,
    pub args: ArgList,
}

impl Resolved {
    /// Fold one generation's values in: env per key, args per flag.
    fn overlay(&mut self, env: &BTreeMap<String, Template>, args: &[String]) {
        self.env
            .extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.args.merge(args);
    }
}

// Resolving is the profile domain's work over config data, so the method lives
// beside `Resolved` instead of in config.rs.
impl Config {
    /// Resolve a profile through its inheritance chain: `[global]` first, then
    /// each ancestor down to the requested profile, so derived profiles override
    /// their parents.
    pub fn resolve(&self, name: &str) -> Result<Resolved, Error> {
        let mut chain: Vec<&Profile> = Vec::new();
        let mut current: Option<&str> = Some(name);

        // Walk the extends chain, collecting profile references.
        while let Some(cur) = current {
            if chain.iter().any(|p| p.name == cur) {
                let cycle = chain
                    .iter()
                    .map(|p| p.name.as_str())
                    .chain(std::iter::once(cur))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                return Err(Error::CircularExtends(cycle));
            }

            let entry = self.profile(cur).ok_or_else(|| match chain.last() {
                Some(parent) => Error::UnknownParent(parent.name.clone(), cur.to_string()),
                None => {
                    let hint = match self.suggest_profile(cur) {
                        Some(s) => format!(" — did you mean \"{s}\"?"),
                        None => String::new(),
                    };
                    Error::ProfileNotFound(cur.to_string(), hint)
                }
            })?;

            chain.push(entry);
            current = entry.extends.as_deref();
        }

        // Merge from the implicit root down to the requested profile.
        let mut resolved = Resolved::default();
        resolved.overlay(&self.global.env, &self.global.args);
        for entry in chain.iter().rev() {
            resolved.overlay(&entry.env, &entry.args);
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Global;

    fn cfg(profiles: Vec<Profile>) -> Config {
        Config {
            global: Global::default(),
            profiles,
        }
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            ..Default::default()
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, Template> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Template::parse(v).unwrap()))
            .collect()
    }

    fn value<'a>(env: &'a BTreeMap<String, Template>, key: &str) -> &'a str {
        env[key].as_str()
    }

    fn strs(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn env_overlays_per_key_and_inherits_untouched() {
        let base = Profile {
            env: map(&[
                ("ANTHROPIC_MODEL", "opus"),
                ("ANTHROPIC_BASE_URL", "https://base"),
            ]),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            env: map(&[("ANTHROPIC_MODEL", "sonnet")]),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        assert_eq!(value(&resolved.env, "ANTHROPIC_MODEL"), "sonnet"); // overridden
        assert_eq!(value(&resolved.env, "ANTHROPIC_BASE_URL"), "https://base"); // inherited
    }

    #[test]
    fn global_is_the_root_of_every_chain() {
        let mut config = cfg(vec![
            Profile {
                extends: Some("base".into()),
                ..profile("leaf")
            },
            Profile {
                env: map(&[("ANTHROPIC_BASE_URL", "https://base")]),
                args: strs(&["--model", "opus"]),
                ..profile("base")
            },
        ]);
        config.global.env = map(&[
            ("ANTHROPIC_MODEL", "haiku"),
            ("ANTHROPIC_BASE_URL", "https://global"),
        ]);
        config.global.args = strs(&["--dangerously-skip-permissions"]);

        let resolved = config.resolve("leaf").unwrap();
        assert_eq!(value(&resolved.env, "ANTHROPIC_MODEL"), "haiku"); // only global sets it
        assert_eq!(value(&resolved.env, "ANTHROPIC_BASE_URL"), "https://base"); // base wins over global
        assert_eq!(
            resolved.args.merged_argv(&[]),
            strs(&["--dangerously-skip-permissions", "--model", "opus"])
        );
    }

    #[test]
    fn child_args_take_over_and_accumulate() {
        let base = Profile {
            args: strs(&["--model=opus", "--plugin-dir", "a"]),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            args: strs(&["--model=sonnet", "--plugin-dir", "b"]),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        // The taken-over flag lands with the child's items; list flags keep
        // their relative order and accumulate.
        assert_eq!(
            resolved.args.merged_argv(&[]),
            strs(&["--plugin-dir", "a", "--model=sonnet", "--plugin-dir", "b"])
        );
    }

    #[test]
    fn hidden_extends_target_resolves() {
        let base = Profile {
            hidden: true,
            env: map(&[("ANTHROPIC_MODEL", "opus")]),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        assert_eq!(value(&resolved.env, "ANTHROPIC_MODEL"), "opus");
    }

    #[test]
    fn detects_cycles() {
        let a = Profile {
            extends: Some("b".into()),
            ..profile("a")
        };
        let b = Profile {
            extends: Some("a".into()),
            ..profile("b")
        };
        let err = cfg(vec![a, b]).resolve("a").unwrap_err();
        assert!(matches!(err, Error::CircularExtends(_)));
    }

    #[test]
    fn unknown_profile_vs_unknown_parent() {
        let leaf = Profile {
            extends: Some("ghost".into()),
            ..profile("leaf")
        };
        let config = cfg(vec![leaf]);
        assert!(matches!(
            config.resolve("leaf").unwrap_err(),
            Error::UnknownParent(..)
        ));
        assert!(matches!(
            config.resolve("nope").unwrap_err(),
            Error::ProfileNotFound(..)
        ));
    }
}
