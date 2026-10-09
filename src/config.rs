use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::env::Template;
use crate::error::Error;
use crate::suggest;

// MARK: - Data structures

/// Top-level config matching the config file structure.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Merged under every profile before its `extends` chain runs.
    #[serde(default)]
    pub global: Global,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

/// The `[global]` table — the implicit root of every profile. A profile's own
/// values (including via `extends`) take precedence.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Global {
    #[serde(default, deserialize_with = "crate::env::de_env_map")]
    pub env: BTreeMap<String, Template>,
    #[serde(default)]
    pub args: Vec<String>,
}

/// A profile as written in the config file; the launch-time form is
/// [`crate::profile::Resolved`].
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub extends: Option<String>,
    pub description: Option<String>,
    /// Omit from every listing (picker, fzf fallback, `--list`) — for profiles
    /// that exist only to be extended. Launchable by name as usual.
    #[serde(default)]
    pub hidden: bool,
    /// Environment variables to launch with, keyed by the exact variable name.
    #[serde(default, deserialize_with = "crate::env::de_env_map")]
    pub env: BTreeMap<String, Template>,
    /// Arguments for claude; merge rules live in [`crate::args`].
    #[serde(default)]
    pub args: Vec<String>,
}

impl Config {
    /// Load and validate `~/.config/clx/config.toml`.
    pub fn load() -> Result<Self, Error> {
        let path = config_path()?;

        if !path.exists() {
            return Err(Error::ConfigNotFound(
                path.display().to_string(),
                EXAMPLE_CONFIG.to_string(),
            ));
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| Error::ConfigRead(path.display().to_string(), e))?;

        let config: Self = toml::from_str(&content).map_err(|e| {
            Error::ConfigParse(path.display().to_string(), suggest::format_toml_error(e))
        })?;

        if config.profiles.is_empty() {
            return Err(Error::ConfigInvalid(
                path.display().to_string(),
                "no profiles defined".into(),
            ));
        }

        Ok(config)
    }

    /// Look up a profile by name, hidden or not.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// The profiles listings show: the picker, its fzf-missing fallback, and
    /// `--list`. Hidden profiles are omitted here but stay reachable through
    /// [`Config::profile`] and the `extends` chain.
    pub fn visible_profiles(&self) -> impl Iterator<Item = &Profile> {
        self.profiles.iter().filter(|p| !p.hidden)
    }

    /// Closest profile name to `name` for a "did you mean" hint; `Some` only
    /// when a reasonably close match exists.
    pub fn suggest_profile(&self, name: &str) -> Option<&str> {
        suggest::closest(name, self.profiles.iter().map(|p| p.name.as_str()))
    }
}

// MARK: - Constants

const EXAMPLE_CONFIG: &str = r##"[global]
# Merged under every profile; a profile's own value wins per key.
env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = "1"

[[profiles]]
name = "work"
description = "via custom gateway"
env.ANTHROPIC_BASE_URL = "https://gateway.example.com"
env.ANTHROPIC_AUTH_TOKEN = "$WORK_API_TOKEN"
env.ANTHROPIC_MODEL = "opus"
args = ["--dangerously-skip-permissions"]

[[profiles]]
name = "work-sonnet"
extends = "work"
description = "same gateway, cheaper model"
env.ANTHROPIC_MODEL = "sonnet"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(body)
    }

    #[test]
    fn suggest_profile_finds_a_close_typo_but_not_garbage() {
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
        assert_eq!(config.suggest_profile("wrok"), Some("work"));
        assert_eq!(config.suggest_profile("zzzzzzzz"), None);
    }

    #[test]
    fn env_values_accept_any_scalar() {
        let config = parse(
            "[[profiles]]\nname = \"p\"\n\
             env.A = \"text\"\nenv.B = 42\nenv.C = 1.5\nenv.D = true\n",
        )
        .unwrap();
        let env = &config.profiles[0].env;
        assert_eq!(env["A"].as_str(), "text");
        assert_eq!(env["B"].as_str(), "42");
        assert_eq!(env["C"].as_str(), "1.5");
        assert_eq!(env["D"].as_str(), "true");
    }

    #[test]
    fn env_values_reject_arrays_tables_and_datetimes() {
        for (body, shown_as) in [
            ("env.A = [1]", "sequence"),
            ("env.A = { inner = 1 }", "map"),
            ("env.A = 1979-05-27", "map"),
        ] {
            let err = parse(&format!("[[profiles]]\nname = \"p\"\n{body}\n")).unwrap_err();
            let text = err.to_string();
            assert!(text.contains("env.A:"), "{text}");
            assert!(text.contains(shown_as), "{text}");
            assert!(text.contains("not env values"), "{text}");
        }
    }

    #[test]
    fn malformed_dollar_syntax_is_rejected_at_parse_with_a_line() {
        let err = parse("[[profiles]]\nname = \"p\"\nenv.FOO = \"${\"\n").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("env.FOO"), "{text}");
        assert!(text.contains("line 3"), "{text}");
    }

    #[test]
    fn well_formed_dollar_templates_parse() {
        let config = parse("[[profiles]]\nname = \"p\"\nenv.FOO = \"${A}-$B-$$\"\n").unwrap();
        assert_eq!(config.profiles[0].env["FOO"].as_str(), "${A}-$B-$$");
    }

    #[test]
    fn unknown_field_typo_gets_a_suggestion() {
        let err = parse("[[profiles]]\nname = \"p\"\narg = [\"--verbose\"]\n").unwrap_err();
        let text = suggest::format_toml_error(err);
        assert!(text.contains("unknown field `arg`"), "{text}");
        assert!(text.contains("did you mean `args`?"), "{text}");
    }

    #[test]
    fn distant_unknown_field_has_no_suggestion() {
        let err = parse("[[profiles]]\nname = \"p\"\nzzzzzzzzzzzz = 1\n").unwrap_err();
        let text = suggest::format_toml_error(err);
        assert!(text.contains("unknown field `zzzzzzzzzzzz`"), "{text}");
        assert!(!text.contains("did you mean"), "{text}");
    }

    #[test]
    fn hidden_profiles_drop_out_of_listings_but_stay_resolvable() {
        let config = parse(
            "[[profiles]]\nname = \"base\"\nhidden = true\n\n[[profiles]]\nname = \"work\"\n",
        )
        .unwrap();
        let listed: Vec<&str> = config.visible_profiles().map(|p| p.name.as_str()).collect();
        assert_eq!(listed, ["work"]);
        assert!(config.profile("base").is_some());
    }

    #[test]
    fn example_config_parses() {
        let config = parse(EXAMPLE_CONFIG).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert!(
            config
                .global
                .env
                .contains_key("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC")
        );
        assert_eq!(config.profiles[1].extends.as_deref(), Some("work"));
    }
}
