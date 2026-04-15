use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MutationLockError {
    #[error("failed to open mutation lock {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("another grepo command is already mutating {path}")]
    Busy { path: PathBuf },

    #[error("failed to lock mutation file {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to update mutation lock {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to unlock mutation lock {path}: {source}")]
    Unlock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) struct MutationLock {
    path: PathBuf,
    file: fs::File,
}

impl MutationLock {
    pub(crate) fn acquire(grepo_dir: &Path) -> Result<Self, MutationLockError> {
        let path = grepo_dir.join(".mutate.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| MutationLockError::Open {
                path: path.clone(),
                source,
            })?;

        match file.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => {
                return Err(MutationLockError::Busy {
                    path: grepo_dir.to_path_buf(),
                });
            }
            Err(source) if source.kind() == ErrorKind::WouldBlock => {
                return Err(MutationLockError::Busy {
                    path: grepo_dir.to_path_buf(),
                });
            }
            Err(source) => {
                return Err(MutationLockError::Lock {
                    path: path.clone(),
                    source,
                });
            }
        }

        file.set_len(0).map_err(|source| MutationLockError::Write {
            path: path.clone(),
            source,
        })?;
        file.seek(SeekFrom::Start(0))
            .map_err(|source| MutationLockError::Write {
                path: path.clone(),
                source,
            })?;
        writeln!(file, "{}", std::process::id()).map_err(|source| MutationLockError::Write {
            path: path.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| MutationLockError::Write {
            path: path.clone(),
            source,
        })?;

        Ok(Self { path, file })
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = self
            .file
            .unlock()
            .map_err(|source| MutationLockError::Unlock {
                path: self.path.clone(),
                source,
            });
    }
}
