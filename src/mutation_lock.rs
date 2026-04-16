use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
use std::path::Path;

use fs4::fs_std::FileExt;

use crate::error::{GrepoError, Result};

pub(crate) struct MutationLock {
    file: fs::File,
}

impl MutationLock {
    pub(crate) fn acquire(grepo_dir: &Path) -> Result<Self> {
        let path = grepo_dir.join(".mutate.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| {
                GrepoError::Io(format!(
                    "failed to open mutation lock {}: {e}",
                    path.display()
                ))
            })?;

        match file.try_lock_exclusive() {
            Ok(true) => {}
            Ok(false) => return Err(GrepoError::Busy(grepo_dir.to_path_buf())),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                return Err(GrepoError::Busy(grepo_dir.to_path_buf()));
            }
            Err(e) => {
                return Err(GrepoError::Io(format!(
                    "failed to lock mutation file {}: {e}",
                    path.display()
                )));
            }
        }

        let write_err = |e: std::io::Error| {
            GrepoError::Io(format!(
                "failed to update mutation lock {}: {e}",
                path.display()
            ))
        };
        file.set_len(0).map_err(write_err)?;
        file.seek(SeekFrom::Start(0)).map_err(write_err)?;
        writeln!(file, "{}", std::process::id()).map_err(write_err)?;
        file.sync_all().map_err(write_err)?;

        Ok(Self { file })
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        // Best-effort; there is no meaningful recovery path in Drop.
        let _ = self.file.unlock();
    }
}
