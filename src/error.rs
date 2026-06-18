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
    ConfigParse(String, toml::de::Error),
    #[error("invalid config in {0}: {1}")]
    ConfigInvalid(String, String),
    #[error("unknown profile \"{0}\"{1}")]
    ProfileNotFound(String, String),
    #[error("circular extends: {0}")]
    CircularExtends(String),
    #[error("profile \"{0}\" extends unknown profile \"{1}\"")]
    UnknownParent(String, String),
    #[error("environment variable {0} is not set")]
    MissingEnvVar(String),
    #[error("failed to launch claude: {0}")]
    ExecFailed(std::io::Error),
    #[error("failed to run fzf: {0} — is fzf installed?")]
    Fzf(std::io::Error),
}
