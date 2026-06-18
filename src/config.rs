use std::path::PathBuf;

use serde::Deserialize;
use serde::de::{self, Deserializer};

use crate::error::Error;

// MARK: - Data structures

/// Top-level config matching the config file structure.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Global defaults applied to every profile unless overridden.
    #[serde(default)]
    pub global: Global,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// Global config defaults — the `[global]` table. A profile's own value
/// (including via `extends`) takes precedence over these.
#[derive(Debug, Default, Deserialize)]
pub struct Global {
    /// Default for passing `--dangerously-skip-permissions`.
    pub skip_permissions: Option<bool>,
}

/// A profile — both the config entry and the launch-time representation.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Profile {
    pub name: String,
    pub extends: Option<String>,
    pub description: Option<String>,
    pub models: Option<Models>,
    pub provider: Option<Provider>,
    /// Auto-compaction threshold percentage (1-100).
    #[serde(default, deserialize_with = "de_auto_compact_pct")]
    pub auto_compact_pct: Option<u32>,
    /// Auto-compaction window size.
    pub auto_compact_window: Option<u32>,
    /// Override whether `--dangerously-skip-permissions` is passed.
    pub skip_permissions: Option<bool>,
}

/// Model overrides for a profile.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct Models {
    pub default: Option<String>,
    pub small_fast: Option<String>,
    pub default_haiku: Option<String>,
    pub default_sonnet: Option<String>,
    pub default_opus: Option<String>,
}

impl Models {
    /// Overlay `other` on top of `self`, letting each set field in `other`
    /// override `self`. Used to merge a child profile's models over its parent.
    pub fn overlay(&mut self, other: &Models) {
        macro_rules! set {
            ($f:ident) => {
                if other.$f.is_some() {
                    self.$f = other.$f.clone();
                }
            };
        }
        set!(default);
        set!(small_fast);
        set!(default_haiku);
        set!(default_sonnet);
        set!(default_opus);
    }
}

/// API provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub base_url: String,
    pub env_key: String,
}

// MARK: - Constants

const EXAMPLE_CONFIG: &str = r##"[[profiles]]
name = "default"
description = "Anthropic API"
models.default = "opus"

[[profiles]]
name = "work"
extends = "default"
description = "via custom gateway"
provider.base_url = "https://gateway.example.com"
provider.env_key = "WORK_API_TOKEN"
"##;

// MARK: - Config loading

/// Return the path to `~/.config/clx/config.toml`.
fn config_path() -> Result<PathBuf, Error> {
    // Respect XDG_CONFIG_HOME, falling back to ~/.config.
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map_err(|_| Error::NoConfigDir)?;
    Ok(base.join("clx").join("config.toml"))
}

/// Load and validate the config file.
pub fn load_config() -> Result<Config, Error> {
    let path = config_path()?;

    if !path.exists() {
        return Err(Error::ConfigNotFound(
            path.display().to_string(),
            EXAMPLE_CONFIG.to_string(),
        ));
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| Error::ConfigRead(path.display().to_string(), e))?;

    let config: Config =
        toml::from_str(&content).map_err(|e| Error::ConfigParse(path.display().to_string(), e))?;

    if config.profiles.is_empty() {
        return Err(Error::ConfigInvalid(
            path.display().to_string(),
            "no profiles defined".into(),
        ));
    }

    Ok(config)
}

/// Deserialize `auto_compact_pct`, enforcing the documented 1-100 range at
/// parse time so the error carries the offending line from the TOML.
fn de_auto_compact_pct<'de, D>(de: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<u32>::deserialize(de)?;
    if let Some(pct) = value
        && !(1..=100).contains(&pct)
    {
        return Err(de::Error::custom(format!(
            "auto_compact_pct must be 1-100, got {pct}"
        )));
    }
    Ok(value)
}

/// Look up a profile by name.
pub fn find_profile<'a>(config: &'a Config, name: &str) -> Option<&'a Profile> {
    config.profiles.iter().find(|p| p.name == name)
}

/// Suggest the closest profile name to `name` for a "did you mean" hint.
/// Returns `Some` only when a reasonably close match exists.
pub fn suggest_profile(config: &Config, name: &str) -> Option<String> {
    config
        .profiles
        .iter()
        .map(|p| (levenshtein(&p.name, name), &p.name))
        .filter(|(d, candidate)| *d <= candidate.len().max(name.len()) / 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, candidate)| candidate.clone())
}

/// Plain Levenshtein edit distance — small, no external dependency.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }

    #[test]
    fn suggest_finds_close_typo_but_not_garbage() {
        let config = Config {
            profiles: vec![
                Profile {
                    name: "work".into(),
                    ..Default::default()
                },
                Profile {
                    name: "personal".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(suggest_profile(&config, "wrok").as_deref(), Some("work"));
        assert_eq!(suggest_profile(&config, "zzzzzzzz"), None);
    }

    #[test]
    fn auto_compact_pct_range_enforced_at_parse() {
        let parse = |pct: u32| {
            toml::from_str::<Config>(&format!(
                "[[profiles]]\nname = \"p\"\nauto_compact_pct = {pct}\n"
            ))
        };
        assert!(parse(0).is_err());
        assert!(parse(101).is_err());
        assert!(parse(50).is_ok());
    }
}
