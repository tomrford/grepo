use thiserror::Error;

use crate::app::AppError;
use crate::git::GitError;
use crate::manifest::ManifestError;
use crate::mutation_lock::MutationLockError;
use crate::store::StoreError;
use crate::util::UtilError;

pub type Result<T> = std::result::Result<T, GrepoError>;

#[derive(Debug, Error)]
pub enum GrepoError {
    #[error(transparent)]
    Cli(#[from] clap::Error),

    #[error(transparent)]
    App(#[from] AppError),

    #[error(transparent)]
    Git(#[from] GitError),

    #[error(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    MutationLock(#[from] MutationLockError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    Util(#[from] UtilError),
}
