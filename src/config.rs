use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

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
    /// What each model is, keyed by its id — the model name without the
    /// gateway prefix or a context marker. A profile refers to one by naming
    /// that model.
    #[serde(default)]
    pub models: BTreeMap<String, ModelInfo>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// The config file's directory, for prompt files given as relative paths.
    #[serde(skip)]
    pub base_dir: PathBuf,
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

/// Facts about a model that do not depend on the gateway serving it. The shape
/// follows models.dev's `limit` object, so its values can be copied across as
/// written.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    /// Effort levels the model accepts, spelled as claude's capability string
    /// spells them. Empty means the model takes no effort setting.
    #[serde(default)]
    pub effort: Vec<String>,
    #[serde(default)]
    pub limit: ModelLimit,
}

/// Token limits, named as models.dev names them.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimit {
    /// Size of the context window, in tokens.
    pub context: Option<u64>,
    /// Most tokens one response can contain.
    pub output: Option<u64>,
}

impl ModelInfo {
    /// Add the env variables claude reads these limits from.
    pub fn expand(&self, env: &mut BTreeMap<String, Template>) {
        if let Some(tokens) = self.limit.context {
            env.insert(
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS".to_owned(),
                Template::literal(tokens.to_string()),
            );
        }
        if let Some(tokens) = self.limit.output {
            env.insert(
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS".to_owned(),
                Template::literal(tokens.to_string()),
            );
        }
    }
}

/// A prompt's text: written in the config file, or loaded from one. A relative
/// path is resolved against the config file's directory.
#[derive(Debug, Clone)]
pub enum PromptSource {
    Inline(String),
    File(PathBuf),
}

impl PromptSource {
    /// The prompt text, reading the file when this source names one.
    pub fn read(&self, base_dir: &Path) -> Result<String, Error> {
        match self {
            Self::Inline(text) => Ok(text.clone()),
            Self::File(path) => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    base_dir.join(path)
                };
                std::fs::read_to_string(&path)
                    .map_err(|source| Error::PromptFile(path.clone(), source))
            }
        }
    }
}

/// Accepts the prompt text as a string, or `{ file = "path" }`.
impl<'de> Deserialize<'de> for PromptSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;

        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = PromptSource;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("prompt text, or a table with `file`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<PromptSource, E> {
                Ok(PromptSource::Inline(value.to_owned()))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PromptSource, A::Error> {
                let mut file: Option<PathBuf> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "file" if file.is_none() => file = Some(map.next_value()?),
                        "file" => return Err(de::Error::duplicate_field("file")),
                        other => return Err(de::Error::unknown_field(other, &["file"])),
                    }
                }
                let file = file.ok_or_else(|| de::Error::missing_field("file"))?;
                Ok(PromptSource::File(file))
            }
        }

        deserializer.deserialize_any(SourceVisitor)
    }
}

/// One field's prompt sources: a single text or file, or a list of them.
/// A list lets several profiles share the same file and add their own text.
#[derive(Debug, Clone, Default)]
pub struct PromptList(pub Vec<PromptSource>);

impl PromptList {
    pub fn iter(&self) -> impl Iterator<Item = &PromptSource> {
        self.0.iter()
    }
}

/// Accepts a string, a `{ file }` table, or a list of either.
impl<'de> Deserialize<'de> for PromptList {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ListVisitor;

        impl<'de> Visitor<'de> for ListVisitor {
            type Value = PromptList;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("prompt text, a `file` table, or a list of either")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<PromptList, E> {
                Ok(PromptList(vec![PromptSource::Inline(value.to_owned())]))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<PromptList, A::Error> {
                let source =
                    PromptSource::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(PromptList(vec![source]))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PromptList, A::Error> {
                let mut sources = Vec::new();
                while let Some(source) = seq.next_element::<PromptSource>()? {
                    sources.push(source);
                }
                Ok(PromptList(sources))
            }
        }

        deserializer.deserialize_any(ListVisitor)
    }
}

/// One model a profile names: the name claude receives, plus where its limits
/// come from — a `models` table id, or claude's own built-in data.
#[derive(Debug, Clone)]
pub struct ModelRef {
    pub name: Template,
    /// The `models` table entry; `None` when `builtin` is set.
    pub id: Option<String>,
    /// Use claude's built-in limits for this model.
    pub builtin: bool,
}

impl ModelRef {
    /// Build a ref, requiring exactly one source of model data.
    fn new<E: de::Error>(name: Template, id: Option<String>, builtin: bool) -> Result<Self, E> {
        match (id.is_some(), builtin) {
            (true, true) => Err(E::custom("use either `id` or `builtin`, not both")),
            (false, false) => Err(E::custom(
                "model needs `id`, or `builtin = true` for a Claude model",
            )),
            _ => {
                if id.is_some() && name.as_str().to_ascii_lowercase().contains("[1m]") {
                    return Err(E::custom(
                        "a model with `id` takes its window from models.toml; \
                         remove the `[1m]` suffix",
                    ));
                }
                Ok(Self { name, id, builtin })
            }
        }
    }

    /// The model variables. One model fills all of them, so `/model` aliases
    /// and subagents stay on the same gateway model. A different model for a
    /// subagent belongs in an [`AgentDef`].
    pub fn expand(&self) -> BTreeMap<String, Template> {
        [
            ("ANTHROPIC_MODEL", &self.name),
            ("ANTHROPIC_DEFAULT_FABLE_MODEL", &self.name),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", &self.name),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", &self.name),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", &self.name),
            ("CLAUDE_CODE_SUBAGENT_MODEL", &self.name),
        ]
        .into_iter()
        .map(|(key, template)| (key.to_owned(), template.clone()))
        .collect()
    }

    /// The `CLAUDE_CODE_MODEL_CAPABILITIES` entry for this model, when its
    /// `models` entry lists effort levels.
    pub fn capabilities(&self, models: &BTreeMap<String, ModelInfo>) -> Option<String> {
        let id = self.id.as_ref()?;
        let info = models.get(id)?;
        if info.effort.is_empty() {
            return None;
        }
        Some(format!("{}={}", self.name.as_str(), info.effort.join(",")))
    }
}

/// Accepts only a table: `{ name, id }` for a `models` entry, or
/// `{ name, builtin = true }` for a Claude model claude already knows.
impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RefVisitor;

        impl<'de> Visitor<'de> for RefVisitor {
            type Value = ModelRef;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a table with `name` and either `id` or `builtin = true`")
            }

            fn visit_str<E: de::Error>(self, _value: &str) -> Result<ModelRef, E> {
                Err(E::custom(
                    "write the model as a table: { name = \"...\", id = \"...\" } \
                     or { name = \"...\", builtin = true }",
                ))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ModelRef, A::Error> {
                let mut name: Option<Template> = None;
                let mut id: Option<String> = None;
                let mut builtin: Option<bool> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" if name.is_none() => name = Some(map.next_value()?),
                        "id" if id.is_none() => id = Some(map.next_value()?),
                        "builtin" if builtin.is_none() => builtin = Some(map.next_value()?),
                        "name" => return Err(de::Error::duplicate_field("name")),
                        "id" => return Err(de::Error::duplicate_field("id")),
                        "builtin" => return Err(de::Error::duplicate_field("builtin")),
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["name", "id", "builtin"],
                            ));
                        }
                    }
                }
                let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
                ModelRef::new(name, id, builtin.unwrap_or(false))
            }
        }

        deserializer.deserialize_any(RefVisitor)
    }
}

/// A subagent defined in a profile, passed to claude through `--agents`.
/// `name` is the object key; `prompt` and `model` get clx treatment; every
/// other key is passed through as written.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentDef {
    pub name: String,
    #[serde(default)]
    pub prompt: Option<PromptSource>,
    #[serde(default)]
    pub model: Option<AgentModel>,
    #[serde(flatten)]
    pub fields: BTreeMap<String, toml::Value>,
}

/// An agent's model: a name claude understands (`sonnet`, `inherit`, a full
/// id), or a `ModelRef` whose `name` is used and whose `id` is checked against
/// the `models` table.
#[derive(Debug, Clone)]
pub enum AgentModel {
    Plain(String),
    Ref(ModelRef),
}

impl AgentModel {
    pub fn name(&self) -> &str {
        match self {
            Self::Plain(name) => name,
            Self::Ref(model) => model.name.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for AgentModel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ModelVisitor;

        impl<'de> Visitor<'de> for ModelVisitor {
            type Value = AgentModel;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "a model name, or a table with `name` and either `id` or `builtin = true`",
                )
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<AgentModel, E> {
                Ok(AgentModel::Plain(value.to_owned()))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<AgentModel, A::Error> {
                ModelRef::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(AgentModel::Ref)
            }
        }

        deserializer.deserialize_any(ModelVisitor)
    }
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
    /// The model claude runs: it fills every model variable. A per-agent model
    /// belongs in an [`AgentDef`] instead.
    pub model: Option<ModelRef>,
    /// Text appended to claude's system prompt. Every text on the `extends`
    /// chain is kept, root first, joined with a blank line.
    pub append_system_prompt: Option<PromptList>,
    /// Environment variables to launch with, keyed by the exact variable name.
    #[serde(default, deserialize_with = "crate::env::de_env_map")]
    pub env: BTreeMap<String, Template>,
    /// Arguments for claude; merge rules live in [`crate::args`].
    #[serde(default)]
    pub args: Vec<String>,
    /// Subagents passed to claude through `--agents`; a child's agent with the
    /// same name replaces the parent's.
    #[serde(default)]
    pub agents: Vec<AgentDef>,
}

impl Config {
    /// Load and validate `~/.config/clx/config.toml`, then merge `models.toml`
    /// and every `conf.d/*.toml` in file-name order.
    pub fn load() -> Result<Self, Error> {
        Self::load_from(&config_path()?)
    }

    /// Load from an explicit config path. The directory holding it supplies
    /// `models.toml`, `conf.d/`, and the base for relative prompt paths.
    fn load_from(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::ConfigNotFound(
                path.display().to_string(),
                EXAMPLE_CONFIG.to_string(),
            ));
        }

        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut config = Self::load_file(path)?;

        let models = dir.join("models.toml");
        if !models.exists() {
            return Err(Error::ModelsNotFound(models.display().to_string()));
        }
        config.merge(Self::load_file(&models)?, &models)?;

        let conf_d = dir.join("conf.d");
        if conf_d.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&conf_d)
                .map_err(|e| Error::ConfigRead(conf_d.display().to_string(), e))?
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|file| file.extension().is_some_and(|ext| ext == "toml"))
                .collect();
            files.sort();
            for file in files {
                config.merge(Self::load_file(&file)?, &file)?;
            }
        }

        config.base_dir = dir;

        if config.profiles.is_empty() {
            return Err(Error::ConfigInvalid(
                path.display().to_string(),
                "no profiles defined".into(),
            ));
        }

        config.validate_models()?;

        Ok(config)
    }

    /// Read and parse one config file.
    fn load_file(path: &Path) -> Result<Self, Error> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::ConfigRead(path.display().to_string(), e))?;
        toml::from_str(&content).map_err(|e| {
            Error::ConfigParse(path.display().to_string(), suggest::format_toml_error(e))
        })
    }

    /// Fold a later file in: env and models merge per key, args append, and a
    /// profile name may appear only once across every file.
    fn merge(&mut self, other: Self, source: &Path) -> Result<(), Error> {
        self.global.env.extend(other.global.env);
        self.global.args.extend(other.global.args);
        self.models.extend(other.models);
        for profile in other.profiles {
            if self.profiles.iter().any(|p| p.name == profile.name) {
                return Err(Error::DuplicateProfile(
                    profile.name,
                    source.display().to_string(),
                ));
            }
            self.profiles.push(profile);
        }
        Ok(())
    }

    /// A model either uses claude's built-in data or names a `models` entry;
    /// a named entry has to exist.
    fn validate_models(&self) -> Result<(), Error> {
        for profile in &self.profiles {
            if let Some(model) = &profile.model {
                self.check_model(profile, model)?;
            }
            for agent in &profile.agents {
                if let Some(AgentModel::Ref(model_ref)) = &agent.model {
                    self.check_model(profile, model_ref)?;
                }
            }
        }
        Ok(())
    }

    fn check_model(&self, profile: &Profile, model_ref: &ModelRef) -> Result<(), Error> {
        if model_ref.builtin {
            return Ok(());
        }
        let Some(id) = &model_ref.id else {
            return Ok(());
        };
        if self.models.contains_key(id) {
            return Ok(());
        }
        Err(Error::UnknownModel(profile.name.clone(), id.clone()))
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

const EXAMPLE_CONFIG: &str = r##"# Create an empty models.toml next to this file; it is required.
# A third-party model also needs a [models.<id>] entry with its limits.

[global]
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
    fn models_table_parses_limits() {
        let config = parse(
            "[models.claude-opus-5-5]\n\
             effort = [\"effort\", \"xhigh_effort\"]\n\
             limit = { context = 1000000, output = 128000 }\n\
             \n[[profiles]]\nname = \"p\"\n",
        )
        .unwrap();
        let info = &config.models["claude-opus-5-5"];
        assert_eq!(info.effort, ["effort", "xhigh_effort"]);
        assert_eq!(info.limit.context, Some(1_000_000));
        assert_eq!(info.limit.output, Some(128_000));
    }

    #[test]
    fn agents_take_prompt_model_and_passthrough_fields() {
        let config = parse(
            "[[profiles]]\nname = \"p\"\n\
             [[profiles.agents]]\nname = \"worker\"\n\
             description = \"does work\"\n\
             tools = [\"Read\", \"Bash\"]\n\
             permissionMode = \"acceptEdits\"\n\
             prompt = \"do it\"\n\
             model = \"sonnet\"\n",
        )
        .unwrap();
        let agent = &config.profiles[0].agents[0];
        assert_eq!(agent.name, "worker");
        assert!(matches!(&agent.prompt, Some(PromptSource::Inline(text)) if text == "do it"));
        assert!(matches!(&agent.model, Some(AgentModel::Plain(name)) if name == "sonnet"));
        assert_eq!(agent.fields["description"].as_str(), Some("does work"));
        assert_eq!(agent.fields["tools"].as_array().unwrap().len(), 2);
        assert_eq!(agent.fields["permissionMode"].as_str(), Some("acceptEdits"));
        assert!(!agent.fields.contains_key("prompt"));
        assert!(!agent.fields.contains_key("model"));
    }

    #[test]
    fn prompt_source_accepts_text_or_a_file() {
        let text = parse("[[profiles]]\nname = \"p\"\nappend_system_prompt = \"text\"\n").unwrap();
        assert!(matches!(
            text.profiles[0]
                .append_system_prompt
                .as_ref()
                .unwrap()
                .0
                .as_slice(),
            [PromptSource::Inline(value)] if value == "text"
        ));

        let file =
            parse("[[profiles]]\nname = \"p\"\nappend_system_prompt = { file = \"text.md\" }\n")
                .unwrap();
        assert!(matches!(
            file.profiles[0]
                .append_system_prompt
                .as_ref()
                .unwrap()
                .0
                .as_slice(),
            [PromptSource::File(path)] if path == Path::new("text.md")
        ));
    }

    #[test]
    fn prompt_list_accepts_several_sources() {
        let config = parse(
            "[[profiles]]\nname = \"p\"\n\
             append_system_prompt = [{ file = \"a.md\" }, \"inline\"]\n",
        )
        .unwrap();
        let list = &config.profiles[0].append_system_prompt.as_ref().unwrap().0;
        assert_eq!(list.len(), 2);
        assert!(matches!(&list[0], PromptSource::File(path) if path == Path::new("a.md")));
        assert!(matches!(&list[1], PromptSource::Inline(text) if text == "inline"));
    }

    #[test]
    fn models_and_conf_d_files_merge_in_order() {
        let dir = std::env::temp_dir().join(format!("clx-config-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(dir.join("config.toml"), "[[profiles]]\nname = \"a\"\n").unwrap();
        std::fs::write(
            dir.join("models.toml"),
            "[models.x]\nlimit = { context = 1000 }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("conf.d/10-b.toml"),
            "[[profiles]]\nname = \"b\"\n[global]\nenv.K = \"v\"\n",
        )
        .unwrap();

        let config = Config::load_from(&dir.join("config.toml")).unwrap();
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.models["x"].limit.context, Some(1000));
        assert_eq!(config.global.env["K"].as_str(), "v");
        assert_eq!(config.base_dir, dir);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_profile_name_may_appear_only_once_across_files() {
        let dir = std::env::temp_dir().join(format!("clx-dup-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        std::fs::write(dir.join("config.toml"), "[[profiles]]\nname = \"a\"\n").unwrap();
        std::fs::write(dir.join("models.toml"), "").unwrap();
        std::fs::write(dir.join("conf.d/10-a.toml"), "[[profiles]]\nname = \"a\"\n").unwrap();

        let err = Config::load_from(&dir.join("config.toml")).unwrap_err();
        assert!(matches!(err, Error::DuplicateProfile(name, _) if name == "a"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_models_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("clx-no-models-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "[[profiles]]\nname = \"a\"\n").unwrap();

        let err = Config::load_from(&dir.join("config.toml")).unwrap_err();
        assert!(matches!(err, Error::ModelsNotFound(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_named_model_id_must_exist_in_the_models_table() {
        let dir = std::env::temp_dir().join(format!("clx-unknown-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[[profiles]]\nname = \"a\"\nmodel = { name = \"x\", id = \"nope\" }\n",
        )
        .unwrap();
        std::fs::write(dir.join("models.toml"), "").unwrap();

        let err = Config::load_from(&dir.join("config.toml")).unwrap_err();
        assert!(matches!(err, Error::UnknownModel(profile, id) if profile == "a" && id == "nope"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_agent_model_id_must_exist_in_the_models_table() {
        let dir = std::env::temp_dir().join(format!("clx-agent-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[[profiles]]\nname = \"a\"\nmodel = { name = \"x\", builtin = true }\n\
             [[profiles.agents]]\nname = \"worker\"\nmodel = { name = \"y\", id = \"nope\" }\n",
        )
        .unwrap();
        std::fs::write(dir.join("models.toml"), "").unwrap();

        let err = Config::load_from(&dir.join("config.toml")).unwrap_err();
        assert!(matches!(err, Error::UnknownModel(profile, id) if profile == "a" && id == "nope"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_builtin_model_needs_no_entry() {
        let dir = std::env::temp_dir().join(format!("clx-plain-model-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "[[profiles]]\nname = \"a\"\nmodel = { name = \"opus[1m]\", builtin = true }\n",
        )
        .unwrap();
        std::fs::write(dir.join("models.toml"), "").unwrap();

        let config = Config::load_from(&dir.join("config.toml")).unwrap();
        assert_eq!(config.profiles.len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn model_expands_into_every_model_variable() {
        let config =
            parse("[[profiles]]\nname = \"p\"\nmodel = { name = \"one\", builtin = true }\n")
                .unwrap();
        let env = config.profiles[0].model.as_ref().unwrap().expand();
        for key in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "CLAUDE_CODE_SUBAGENT_MODEL",
        ] {
            assert_eq!(env[key].as_str(), "one", "{key}");
        }
    }

    #[test]
    fn model_rejects_typos_and_missing_source() {
        let err = parse("[[profiles]]\nname = \"p\"\nmodel = { nmae = \"x\" }\n").unwrap_err();
        assert!(
            suggest::format_toml_error(err).contains("did you mean `name`?"),
            "typo in a model table gets a hint"
        );

        for body in [
            "model = 42",
            "model = \"plain string\"",
            "model = { name = \"x\" }",
            "model = { name = \"x\", id = \"a\", builtin = true }",
            "model = { name = \"x[1m]\", id = \"a\" }",
            "model = { name = \"x\", extra = \"c\" }",
            "model.main = { name = \"x\", builtin = true }",
        ] {
            assert!(
                parse(&format!("[[profiles]]\nname = \"p\"\n{body}\n")).is_err(),
                "accepted {body}"
            );
        }
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
