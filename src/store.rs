use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::git::Git;
use crate::manifest::Lockfile;
use crate::util::{ensure_dir, is_valid_alias};

#[derive(Clone, Debug)]
pub struct Store {
    cache_root: PathBuf,
    state_root: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct GcReport {
    pub removed_snapshots: Vec<PathBuf>,
    pub removed_remotes: Vec<PathBuf>,
    pub removed_roots: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Git(#[from] crate::git::GitError),

    #[error(transparent)]
    Manifest(#[from] crate::manifest::ManifestError),

    #[error(transparent)]
    Util(#[from] crate::util::UtilError),

    #[error("link path has no parent directory: {path}")]
    MissingLinkParent { path: PathBuf },

    #[error("path collision at {path}: expected a symlink managed by grepo")]
    PathCollision { path: PathBuf },

    #[error("failed to inspect {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read directory entry under {path}: {source}")]
    ReadDirEntry {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to canonicalize {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to create symlink {path} -> {target}: {source}")]
    CreateSymlink {
        path: PathBuf,
        target: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to remove file {path}: {source}")]
    RemoveFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to remove directory {path}: {source}")]
    RemoveDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to set permissions on {path}: {source}")]
    SetPermissions {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Store {
    pub fn new(cache_root: PathBuf, state_root: PathBuf) -> Self {
        Self {
            cache_root,
            state_root,
        }
    }

    fn snapshots_dir(&self) -> PathBuf {
        self.cache_root.join("snapshots")
    }

    fn remotes_dir(&self) -> PathBuf {
        self.cache_root.join("remotes")
    }

    fn roots_dir(&self) -> PathBuf {
        self.state_root.join("roots")
    }

    pub(crate) fn prepare(&self) -> Result<(), StoreError> {
        ensure_dir(&self.snapshots_dir())?;
        ensure_dir(&self.remotes_dir())?;
        ensure_dir(&self.roots_dir())?;
        Ok(())
    }

    pub fn ensure_remote_cache(&self, git: &Git, url: &str) -> Result<PathBuf, StoreError> {
        let remote_key = git.hash_string(url)?;
        let remote_dir = self.remotes_dir().join(format!("{remote_key}.git"));
        git.ensure_remote_cache(&remote_dir, url)?;
        Ok(remote_dir)
    }

    pub fn ensure_snapshot_for_commit(
        &self,
        git: &Git,
        url: &str,
        commit: &str,
    ) -> Result<PathBuf, StoreError> {
        let remote_key = git.hash_string(url)?;
        let snapshot_key = git.hash_string(&format!("{url}\n{commit}"))?;
        let snapshot_dir = self.snapshots_dir().join(&remote_key).join(&snapshot_key);
        if snapshot_dir.exists() {
            return Ok(snapshot_dir);
        }

        let remote_dir = self.ensure_remote_cache(git, url)?;
        git.ensure_commit_available(&remote_dir, commit)?;
        git.materialize_snapshot(&remote_dir, commit, &snapshot_dir)?;
        make_read_only(&snapshot_dir)?;
        Ok(snapshot_dir)
    }

    pub fn refresh_root(&self, git: &Git, lock_path: &Path) -> Result<PathBuf, StoreError> {
        let canonical = lock_path
            .canonicalize()
            .map_err(|source| StoreError::Canonicalize {
                path: lock_path.to_path_buf(),
                source,
            })?;
        let root_key = git.hash_string(&canonical.display().to_string())?;
        let root_link = self.roots_dir().join(format!("{root_key}.lock"));
        if root_link.exists() {
            fs::remove_file(&root_link).map_err(|source| StoreError::RemoveFile {
                path: root_link.clone(),
                source,
            })?;
        }
        symlink(&canonical, &root_link).map_err(|source| StoreError::CreateSymlink {
            path: root_link.clone(),
            target: canonical,
            source,
        })?;
        Ok(root_link)
    }

    pub fn gc(&self, git: &Git) -> Result<GcReport, StoreError> {
        let mut report = GcReport::default();
        let mut reachable_snapshots = BTreeSet::new();
        let mut reachable_remotes = BTreeSet::new();

        for entry in read_dir_paths(&self.roots_dir())? {
            let metadata = fs::symlink_metadata(&entry).map_err(|source| StoreError::Metadata {
                path: entry.clone(),
                source,
            })?;
            if !metadata.file_type().is_symlink() {
                continue;
            }

            let lock_path = match fs::canonicalize(&entry) {
                Ok(path) => path,
                Err(source) => {
                    fs::remove_file(&entry).map_err(|remove_source| StoreError::RemoveFile {
                        path: entry.clone(),
                        source: remove_source,
                    })?;
                    report.removed_roots.push(entry);
                    let _ = source;
                    continue;
                }
            };

            let lockfile = Lockfile::load(&lock_path)?;
            for repo in lockfile.entries() {
                let Some(commit) = &repo.commit else {
                    continue;
                };
                let remote_key = git.hash_string(&repo.url)?;
                let snapshot_key = git.hash_string(&format!("{}\n{}", repo.url, commit))?;
                reachable_snapshots
                    .insert(self.snapshots_dir().join(&remote_key).join(snapshot_key));
                reachable_remotes.insert(self.remotes_dir().join(format!("{remote_key}.git")));
            }
        }

        for url_dir in read_dir_paths(&self.snapshots_dir())? {
            if !url_dir.is_dir() {
                continue;
            }

            for snapshot_dir in read_dir_paths(&url_dir)? {
                if !snapshot_dir.is_dir() || reachable_snapshots.contains(&snapshot_dir) {
                    continue;
                }
                make_writable(&snapshot_dir)?;
                fs::remove_dir_all(&snapshot_dir).map_err(|source| StoreError::RemoveDir {
                    path: snapshot_dir.clone(),
                    source,
                })?;
                report.removed_snapshots.push(snapshot_dir);
            }

            if read_dir_paths(&url_dir)?.is_empty() {
                fs::remove_dir(&url_dir).map_err(|source| StoreError::RemoveDir {
                    path: url_dir.clone(),
                    source,
                })?;
            }
        }

        for remote_dir in read_dir_paths(&self.remotes_dir())? {
            if !remote_dir.is_dir() || reachable_remotes.contains(&remote_dir) {
                continue;
            }
            fs::remove_dir_all(&remote_dir).map_err(|source| StoreError::RemoveDir {
                path: remote_dir.clone(),
                source,
            })?;
            report.removed_remotes.push(remote_dir);
        }

        Ok(report)
    }
}

pub fn replace_symlink(link_path: &Path, target: &Path) -> Result<(), StoreError> {
    if let Some(metadata) = symlink_metadata_if_exists(link_path)? {
        if !metadata.file_type().is_symlink() {
            return Err(StoreError::PathCollision {
                path: link_path.to_path_buf(),
            });
        }
        fs::remove_file(link_path).map_err(|source| StoreError::RemoveFile {
            path: link_path.to_path_buf(),
            source,
        })?;
    }

    let parent = link_path
        .parent()
        .ok_or_else(|| StoreError::MissingLinkParent {
            path: link_path.to_path_buf(),
        })?;
    ensure_dir(parent)?;
    symlink(target, link_path).map_err(|source| StoreError::CreateSymlink {
        path: link_path.to_path_buf(),
        target: target.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn remove_managed_symlink(path: &Path) -> Result<(), StoreError> {
    let Some(metadata) = symlink_metadata_if_exists(path)? else {
        return Ok(());
    };

    if !metadata.file_type().is_symlink() {
        return Err(StoreError::PathCollision {
            path: path.to_path_buf(),
        });
    }

    fs::remove_file(path).map_err(|source| StoreError::RemoveFile {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

pub fn is_managed_symlink_name(name: &str) -> bool {
    !name.starts_with('.') && is_valid_alias(name)
}

fn make_read_only(root: &Path) -> Result<(), StoreError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|source| StoreError::Metadata {
            path: path.clone(),
            source,
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() & !0o222);
        fs::set_permissions(&path, permissions).map_err(|source| StoreError::SetPermissions {
            path: path.clone(),
            source,
        })?;

        if file_type.is_dir() {
            for entry in fs::read_dir(&path).map_err(|source| StoreError::ReadDir {
                path: path.clone(),
                source,
            })? {
                stack.push(
                    entry
                        .map_err(|source| StoreError::ReadDirEntry {
                            path: path.clone(),
                            source,
                        })?
                        .path(),
                );
            }
        }
    }
    Ok(())
}

fn make_writable(root: &Path) -> Result<(), StoreError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|source| StoreError::Metadata {
            path: path.clone(),
            source,
        })?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(&path, permissions).map_err(|source| StoreError::SetPermissions {
            path: path.clone(),
            source,
        })?;

        if file_type.is_dir() {
            for entry in fs::read_dir(&path).map_err(|source| StoreError::ReadDir {
                path: path.clone(),
                source,
            })? {
                stack.push(
                    entry
                        .map_err(|source| StoreError::ReadDirEntry {
                            path: path.clone(),
                            source,
                        })?
                        .path(),
                );
            }
        }
    }
    Ok(())
}

fn read_dir_paths(path: &Path) -> Result<Vec<PathBuf>, StoreError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(|source| StoreError::ReadDir {
        path: path.to_path_buf(),
        source,
    })? {
        entries.push(
            entry
                .map_err(|source| StoreError::ReadDirEntry {
                    path: path.to_path_buf(),
                    source,
                })?
                .path(),
        );
    }
    entries.sort();
    Ok(entries)
}

fn symlink_metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StoreError::Metadata {
            path: path.to_path_buf(),
            source,
        }),
    }
}
