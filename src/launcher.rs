use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::config::Profile;
use crate::error::Error;

// MARK: - Environment computation

/// Build the profile-specific environment variables for a resolved profile.
///
/// When `reveal` is `true`, the provider's `ANTHROPIC_AUTH_TOKEN` is resolved
/// from the real environment.  When `false`, a placeholder (`$ENVVAR`) is used
/// instead — safe for displaying in the preview panel.
fn build_env(profile: &Profile, reveal: bool) -> Result<Vec<(String, String)>, Error> {
    let mut env = Vec::new();

    // Model overrides.
    if let Some(ref models) = profile.models {
        let model_vars = [
            ("ANTHROPIC_MODEL", &models.default),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &models.default_haiku),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", &models.default_sonnet),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", &models.default_opus),
            ("CLAUDE_CODE_SUBAGENT_MODEL", &models.subagent),
        ];
        for (key, value) in model_vars {
            if let Some(v) = value {
                env.push((key.into(), v.clone()));
            }
        }
    }

    // Provider.
    if let Some(ref provider) = profile.provider {
        env.push(("ANTHROPIC_BASE_URL".into(), provider.base_url.clone()));
        // The token comes from either an env var (`env_key`) or an inline
        // literal (`key`); config validation guarantees exactly one is set. In
        // preview mode we never emit the real secret — an env var shows as
        // `$NAME` and an inline key is masked.
        let token = match (&provider.env_key, &provider.key) {
            (Some(env_key), _) if reveal => {
                std::env::var(env_key).map_err(|_| Error::MissingEnvVar(env_key.clone()))?
            }
            (Some(env_key), _) => format!("${env_key}"),
            (_, Some(key)) if reveal => key.clone(),
            (_, Some(_)) => "****".into(),
            (None, None) => return Err(Error::ProviderMissingAuth),
        };
        env.push(("ANTHROPIC_AUTH_TOKEN".into(), token));
        // No need to set ANTHROPIC_API_KEY: it is in MANAGED_VARS, so it is
        // already removed from claude's environment.

        // A custom provider means an unofficial (non-Anthropic) endpoint, so:
        //   - suppress claude's non-essential traffic — telemetry and similar
        //     background requests shouldn't hit a third-party gateway;
        //   - drop the attribution header — it carries no meaning off Anthropic
        //     and need not be advertised to a third-party gateway.
        env.push((
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ));
        env.push(("CLAUDE_CODE_ATTRIBUTION_HEADER".into(), "0".into()));
        // Explore caps `inherit` onto a cheaper first-party model so a pricey
        // parent isn't spent on search. That rewrite used to lose to
        // CLAUDE_CODE_SUBAGENT_MODEL; as of 2.1.251 the env is only a default
        // (https://code.claude.com/docs/en/changelog#2-1-251), so the cap wins
        // and Explore requests opus — which third-party gateways don't serve.
        // Undocumented; first shipped in 2.1.217.
        env.push(("CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP".into(), "1".into()));
    }

    // Auto-compaction.
    if let Some(pct) = profile.auto_compact_pct {
        env.push(("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into(), pct.to_string()));
    }
    if let Some(window) = profile.auto_compact_window {
        env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), window.to_string()));
    }
    if let Some(tokens) = profile.max_context_tokens {
        env.push(("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(), tokens.to_string()));
    }

    if let Some(level) = profile.effort_level {
        env.push(("CLAUDE_CODE_EFFORT_LEVEL".into(), level.as_str().into()));
    }

    Ok(env)
}

/// Compute env for preview display — placeholders only.
pub fn compute_preview_env(profile: &Profile) -> Vec<(String, String)> {
    build_env(profile, false).unwrap_or_default()
}

// MARK: - Managed variables

/// Environment variables clx manages. These are removed from the inherited
/// environment before the profile-specific values are applied, so a stale value
/// from a previous session cannot leak in. The list is intentionally exhaustive
/// (not prefix-based) so unrelated user settings like `CLAUDE_CONFIG_DIR` pass
/// through untouched.
///
/// Every key `build_env` may set must appear here (enforced by a test), plus
/// `ANTHROPIC_API_KEY`: clx never sets it, but a leftover one would override a
/// profile's `ANTHROPIC_AUTH_TOKEN` inside claude.
const MANAGED_VARS: &[&str] = &[
    "ANTHROPIC_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
    "CLAUDE_CODE_ATTRIBUTION_HEADER",
    "CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP",
    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_EFFORT_LEVEL",
];

// MARK: - launch

/// Replace the current process with `claude`, passing through extra args.
///
/// `claude` inherits clx's full environment by default (so `TERM`, `COLORTERM`,
/// `SHELL`, locale, display, etc. all pass through). We only drop the
/// clx-managed variables (see `MANAGED_VARS`) to avoid stale-config leaks, then
/// apply the profile-specific overrides on top.
///
/// Uses `.exec()` which replaces the current process on Unix — this function
/// never returns on success.
pub fn launch(profile: &Profile, extra_args: &[String]) -> Result<(), Error> {
    let mut cmd = Command::new("claude");

    // Wipe any Anthropic / Claude leftovers so the profile is authoritative,
    // then layer the profile's own values on top.
    for var in MANAGED_VARS {
        cmd.env_remove(var);
    }
    cmd.envs(build_env(profile, true)?);

    if profile.skip_permissions.unwrap_or(false) {
        cmd.arg("--dangerously-skip-permissions");
    }
    cmd.args(extra_args);

    // exec() replaces the current process — only returns on error.
    Err(Error::ExecFailed(cmd.exec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EffortLevel, Models, Provider};

    #[test]
    fn preview_env_uses_placeholder_token_and_skips_unset_models() {
        let profile = Profile {
            models: Some(Models {
                default: Some("opus".into()),
                ..Default::default()
            }),
            provider: Some(Provider {
                base_url: "https://gw".into(),
                env_key: Some("TOKEN_VAR".into()),
                key: None,
            }),
            auto_compact_pct: Some(80),
            ..Default::default()
        };
        let env = compute_preview_env(&profile);

        assert_eq!(env_value(&env, "ANTHROPIC_MODEL"), Some("opus"));
        assert_eq!(env_value(&env, "CLAUDE_CODE_SUBAGENT_MODEL"), None); // unset
        assert_eq!(env_value(&env, "ANTHROPIC_BASE_URL"), Some("https://gw"));
        // Token is a placeholder, never the real secret, in preview mode.
        assert_eq!(env_value(&env, "ANTHROPIC_AUTH_TOKEN"), Some("$TOKEN_VAR"));
        assert_eq!(
            env_value(&env, "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"),
            Some("80")
        );
        assert_eq!(env_value(&env, "CLAUDE_CODE_EFFORT_LEVEL"), None); // unset
        assert_eq!(env_value(&env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"), None); // unset
        // A custom provider is unofficial, so non-essential traffic is disabled,
        // the attribution header is dropped, and Explore's inherit cap is off.
        assert_eq!(
            env_value(&env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
            Some("1")
        );
        assert_eq!(env_value(&env, "CLAUDE_CODE_ATTRIBUTION_HEADER"), Some("0"));
        assert_eq!(
            env_value(&env, "CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP"),
            Some("1")
        );
    }

    #[test]
    fn preview_masks_inline_key() {
        let profile = Profile {
            provider: Some(Provider {
                base_url: "https://gw".into(),
                env_key: None,
                key: Some("sk-secret".into()),
            }),
            ..Default::default()
        };
        let env = compute_preview_env(&profile);
        // The literal key must never appear in preview output.
        assert_eq!(env_value(&env, "ANTHROPIC_AUTH_TOKEN"), Some("****"));
    }

    #[test]
    fn launch_env_uses_inline_key_verbatim() {
        let profile = Profile {
            provider: Some(Provider {
                base_url: "https://gw".into(),
                env_key: None,
                key: Some("sk-secret".into()),
            }),
            ..Default::default()
        };
        let env = build_env(&profile, true).unwrap();
        assert_eq!(env_value(&env, "ANTHROPIC_AUTH_TOKEN"), Some("sk-secret"));
    }

    #[test]
    fn no_provider_leaves_nonessential_traffic_untouched() {
        let profile = Profile {
            models: Some(Models {
                default: Some("opus".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let env = compute_preview_env(&profile);
        assert_eq!(
            env_value(&env, "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"),
            None
        );
        assert_eq!(env_value(&env, "CLAUDE_CODE_ATTRIBUTION_HEADER"), None);
        assert_eq!(
            env_value(&env, "CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP"),
            None
        );
    }

    #[test]
    fn managed_vars_cover_everything_build_env_sets() {
        // A profile that triggers every env var build_env can emit, so the
        // managed list can't silently drift out of sync with what we write.
        let profile = Profile {
            models: Some(Models {
                default: Some("a".into()),
                default_haiku: Some("c".into()),
                default_sonnet: Some("d".into()),
                default_opus: Some("e".into()),
                subagent: Some("b".into()),
            }),
            provider: Some(Provider {
                base_url: "u".into(),
                env_key: Some("K".into()),
                key: None,
            }),
            auto_compact_pct: Some(50),
            auto_compact_window: Some(100),
            max_context_tokens: Some(1_000_000),
            effort_level: Some(EffortLevel::High),
            ..Default::default()
        };
        for (key, _) in compute_preview_env(&profile) {
            assert!(
                MANAGED_VARS.contains(&key.as_str()),
                "build_env sets {key}, but it is missing from MANAGED_VARS"
            );
        }
    }

    fn env_value<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}
