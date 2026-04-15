use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::git::Git;
use crate::manifest::Lockfile;
use crate::util::{Result, ensure_dir, err};

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

    pub(crate) fn prepare(&self) -> Result<()> {
        ensure_dir(&self.snapshots_dir())?;
        ensure_dir(&self.remotes_dir())?;
        ensure_dir(&self.roots_dir())?;
        Ok(())
    }

    pub fn ensure_remote_cache(&self, git: &Git, url: &str) -> Result<PathBuf> {
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
    ) -> Result<PathBuf> {
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

    pub fn refresh_root(&self, git: &Git, lock_path: &Path) -> Result<PathBuf> {
        let canonical = lock_path
            .canonicalize()
            .unwrap_or_else(|_| lock_path.to_path_buf());
        let root_key = git.hash_string(&canonical.display().to_string())?;
        let root_link = self.roots_dir().join(format!("{root_key}.lock"));
        if root_link.exists() {
            fs::remove_file(&root_link)?;
        }
        symlink(&canonical, &root_link)?;
        Ok(root_link)
    }

    pub fn gc(&self, git: &Git) -> Result<GcReport> {
        let mut report = GcReport::default();
        let mut reachable_snapshots = BTreeSet::new();
        let mut reachable_remotes = BTreeSet::new();

        for entry in read_dir_paths(&self.roots_dir())? {
            let metadata = fs::symlink_metadata(&entry)?;
            if !metadata.file_type().is_symlink() {
                continue;
            }

            let Ok(lock_path) = fs::canonicalize(&entry) else {
                fs::remove_file(&entry)?;
                report.removed_roots.push(entry);
                continue;
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
                fs::remove_dir_all(&snapshot_dir)?;
                report.removed_snapshots.push(snapshot_dir);
            }

            if read_dir_paths(&url_dir)?.is_empty() {
                fs::remove_dir(&url_dir)?;
            }
        }

        for remote_dir in read_dir_paths(&self.remotes_dir())? {
            if !remote_dir.is_dir() || reachable_remotes.contains(&remote_dir) {
                continue;
            }
            fs::remove_dir_all(&remote_dir)?;
            report.removed_remotes.push(remote_dir);
        }

        Ok(report)
    }
}

pub fn replace_symlink(link_path: &Path, target: &Path) -> Result<()> {
    if let Some(metadata) = symlink_metadata_if_exists(link_path)? {
        if !metadata.file_type().is_symlink() {
            return Err(err(format!(
                "path collision at {}: expected a symlink managed by grepo",
                link_path.display()
            )));
        }
        fs::remove_file(link_path)?;
    }

    let parent = link_path.parent().ok_or_else(|| {
        err(format!(
            "link path has no parent directory: {}",
            link_path.display()
        ))
    })?;
    ensure_dir(parent)?;
    symlink(target, link_path)?;
    Ok(())
}

pub fn remove_managed_symlink(path: &Path) -> Result<()> {
    let Some(metadata) = symlink_metadata_if_exists(path)? else {
        return Ok(());
    };

    if !metadata.file_type().is_symlink() {
        return Err(err(format!(
            "path collision at {}: expected a symlink managed by grepo",
            path.display()
        )));
    }

    fs::remove_file(path)?;
    Ok(())
}

fn make_read_only(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() & !0o222);
        fs::set_permissions(&path, permissions)?;

        if file_type.is_dir() {
            for entry in fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        }
    }
    Ok(())
}

fn make_writable(root: &Path) -> Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(&path, permissions)?;

        if file_type.is_dir() {
            for entry in fs::read_dir(&path)? {
                stack.push(entry?.path());
            }
        }
    }
    Ok(())
}

fn read_dir_paths(path: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        entries.push(entry?.path());
    }
    entries.sort();
    Ok(entries)
}

fn symlink_metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
