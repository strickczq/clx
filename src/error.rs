use std::path::PathBuf;

use thiserror::Error;

/// All fatal errors clx can surface. `main` prints these as `error: <message>`
/// and exits with code 1.
#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot determine config directory: set $HOME or $XDG_CONFIG_HOME")]
    NoConfigDir,
    #[error("no config found at {0}\ncreate it with at least one profile, for example:\n\n{1}")]
    ConfigNotFound(String, String),
    #[error("cannot read {0}: {1}")]
    ConfigRead(String, std::io::Error),
    #[error("invalid toml in {0}: {1}")]
    ConfigParse(String, String),
    #[error("invalid config in {0}: {1}")]
    ConfigInvalid(String, String),
    #[error("profile \"{0}\" is defined more than once (again in {1})")]
    DuplicateProfile(String, String),
    #[error(
        "no models file at {0}\nthird-party models need their limits; create it \
         with a [models.<id>] entry per model"
    )]
    ModelsNotFound(String),
    #[error("profile \"{0}\" uses model id \"{1}\", which models.toml does not define")]
    UnknownModel(String, String),
    #[error("cannot build the --agents JSON: {0}")]
    AgentJson(serde_json::Error),
    #[error("unknown profile \"{0}\"{1}")]
    ProfileNotFound(String, String),
    #[error("circular extends: {0}")]
    CircularExtends(String),
    #[error("profile \"{0}\" extends unknown profile \"{1}\"")]
    UnknownParent(String, String),
    #[error("env.{key}: environment variable {var} is not set")]
    MissingEnvVar { key: String, var: String },
    #[error("env.{key}: environment variable {var} is set but not valid Unicode")]
    NotUnicodeEnvVar { key: String, var: String },
    #[error("{0} is set by clx from the profile; remove it from args")]
    ReservedFlag(String),
    #[error("cannot read prompt file {0}: {1}")]
    PromptFile(PathBuf, std::io::Error),
    #[error("failed to launch claude: {0}")]
    ExecFailed(std::io::Error),
    #[error("failed to run fzf: {0} — is fzf installed?")]
    Fzf(std::io::Error),
}
