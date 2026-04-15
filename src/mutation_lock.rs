use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::util::{Result, err};

pub(crate) struct MutationLock {
    path: PathBuf,
    _file: fs::File,
}

impl MutationLock {
    pub(crate) fn acquire(grepo_dir: &Path) -> Result<Self> {
        let path = grepo_dir.join(".mutate.lock");
        loop {
            match Self::try_create(&path) {
                Ok(lock) => return Ok(lock),
                Err(TryAcquireError::Retry) => continue,
                Err(TryAcquireError::Stale { pid }) => remove_stale_lock(&path, pid)?,
                Err(TryAcquireError::Busy { pid }) => {
                    let owner = pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
                    return Err(err(format!(
                        "another grepo command is already mutating {}{}",
                        grepo_dir.display(),
                        owner
                    )));
                }
                Err(TryAcquireError::Malformed { contents }) => {
                    let detail = if contents.is_empty() {
                        "lock file is empty".to_string()
                    } else {
                        format!("expected pid, found {:?}", contents)
                    };
                    return Err(err(format!(
                        "cannot recover malformed mutation lock {}: {}; delete it manually",
                        path.display(),
                        detail
                    )));
                }
                Err(TryAcquireError::Io(error)) => {
                    return Err(err(format!(
                        "failed to create mutation lock {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }

    fn try_create(path: &Path) -> std::result::Result<Self, TryAcquireError> {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id()).map_err(TryAcquireError::Io)?;
                file.sync_all().map_err(TryAcquireError::Io)?;
                Ok(Self {
                    path: path.to_path_buf(),
                    _file: file,
                })
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Self::inspect_existing(path),
            Err(error) => Err(TryAcquireError::Io(error)),
        }
    }

    fn inspect_existing(path: &Path) -> std::result::Result<Self, TryAcquireError> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(TryAcquireError::Retry);
            }
            Err(error) => return Err(TryAcquireError::Io(error)),
        };

        let pid_text = contents.trim();
        if pid_text.is_empty() {
            return Err(TryAcquireError::Busy { pid: None });
        }

        let pid = pid_text
            .parse::<u32>()
            .map_err(|_| TryAcquireError::Malformed {
                contents: pid_text.to_string(),
            })?;
        if process_exists(pid) {
            Err(TryAcquireError::Busy { pid: Some(pid) })
        } else {
            Err(TryAcquireError::Stale { pid })
        }
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

enum TryAcquireError {
    Retry,
    Busy { pid: Option<u32> },
    Stale { pid: u32 },
    Malformed { contents: String },
    Io(std::io::Error),
}

fn remove_stale_lock(path: &Path, pid: u32) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(err(format!(
            "failed to remove stale mutation lock {} owned by pid {}: {error}",
            path.display(),
            pid
        ))),
    }
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    let result = unsafe { kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }

    match std::io::Error::last_os_error().raw_os_error() {
        Some(EPERM) => true,
        Some(ESRCH) => false,
        _ => false,
    }
}

#[cfg(not(unix))]
fn process_exists(pid: u32) -> bool {
    let _ = pid;
    false
}

#[cfg(unix)]
const EPERM: i32 = 1;
#[cfg(unix)]
const ESRCH: i32 = 3;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
