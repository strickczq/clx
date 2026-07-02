use crate::config::{Config, Profile, find_profile, suggest_profile};
use crate::error::Error;

/// Resolve a profile through its inheritance chain, merging from base ancestor
/// down to the requested profile so derived profiles override their parents.
pub fn resolve_profile(config: &Config, name: &str) -> Result<Profile, Error> {
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

        let entry = find_profile(config, cur).ok_or_else(|| match chain.last() {
            Some(parent) => Error::UnknownParent(parent.name.clone(), cur.to_string()),
            None => {
                let hint = match suggest_profile(config, cur) {
                    Some(s) => format!(" — did you mean \"{s}\"?"),
                    None => String::new(),
                };
                Error::ProfileNotFound(cur.to_string(), hint)
            }
        })?;

        chain.push(entry);
        current = entry.extends.as_deref();
    }

    // Merge from ancestor to descendant.
    let mut merged = Profile::default();

    for entry in chain.iter().rev() {
        // Merge models field-by-field (child non-None values override parent).
        if let Some(ref models) = entry.models {
            merged
                .models
                .get_or_insert_with(Default::default)
                .overlay(models);
        }

        // Provider: child overrides parent entirely.
        if entry.provider.is_some() {
            merged.provider = entry.provider.clone();
        }

        // Auto-compaction: child overrides parent.
        if entry.auto_compact_pct.is_some() {
            merged.auto_compact_pct = entry.auto_compact_pct;
        }
        if entry.auto_compact_window.is_some() {
            merged.auto_compact_window = entry.auto_compact_window;
        }

        // skip_permissions: child overrides parent.
        if entry.skip_permissions.is_some() {
            merged.skip_permissions = entry.skip_permissions;
        }
    }

    // Fall back to the global default when no profile in the chain set it.
    if merged.skip_permissions.is_none() {
        merged.skip_permissions = config.global.skip_permissions;
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Global, Models, Provider};

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

    #[test]
    fn child_overrides_parent_and_inherits_unset() {
        let base = Profile {
            models: Some(Models {
                default: Some("opus".into()),
                small_fast: Some("haiku".into()),
                ..Default::default()
            }),
            provider: Some(Provider {
                base_url: "https://base".into(),
                env_key: Some("BASE_KEY".into()),
                key: None,
            }),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            models: Some(Models {
                default: Some("sonnet".into()),
                ..Default::default()
            }),
            ..profile("child")
        };
        let resolved = resolve_profile(&cfg(vec![base, child]), "child").unwrap();
        let models = resolved.models.unwrap();
        assert_eq!(models.default.as_deref(), Some("sonnet")); // overridden
        assert_eq!(models.small_fast.as_deref(), Some("haiku")); // inherited
        // Provider is inherited untouched.
        assert_eq!(
            resolved.provider.unwrap().env_key.as_deref(),
            Some("BASE_KEY")
        );
    }

    #[test]
    fn global_skip_permissions_is_fallback_only() {
        let mut config = cfg(vec![profile("plain")]);
        config.global.skip_permissions = Some(true);
        let resolved = resolve_profile(&config, "plain").unwrap();
        assert_eq!(resolved.skip_permissions, Some(true));

        let mut config = cfg(vec![Profile {
            skip_permissions: Some(false),
            ..profile("explicit")
        }]);
        config.global.skip_permissions = Some(true);
        let resolved = resolve_profile(&config, "explicit").unwrap();
        assert_eq!(resolved.skip_permissions, Some(false)); // profile wins
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
        let err = resolve_profile(&cfg(vec![a, b]), "a").unwrap_err();
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
            resolve_profile(&config, "leaf").unwrap_err(),
            Error::UnknownParent(..)
        ));
        assert!(matches!(
            resolve_profile(&config, "nope").unwrap_err(),
            Error::ProfileNotFound(..)
        ));
    }
}
