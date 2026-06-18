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
            ("ANTHROPIC_SMALL_FAST_MODEL", &models.small_fast),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &models.default_haiku),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", &models.default_sonnet),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", &models.default_opus),
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
        let token = if reveal {
            std::env::var(&provider.env_key)
                .map_err(|_| Error::MissingEnvVar(provider.env_key.clone()))?
        } else {
            format!("${}", provider.env_key)
        };
        env.push(("ANTHROPIC_AUTH_TOKEN".into(), token));
        // No need to set ANTHROPIC_API_KEY: it is in MANAGED_VARS, so it is
        // already removed from claude's environment.
    }

    // Auto-compaction.
    if let Some(pct) = profile.auto_compact_pct {
        env.push(("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE".into(), pct.to_string()));
    }
    if let Some(window) = profile.auto_compact_window {
        env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), window.to_string()));
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
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
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
    use crate::config::{Models, Provider};

    #[test]
    fn preview_env_uses_placeholder_token_and_skips_unset_models() {
        let profile = Profile {
            models: Some(Models {
                default: Some("opus".into()),
                ..Default::default()
            }),
            provider: Some(Provider {
                base_url: "https://gw".into(),
                env_key: "TOKEN_VAR".into(),
            }),
            auto_compact_pct: Some(80),
            ..Default::default()
        };
        let env = compute_preview_env(&profile);

        assert_eq!(env_value(&env, "ANTHROPIC_MODEL"), Some("opus"));
        assert_eq!(env_value(&env, "ANTHROPIC_SMALL_FAST_MODEL"), None); // unset
        assert_eq!(env_value(&env, "ANTHROPIC_BASE_URL"), Some("https://gw"));
        // Token is a placeholder, never the real secret, in preview mode.
        assert_eq!(env_value(&env, "ANTHROPIC_AUTH_TOKEN"), Some("$TOKEN_VAR"));
        assert_eq!(
            env_value(&env, "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"),
            Some("80")
        );
    }

    #[test]
    fn managed_vars_cover_everything_build_env_sets() {
        // A profile that triggers every env var build_env can emit, so the
        // managed list can't silently drift out of sync with what we write.
        let profile = Profile {
            models: Some(Models {
                default: Some("a".into()),
                small_fast: Some("b".into()),
                default_haiku: Some("c".into()),
                default_sonnet: Some("d".into()),
                default_opus: Some("e".into()),
            }),
            provider: Some(Provider {
                base_url: "u".into(),
                env_key: "K".into(),
            }),
            auto_compact_pct: Some(50),
            auto_compact_window: Some(100),
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
