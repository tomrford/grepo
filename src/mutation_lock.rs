use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::Path;

use fs4::fs_std::FileExt;

use crate::error::{GrepoError, Result};

pub(crate) struct FileLock {
    file: fs::File,
}

pub(crate) struct MutationLock {
    _lock: FileLock,
}

impl FileLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        let file = open_lock_file(path)?;
        file.lock_exclusive()
            .map_err(|e| GrepoError::Io(format!("failed to lock {}: {e}", path.display())))?;
        Ok(Self { file })
    }

    pub(crate) fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = open_lock_file(path)?;
        match file.try_lock_exclusive() {
            Ok(true) => Ok(Some(Self { file })),
            Ok(false) => Ok(None),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(GrepoError::Io(format!(
                "failed to lock {}: {e}",
                path.display()
            ))),
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Best-effort; there is no meaningful recovery path in Drop.
        let _ = self.file.unlock();
    }
}

impl MutationLock {
    pub(crate) fn acquire(grepo_dir: &Path) -> Result<Self> {
        let path = grepo_dir.join(".mutate.lock");
        let lock = FileLock::try_acquire(&path)?;
        let Some(lock) = lock else {
            return Err(GrepoError::Busy(grepo_dir.to_path_buf()));
        };
        Ok(Self { _lock: lock })
    }
}

fn open_lock_file(path: &Path) -> Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| GrepoError::Io(format!("failed to open lock file {}: {e}", path.display())))
}
