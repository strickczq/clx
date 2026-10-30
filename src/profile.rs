use std::collections::BTreeMap;
use std::path::Path;

use crate::args::ArgList;
use crate::config::{AgentDef, Config, ModelInfo, Profile};
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
    /// `append_system_prompt` texts from the whole chain, root first. Unlike
    /// env and args, these accumulate: a child adds to its parents' prompts.
    pub append_system_prompt: Vec<String>,
    /// The `--agents` JSON, merged across the chain by agent name.
    pub agents: Option<String>,
    /// The agent names in that JSON, for the picker preview.
    pub agent_names: Vec<String>,
}

impl Resolved {
    /// Fold one generation's values in: env per key, args per flag.
    fn overlay(&mut self, env: &BTreeMap<String, Template>, args: &[String]) {
        self.env
            .extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.args.merge(args);
    }
}

impl Profile {
    /// This generation's env with `model` expanded underneath, so a key the
    /// profile writes itself wins over the shorthand and the model limits.
    fn layer_env(&self, models: &BTreeMap<String, ModelInfo>) -> BTreeMap<String, Template> {
        let mut env = BTreeMap::new();
        if let Some(model) = &self.model {
            env.extend(model.expand());
            if let Some(id) = &model.id
                && let Some(info) = models.get(id)
            {
                info.expand(&mut env);
            }
            if let Some(capabilities) = model.capabilities(models) {
                env.insert(
                    "CLAUDE_CODE_MODEL_CAPABILITIES".to_owned(),
                    Template::literal(capabilities),
                );
            }
        }
        env.extend(self.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
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
        let mut agents: BTreeMap<String, AgentDef> = BTreeMap::new();
        resolved.overlay(&self.global.env, &self.global.args);
        for entry in chain.iter().rev() {
            resolved.overlay(&entry.layer_env(&self.models), &entry.args);
            if let Some(prompts) = &entry.append_system_prompt {
                for prompt in prompts.iter() {
                    resolved
                        .append_system_prompt
                        .push(prompt.read(&self.base_dir)?);
                }
            }
            for agent in &entry.agents {
                agents.insert(agent.name.clone(), agent.clone());
            }
        }
        resolved.agent_names = agents.keys().cloned().collect();
        resolved.agents = agents_json(&agents, &self.base_dir)?;
        Ok(resolved)
    }
}

/// Build the `--agents` JSON for the merged agent set. `prompt` is read from
/// its source, `model` becomes the name claude receives, and every other key
/// is copied verbatim.
fn agents_json(
    agents: &BTreeMap<String, AgentDef>,
    base_dir: &Path,
) -> Result<Option<String>, Error> {
    if agents.is_empty() {
        return Ok(None);
    }
    let mut object = serde_json::Map::new();
    for agent in agents.values() {
        let mut definition = serde_json::Map::new();
        if let Some(prompt) = &agent.prompt {
            definition.insert(
                "prompt".to_owned(),
                serde_json::Value::String(prompt.read(base_dir)?),
            );
        }
        if let Some(model) = &agent.model {
            definition.insert(
                "model".to_owned(),
                serde_json::Value::String(model.name().to_owned()),
            );
        }
        for (key, value) in &agent.fields {
            let value = serde_json::to_value(value).map_err(Error::AgentJson)?;
            definition.insert(key.clone(), value);
        }
        object.insert(agent.name.clone(), serde_json::Value::Object(definition));
    }
    serde_json::to_string(&object)
        .map(Some)
        .map_err(Error::AgentJson)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentModel, ModelLimit, ModelRef, PromptList, PromptSource};

    fn cfg(profiles: Vec<Profile>) -> Config {
        Config {
            profiles,
            ..Default::default()
        }
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            ..Default::default()
        }
    }

    fn model(name: &str) -> ModelRef {
        ModelRef {
            name: Template::parse(name).unwrap(),
            id: None,
            builtin: true,
        }
    }

    fn linked(name: &str, id: &str) -> ModelRef {
        ModelRef {
            name: Template::parse(name).unwrap(),
            id: Some(id.into()),
            builtin: false,
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
    fn model_shorthand_expands_and_env_wins() {
        let p = Profile {
            model: Some(model("m")),
            env: map(&[("ANTHROPIC_DEFAULT_SONNET_MODEL", "override")]),
            ..profile("p")
        };
        let resolved = cfg(vec![p]).resolve("p").unwrap();
        assert_eq!(value(&resolved.env, "ANTHROPIC_MODEL"), "m");
        assert_eq!(value(&resolved.env, "CLAUDE_CODE_SUBAGENT_MODEL"), "m");
        assert_eq!(
            value(&resolved.env, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
            "override"
        );
    }

    #[test]
    fn a_child_model_replaces_the_parent_shorthand() {
        let base = Profile {
            model: Some(model("base")),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            model: Some(model("big")),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        assert_eq!(value(&resolved.env, "ANTHROPIC_MODEL"), "big");
        assert_eq!(value(&resolved.env, "ANTHROPIC_DEFAULT_OPUS_MODEL"), "big");
        assert_eq!(
            value(&resolved.env, "ANTHROPIC_DEFAULT_SONNET_MODEL"),
            "big"
        );
    }

    #[test]
    fn a_child_without_model_inherits_the_parent_expansion() {
        let base = Profile {
            model: Some(model("base")),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        assert_eq!(
            value(&resolved.env, "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            "base"
        );
    }

    #[test]
    fn model_limits_come_from_the_shared_models_table() {
        let mut config = cfg(vec![
            Profile {
                model: Some(linked("example/large-model[1m]", "large-model")),
                ..profile("gateway-a")
            },
            Profile {
                model: Some(linked("other-gateway/large-model[1m]", "large-model")),
                ..profile("gateway-b")
            },
        ]);
        config.models.insert(
            "large-model".into(),
            ModelInfo {
                limit: ModelLimit {
                    context: Some(1_000_000),
                    output: Some(128_000),
                },
                ..Default::default()
            },
        );

        for name in ["gateway-a", "gateway-b"] {
            let resolved = config.resolve(name).unwrap();
            assert_eq!(
                value(&resolved.env, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
                "1000000"
            );
            assert_eq!(
                value(&resolved.env, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"),
                "128000"
            );
        }
    }

    #[test]
    fn capabilities_come_from_the_models_entry() {
        let mut config = cfg(vec![Profile {
            model: Some(linked("example/large-model", "large-model")),
            ..profile("p")
        }]);
        config.models.insert(
            "large-model".into(),
            ModelInfo {
                effort: vec!["effort".into(), "xhigh_effort".into(), "max_effort".into()],
                ..Default::default()
            },
        );

        let resolved = config.resolve("p").unwrap();
        assert_eq!(
            value(&resolved.env, "CLAUDE_CODE_MODEL_CAPABILITIES"),
            "example/large-model=effort,xhigh_effort,max_effort"
        );
    }

    #[test]
    fn capabilities_skip_empty_effort_and_dedupe_one_model() {
        let mut config = cfg(vec![
            Profile {
                model: Some(linked("example/model-a@500k", "model-a")),
                ..profile("gateway-a")
            },
            Profile {
                model: Some(linked("plain", "no-effort")),
                ..profile("plain")
            },
        ]);
        config.models.insert(
            "model-a".into(),
            ModelInfo {
                effort: vec!["effort".into(), "xhigh_effort".into()],
                ..Default::default()
            },
        );
        config
            .models
            .insert("no-effort".into(), ModelInfo::default());

        let resolved = config.resolve("gateway-a").unwrap();
        assert_eq!(
            value(&resolved.env, "CLAUDE_CODE_MODEL_CAPABILITIES"),
            "example/model-a@500k=effort,xhigh_effort"
        );
        assert!(
            !config
                .resolve("plain")
                .unwrap()
                .env
                .contains_key("CLAUDE_CODE_MODEL_CAPABILITIES")
        );
    }

    #[test]
    fn profile_env_overrides_generated_capabilities() {
        let mut config = cfg(vec![Profile {
            model: Some(linked("m", "m")),
            env: map(&[("CLAUDE_CODE_MODEL_CAPABILITIES", "manual")]),
            ..profile("p")
        }]);
        config.models.insert(
            "m".into(),
            ModelInfo {
                effort: vec!["effort".into()],
                ..Default::default()
            },
        );
        let resolved = config.resolve("p").unwrap();
        assert_eq!(
            value(&resolved.env, "CLAUDE_CODE_MODEL_CAPABILITIES"),
            "manual"
        );
    }

    #[test]
    fn profile_env_overrides_model_limits() {
        let mut config = cfg(vec![Profile {
            model: Some(linked("m", "m")),
            env: map(&[("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "4096")]),
            ..profile("p")
        }]);
        config.models.insert(
            "m".into(),
            ModelInfo {
                limit: ModelLimit {
                    context: None,
                    output: Some(128_000),
                },
                ..Default::default()
            },
        );
        let resolved = config.resolve("p").unwrap();
        assert_eq!(
            value(&resolved.env, "CLAUDE_CODE_MAX_OUTPUT_TOKENS"),
            "4096"
        );
        assert!(!resolved.env.contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));
    }

    #[test]
    fn a_model_without_id_gets_no_limits() {
        let mut config = cfg(vec![Profile {
            model: Some(model("example/large-model[1m]")),
            ..profile("p")
        }]);
        config.models.insert(
            "large-model".into(),
            ModelInfo {
                limit: ModelLimit {
                    context: Some(1_000_000),
                    output: None,
                },
                ..Default::default()
            },
        );
        let resolved = config.resolve("p").unwrap();
        assert!(!resolved.env.contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));
    }

    #[test]
    fn agents_merge_by_name_and_build_json() {
        let base = Profile {
            agents: vec![AgentDef {
                name: "worker".into(),
                prompt: Some(PromptSource::Inline("base".into())),
                model: Some(AgentModel::Plain("haiku".into())),
                fields: [("description".to_owned(), toml::Value::String("base".into()))]
                    .into_iter()
                    .collect(),
            }],
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            agents: vec![AgentDef {
                name: "worker".into(),
                prompt: Some(PromptSource::Inline("child".into())),
                model: None,
                fields: [(
                    "description".to_owned(),
                    toml::Value::String("child".into()),
                )]
                .into_iter()
                .collect(),
            }],
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        let json: serde_json::Value =
            serde_json::from_str(resolved.agents.as_ref().unwrap()).unwrap();
        assert_eq!(json["worker"]["description"], "child");
        assert_eq!(json["worker"]["prompt"], "child");
        assert!(json["worker"].get("model").is_none());
    }

    #[test]
    fn no_agents_means_no_flag() {
        let resolved = cfg(vec![profile("p")]).resolve("p").unwrap();
        assert!(resolved.agents.is_none());
    }

    #[test]
    fn append_prompts_accumulate_root_first() {
        let base = Profile {
            append_system_prompt: Some(PromptList(vec![PromptSource::Inline(
                "base prompt".into(),
            )])),
            ..profile("base")
        };
        let child = Profile {
            extends: Some("base".into()),
            append_system_prompt: Some(PromptList(vec![PromptSource::Inline(
                "child prompt".into(),
            )])),
            ..profile("child")
        };
        let resolved = cfg(vec![base, child]).resolve("child").unwrap();
        assert_eq!(
            resolved.append_system_prompt,
            ["base prompt", "child prompt"]
        );
    }

    #[test]
    fn a_prompt_file_is_read_relative_to_the_config_dir() {
        let dir = std::env::temp_dir().join(format!("clx-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("guard.md"), "file text").unwrap();

        let mut config = cfg(vec![Profile {
            append_system_prompt: Some(PromptList(vec![PromptSource::File("guard.md".into())])),
            ..profile("p")
        }]);
        config.base_dir = dir.clone();
        let resolved = config.resolve("p").unwrap();
        assert_eq!(resolved.append_system_prompt, ["file text"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_prompt_file_is_an_error() {
        let mut config = cfg(vec![Profile {
            append_system_prompt: Some(PromptList(vec![PromptSource::File("missing.md".into())])),
            ..profile("p")
        }]);
        config.base_dir = std::env::temp_dir().join("clx-dir-that-does-not-exist");
        assert!(matches!(
            config.resolve("p").unwrap_err(),
            Error::PromptFile(..)
        ));
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
