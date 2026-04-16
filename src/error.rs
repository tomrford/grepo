use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, GrepoError>;

#[derive(Debug, Error)]
pub enum GrepoError {
    #[error(transparent)]
    Cli(#[from] clap::Error),

    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Command(String),

    #[error("invalid grepo/.lock TOML: {0}")]
    LockParse(#[from] toml::de::Error),

    #[error("failed to serialize grepo/.lock: {0}")]
    LockSerialize(#[from] toml::ser::Error),

    #[error("invalid alias: {0}")]
    InvalidAlias(String),

    #[error("invalid alias in grepo/.lock: {0}")]
    InvalidLockAlias(String),

    #[error("cannot initialize grepo root because {0} is not a directory")]
    RootPathNotDirectory(PathBuf),

    #[error("no grepo root found from {0}")]
    NoProjectRoot(PathBuf),

    #[error("alias not found: {0}")]
    AliasNotFound(String),

    #[error("alias already exists: {0} (use --force to replace)")]
    AliasExists(String),

    #[error("alias {0} has no commit")]
    MissingCommit(String),

    #[error("path collision at {0}: expected a symlink managed by grepo")]
    PathCollision(PathBuf),

    #[error("another grepo command is already mutating {0}")]
    Busy(PathBuf),
}
