use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::error::{GrepoError, Result};
use crate::git::Git;
use crate::manifest::{LockEntry, Lockfile};
use crate::mutation_lock::FileLock;
use crate::registry::get_bytes;
use crate::tarball;
use crate::util::{ensure_dir, ensure_dir_mode, is_valid_alias};

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
    pub warnings: Vec<String>,
}

pub struct StoreMutationLock {
    _lock: FileLock,
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

    fn tarballs_dir(&self) -> PathBuf {
        self.cache_root.join("tarballs")
    }

    fn remotes_dir(&self) -> PathBuf {
        self.cache_root.join("remotes")
    }

    fn roots_dir(&self) -> PathBuf {
        self.state_root.join("roots")
    }

    fn locks_dir(&self) -> PathBuf {
        self.state_root.join("locks")
    }

    fn store_lock_path(&self) -> PathBuf {
        self.locks_dir().join("store.lock")
    }

    pub(crate) fn prepare(&self) -> Result<()> {
        ensure_dir_mode(&self.cache_root, 0o700)?;
        ensure_dir_mode(&self.state_root, 0o700)?;
        ensure_dir_mode(&self.snapshots_dir(), 0o700)?;
        ensure_dir_mode(&self.tarballs_dir(), 0o700)?;
        ensure_dir_mode(&self.remotes_dir(), 0o700)?;
        ensure_dir_mode(&self.roots_dir(), 0o700)?;
        ensure_dir_mode(&self.locks_dir(), 0o700)?;
        Ok(())
    }

    pub fn lock_mutation(&self) -> Result<StoreMutationLock> {
        let lock = FileLock::try_acquire(&self.store_lock_path())?;
        let Some(lock) = lock else {
            return Err(GrepoError::StoreBusy(self.state_root.clone()));
        };
        Ok(StoreMutationLock { _lock: lock })
    }

    pub fn with_remote_cache<T>(
        &self,
        git: &Git,
        url: &str,
        f: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        let remote_key = self.remote_key(git, url)?;
        self.with_remote_cache_for_key(git, url, &remote_key, f)
    }

    pub fn ensure_snapshot_for_commit(
        &self,
        git: &Git,
        url: &str,
        commit: &str,
        subdir: Option<&str>,
    ) -> Result<PathBuf> {
        let remote_key = self.remote_key(git, url)?;
        let snapshot_key = self.snapshot_key(git, url, commit, subdir)?;
        let snapshot_dir = self.snapshot_dir_for_keys(&remote_key, &snapshot_key);
        self.with_remote_cache_for_key(git, url, &remote_key, |remote_dir| {
            if snapshot_dir.exists() {
                return Ok(snapshot_dir.clone());
            }
            git.ensure_commit_available(remote_dir, commit)?;
            git.materialize_snapshot(remote_dir, commit, &snapshot_dir, subdir)?;
            make_read_only(&snapshot_dir)?;
            Ok(snapshot_dir)
        })
    }

    pub fn ensure_snapshot_for_tarball(
        &self,
        agent: &ureq::Agent,
        download_url: &str,
        sha256: &str,
    ) -> Result<PathBuf> {
        let snapshot_dir = self.tarball_dir_for_sha(sha256);
        let _lock = FileLock::acquire(&self.tarball_lock_path(sha256))?;
        if snapshot_dir.exists() {
            return Ok(snapshot_dir);
        }
        let bytes = get_bytes(agent, download_url)?;
        tarball::verify_sha256(&bytes, sha256)?;
        tarball::extract_crate_tarball(&bytes, &snapshot_dir)?;
        make_read_only(&snapshot_dir)?;
        Ok(snapshot_dir)
    }

    pub fn refresh_root(&self, git: &Git, lock_path: &Path) -> Result<PathBuf> {
        let canonical = lock_path.canonicalize().map_err(|e| {
            GrepoError::Io(format!(
                "failed to canonicalize {}: {e}",
                lock_path.display()
            ))
        })?;
        let root_link = self.root_link(git, &canonical)?;
        if root_link.exists() {
            fs::remove_file(&root_link).map_err(|e| {
                GrepoError::Io(format!("failed to remove {}: {e}", root_link.display()))
            })?;
        }
        symlink(&canonical, &root_link).map_err(|e| {
            GrepoError::Io(format!(
                "failed to create symlink {} -> {}: {e}",
                root_link.display(),
                canonical.display()
            ))
        })?;
        Ok(root_link)
    }

    pub fn gc(&self, git: &Git) -> Result<GcReport> {
        let mut report = GcReport::default();
        let mut reachable_snapshots = BTreeSet::new();
        let mut reachable_remotes = BTreeSet::new();
        let mut reachable_tarballs = BTreeSet::new();

        for entry in read_dir_paths(&self.roots_dir())? {
            let metadata = fs::symlink_metadata(&entry).map_err(|e| {
                GrepoError::Io(format!("failed to inspect {}: {e}", entry.display()))
            })?;
            if !metadata.file_type().is_symlink() {
                continue;
            }

            let lock_path = match fs::canonicalize(&entry) {
                Ok(path) => path,
                Err(_) => {
                    fs::remove_file(&entry).map_err(|e| {
                        GrepoError::Io(format!("failed to remove {}: {e}", entry.display()))
                    })?;
                    report.removed_roots.push(entry);
                    continue;
                }
            };

            let lockfile = match Lockfile::load(&lock_path) {
                Ok(lockfile) => lockfile,
                Err(error) => {
                    report.warnings.push(format!(
                        "skipped rooted lockfile {}: {error}",
                        lock_path.display()
                    ));
                    continue;
                }
            };
            for repo in lockfile.entries() {
                match repo {
                    LockEntry::Git(git_entry) => {
                        let Some(commit) = &git_entry.commit else {
                            continue;
                        };
                        let remote_key = self.remote_key(git, &git_entry.url)?;
                        let snapshot_key = self.snapshot_key(
                            git,
                            &git_entry.url,
                            commit,
                            git_entry.subdir.as_deref(),
                        )?;
                        reachable_snapshots
                            .insert(self.snapshot_dir_for_keys(&remote_key, &snapshot_key));
                        reachable_remotes.insert(self.remote_dir_for_key(&remote_key));
                    }
                    LockEntry::Tarball(tarball_entry) => {
                        reachable_tarballs.insert(self.tarball_dir_for_sha(&tarball_entry.sha256));
                    }
                }
            }
        }

        for url_dir in read_dir_paths(&self.snapshots_dir())? {
            if !url_dir.is_dir() {
                continue;
            }

            let mut has_remaining_entries = false;
            for snapshot_dir in read_dir_paths(&url_dir)? {
                if !snapshot_dir.is_dir() || reachable_snapshots.contains(&snapshot_dir) {
                    has_remaining_entries = true;
                    continue;
                }
                make_writable(&snapshot_dir)?;
                fs::remove_dir_all(&snapshot_dir).map_err(|e| {
                    GrepoError::Io(format!("failed to remove {}: {e}", snapshot_dir.display()))
                })?;
                report.removed_snapshots.push(snapshot_dir);
            }

            if !has_remaining_entries {
                fs::remove_dir(&url_dir).map_err(|e| {
                    GrepoError::Io(format!("failed to remove {}: {e}", url_dir.display()))
                })?;
            }
        }

        for remote_dir in read_dir_paths(&self.remotes_dir())? {
            if !remote_dir.is_dir() || reachable_remotes.contains(&remote_dir) {
                continue;
            }
            fs::remove_dir_all(&remote_dir).map_err(|e| {
                GrepoError::Io(format!("failed to remove {}: {e}", remote_dir.display()))
            })?;
            report.removed_remotes.push(remote_dir);
        }

        for tarball_dir in read_dir_paths(&self.tarballs_dir())? {
            if !tarball_dir.is_dir() || reachable_tarballs.contains(&tarball_dir) {
                continue;
            }
            make_writable(&tarball_dir)?;
            fs::remove_dir_all(&tarball_dir).map_err(|e| {
                GrepoError::Io(format!("failed to remove {}: {e}", tarball_dir.display()))
            })?;
            report.removed_snapshots.push(tarball_dir);
        }

        Ok(report)
    }

    fn remote_key(&self, git: &Git, url: &str) -> Result<String> {
        git.hash_string(url)
    }

    fn snapshot_key(
        &self,
        git: &Git,
        url: &str,
        commit: &str,
        subdir: Option<&str>,
    ) -> Result<String> {
        let payload = format!("{url}\n{commit}\n{}", subdir.unwrap_or(""));
        git.hash_string(&payload)
    }

    fn remote_dir_for_key(&self, remote_key: &str) -> PathBuf {
        self.remotes_dir().join(format!("{remote_key}.git"))
    }

    fn snapshot_dir_for_keys(&self, remote_key: &str, snapshot_key: &str) -> PathBuf {
        self.snapshots_dir().join(remote_key).join(snapshot_key)
    }

    fn tarball_dir_for_sha(&self, sha256: &str) -> PathBuf {
        self.tarballs_dir().join(sha256)
    }

    fn tarball_lock_path(&self, sha256: &str) -> PathBuf {
        self.locks_dir().join(format!("tarball-{sha256}.lock"))
    }

    fn ensure_remote_cache_for_key(
        &self,
        git: &Git,
        url: &str,
        remote_key: &str,
    ) -> Result<PathBuf> {
        let remote_dir = self.remote_dir_for_key(remote_key);
        git.ensure_remote_cache(&remote_dir, url)?;
        Ok(remote_dir)
    }

    fn with_remote_cache_for_key<T>(
        &self,
        git: &Git,
        url: &str,
        remote_key: &str,
        f: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        let _lock = FileLock::acquire(&self.remote_lock_path(remote_key))?;
        let remote_dir = self.ensure_remote_cache_for_key(git, url, remote_key)?;
        f(&remote_dir)
    }

    fn root_link(&self, git: &Git, canonical_lock_path: &Path) -> Result<PathBuf> {
        let root_key = git.hash_string(&canonical_lock_path.display().to_string())?;
        Ok(self.roots_dir().join(format!("{root_key}.lock")))
    }

    fn remote_lock_path(&self, remote_key: &str) -> PathBuf {
        self.locks_dir().join(format!("remote-{remote_key}.lock"))
    }
}

pub fn replace_symlink(link_path: &Path, target: &Path) -> Result<()> {
    if let Some(metadata) = symlink_metadata_if_exists(link_path)? {
        if !metadata.file_type().is_symlink() {
            return Err(GrepoError::PathCollision(link_path.to_path_buf()));
        }
        fs::remove_file(link_path).map_err(|e| {
            GrepoError::Io(format!("failed to remove {}: {e}", link_path.display()))
        })?;
    }

    let parent = link_path.parent().ok_or_else(|| {
        GrepoError::Io(format!(
            "link path has no parent directory: {}",
            link_path.display()
        ))
    })?;
    ensure_dir(parent)?;
    symlink(target, link_path).map_err(|e| {
        GrepoError::Io(format!(
            "failed to create symlink {} -> {}: {e}",
            link_path.display(),
            target.display()
        ))
    })?;
    Ok(())
}

pub fn remove_managed_symlink(path: &Path) -> Result<()> {
    let Some(metadata) = symlink_metadata_if_exists(path)? else {
        return Ok(());
    };

    if !metadata.file_type().is_symlink() {
        return Err(GrepoError::PathCollision(path.to_path_buf()));
    }

    fs::remove_file(path)
        .map_err(|e| GrepoError::Io(format!("failed to remove {}: {e}", path.display())))?;
    Ok(())
}

pub fn is_managed_symlink_name(name: &str) -> bool {
    !name.starts_with('.') && is_valid_alias(name)
}

fn make_read_only(root: &Path) -> Result<()> {
    rewrite_tree_modes(root, |is_dir, mode| {
        if is_dir || mode & 0o100 != 0 {
            0o500
        } else {
            0o400
        }
    })
}

fn make_writable(root: &Path) -> Result<()> {
    rewrite_tree_modes(root, |is_dir, mode| {
        if is_dir || mode & 0o100 != 0 {
            0o700
        } else {
            0o600
        }
    })
}

fn rewrite_tree_modes(root: &Path, rewrite: impl Fn(bool, u32) -> u32) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| GrepoError::Io(format!("failed to inspect {}: {e}", path.display())))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(rewrite(file_type.is_dir(), permissions.mode()));
        fs::set_permissions(&path, permissions).map_err(|e| {
            GrepoError::Io(format!(
                "failed to set permissions on {}: {e}",
                path.display()
            ))
        })?;

        if file_type.is_dir() {
            for entry in fs::read_dir(&path).map_err(|e| {
                GrepoError::Io(format!("failed to read directory {}: {e}", path.display()))
            })? {
                stack.push(
                    entry
                        .map_err(|e| {
                            GrepoError::Io(format!(
                                "failed to read directory entry under {}: {e}",
                                path.display()
                            ))
                        })?
                        .path(),
                );
            }
        }
    }
    Ok(())
}

pub fn read_dir_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|e| GrepoError::Io(format!("failed to read directory {}: {e}", path.display())))?
    {
        entries.push(
            entry
                .map_err(|e| {
                    GrepoError::Io(format!(
                        "failed to read directory entry under {}: {e}",
                        path.display()
                    ))
                })?
                .path(),
        );
    }
    entries.sort();
    Ok(entries)
}

pub fn symlink_metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(GrepoError::Io(format!(
            "failed to inspect {}: {e}",
            path.display()
        ))),
    }
}
