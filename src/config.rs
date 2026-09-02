use std::path::PathBuf;

use serde::Deserialize;
use serde::de::{self, Deserializer};

use crate::error::Error;

// MARK: - Data structures

/// Top-level config matching the config file structure.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct Global {
    /// Default for passing `--dangerously-skip-permissions`.
    pub skip_permissions: Option<bool>,
}

/// A profile — both the config entry and the launch-time representation.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Assumed context window (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`).
    #[serde(default, deserialize_with = "de_max_context_tokens")]
    pub max_context_tokens: Option<u32>,
    /// Override whether `--dangerously-skip-permissions` is passed.
    pub skip_permissions: Option<bool>,
    /// Effort level (`CLAUDE_CODE_EFFORT_LEVEL`).
    pub effort_level: Option<EffortLevel>,
}

/// Claude Code effort levels. Values match the env var / `--effort` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Auto,
}

impl EffortLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Auto => "auto",
        }
    }
}

/// Model overrides for a profile.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Models {
    pub default: Option<String>,
    pub default_haiku: Option<String>,
    pub default_sonnet: Option<String>,
    pub default_opus: Option<String>,
    pub subagent: Option<String>,
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
        set!(default_haiku);
        set!(default_sonnet);
        set!(default_opus);
        set!(subagent);
    }
}

/// API provider configuration. Exactly one auth source must be set: `env_key`
/// names an environment variable holding the token (keeping the secret out of
/// the config file), while `key` embeds the token directly.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub base_url: String,
    pub env_key: Option<String>,
    pub key: Option<String>,
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

    let config: Config = toml::from_str(&content)
        .map_err(|e| Error::ConfigParse(path.display().to_string(), format_toml_error(e)))?;

    if config.profiles.is_empty() {
        return Err(Error::ConfigInvalid(
            path.display().to_string(),
            "no profiles defined".into(),
        ));
    }

    for profile in &config.profiles {
        if let Some(msg) = provider_auth_error(profile) {
            return Err(Error::ConfigInvalid(
                path.display().to_string(),
                format!("profile \"{}\": {msg}", profile.name),
            ));
        }
    }

    Ok(config)
}

/// Validate a profile's provider auth: exactly one of `env_key` or `key` must
/// be set. Returns a human-readable message describing the problem, or `None`
/// when the provider is absent or well-formed.
fn provider_auth_error(profile: &Profile) -> Option<&'static str> {
    let provider = profile.provider.as_ref()?;
    match (&provider.env_key, &provider.key) {
        (Some(_), Some(_)) => Some("provider sets both `env_key` and `key`; set only one"),
        (None, None) => Some("provider must set either `env_key` or `key`"),
        _ => None,
    }
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

/// Deserialize `max_context_tokens`, requiring a positive count so the error
/// carries the offending line from the TOML.
fn de_max_context_tokens<'de, D>(de: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<u32>::deserialize(de)?;
    if let Some(tokens) = value
        && tokens == 0
    {
        return Err(de::Error::custom(
            "max_context_tokens must be a positive integer, got 0",
        ));
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
    suggest(name, config.profiles.iter().map(|p| p.name.as_str())).map(str::to_owned)
}

/// Format a TOML deserialize error, appending a did-you-mean hint when serde
/// rejected an unknown field that is close to a known one.
fn format_toml_error(err: toml::de::Error) -> String {
    let mut text = err.to_string();
    let hint = unknown_field_hint(err.message()).or_else(|| unknown_field_hint(&text));
    if let Some(hint) = hint {
        text.push('\n');
        text.push_str(&hint);
    }
    text
}

/// Parse serde's `unknown field` message and, if a close known field exists,
/// return `did you mean \`field\`?`.
fn unknown_field_hint(msg: &str) -> Option<String> {
    let (unknown, expected) = parse_unknown_field_error(msg)?;
    let hint = suggest(unknown, expected)?;
    Some(format!("did you mean `{hint}`?"))
}

/// Pull `(unknown, expected_fields)` out of serde's unknown-field wording:
/// - `unknown field \`foo\`, expected \`bar\``
/// - `unknown field \`foo\`, expected \`bar\` or \`baz\``
/// - `unknown field \`foo\`, expected one of \`a\`, \`b\`, \`c\``
fn parse_unknown_field_error(msg: &str) -> Option<(&str, Vec<&str>)> {
    let rest = msg.split("unknown field `").nth(1)?;
    let (unknown, rest) = rest.split_once('`')?;
    let rest = rest.trim_start_matches([' ', ',']);
    let rest = rest.strip_prefix("expected ")?;
    if rest.starts_with("there are no fields") {
        return None;
    }
    let expected = if let Some(list) = rest.strip_prefix("one of ") {
        parse_backticked_list(list)
    } else if let Some((a, b)) = rest.split_once("` or `") {
        vec![a.trim_matches('`'), b.trim_matches('`')]
    } else {
        vec![rest.trim_matches('`').trim_end_matches(['.', ' '])]
    };
    let expected: Vec<&str> = expected.into_iter().filter(|s| !s.is_empty()).collect();
    (!expected.is_empty()).then_some((unknown, expected))
}

fn parse_backticked_list(list: &str) -> Vec<&str> {
    list.split("`, `")
        .map(|s| s.trim().trim_matches('`').trim_end_matches(['.', ' ']))
        .collect()
}

/// Closest candidate by Levenshtein distance, if close enough to be a typo.
fn suggest<'a>(unknown: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|candidate| (levenshtein(candidate, unknown), candidate))
        .filter(|(d, candidate)| *d > 0 && *d <= candidate.len().max(unknown.len()) / 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, candidate)| candidate)
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
    fn provider_auth_requires_exactly_one_source() {
        let provider = |env_key: Option<&str>, key: Option<&str>| Profile {
            provider: Some(Provider {
                base_url: "https://gw".into(),
                env_key: env_key.map(Into::into),
                key: key.map(Into::into),
            }),
            ..Default::default()
        };
        assert!(provider_auth_error(&provider(Some("K"), None)).is_none());
        assert!(provider_auth_error(&provider(None, Some("sk-x"))).is_none());
        assert!(provider_auth_error(&provider(Some("K"), Some("sk-x"))).is_some());
        assert!(provider_auth_error(&provider(None, None)).is_some());
        // No provider at all is fine.
        assert!(provider_auth_error(&Profile::default()).is_none());
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

    #[test]
    fn max_context_tokens_must_be_positive() {
        let parse = |tokens: u32| {
            toml::from_str::<Config>(&format!(
                "[[profiles]]\nname = \"p\"\nmax_context_tokens = {tokens}\n"
            ))
        };
        assert!(parse(0).is_err());
        assert_eq!(
            parse(1_000_000).unwrap().profiles[0].max_context_tokens,
            Some(1_000_000)
        );
    }

    #[test]
    fn unknown_model_field_is_rejected_with_suggestion() {
        let err = toml::from_str::<Config>(
            "[[profiles]]\nname = \"p\"\nmodels.default_opus_model = \"grok-4.6\"\n",
        )
        .unwrap_err();
        let text = format_toml_error(err);
        assert!(
            text.contains("unknown field `default_opus_model`"),
            "{text}"
        );
        assert!(text.contains("did you mean `default_opus`?"), "{text}");
    }

    #[test]
    fn unknown_profile_key_suggests_models() {
        let err =
            toml::from_str::<Config>("[[profiles]]\nname = \"p\"\nmodel.default = \"opus\"\n")
                .unwrap_err();
        let text = format_toml_error(err);
        assert!(text.contains("unknown field `model`"), "{text}");
        assert!(text.contains("did you mean `models`?"), "{text}");
    }

    #[test]
    fn distant_unknown_field_has_no_suggestion() {
        let err =
            toml::from_str::<Config>("[[profiles]]\nname = \"p\"\nzzzzzzzzzzzz = 1\n").unwrap_err();
        let text = format_toml_error(err);
        assert!(text.contains("unknown field `zzzzzzzzzzzz`"), "{text}");
        assert!(!text.contains("did you mean"), "{text}");
    }

    #[test]
    fn parse_unknown_field_error_handles_serde_wordings() {
        let one = parse_unknown_field_error("unknown field `foo`, expected `bar`").unwrap();
        assert_eq!(one.0, "foo");
        assert_eq!(one.1, ["bar"]);

        let two =
            parse_unknown_field_error("unknown field `foo`, expected `bar` or `baz`").unwrap();
        assert_eq!(two.0, "foo");
        assert_eq!(two.1, ["bar", "baz"]);

        let many = parse_unknown_field_error(
            "unknown field `default_opus_model`, expected one of `default`, `default_haiku`, `default_sonnet`, `default_opus`, `subagent`",
        )
        .unwrap();
        assert_eq!(many.0, "default_opus_model");
        assert_eq!(
            many.1,
            [
                "default",
                "default_haiku",
                "default_sonnet",
                "default_opus",
                "subagent"
            ]
        );
        assert_eq!(suggest(many.0, many.1), Some("default_opus"));
    }

    #[test]
    fn effort_level_accepts_known_values_only() {
        let parse = |level: &str| {
            toml::from_str::<Config>(&format!(
                "[[profiles]]\nname = \"p\"\neffort_level = \"{level}\"\n"
            ))
        };
        for level in ["low", "medium", "high", "xhigh", "max", "auto"] {
            let config = parse(level).unwrap();
            assert_eq!(config.profiles[0].effort_level.unwrap().as_str(), level);
        }
        assert!(parse("ultra").is_err());
    }
}
