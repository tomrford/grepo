use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;

use crate::error::{GrepoError, Result};
use crate::util::{ensure_dir_mode, run_command, unique_path};

#[derive(Clone, Debug)]
pub struct Git {
    program: OsString,
}

#[derive(Clone, Debug)]
pub enum ResolveSpec {
    DefaultBranch,
    Ref(String),
}

impl Git {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub fn hash_string(&self, value: &str) -> Result<String> {
        let args = vec![OsString::from("hash-object"), OsString::from("--stdin")];
        let output = run_command(self.program(), &args, None, Some(value.as_bytes()))?.check()?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn ensure_remote_cache(&self, remote_dir: &Path, url: &str) -> Result<()> {
        let parent = remote_dir.parent().ok_or_else(|| {
            GrepoError::Io(format!(
                "remote cache path has no parent: {}",
                remote_dir.display()
            ))
        })?;
        ensure_dir_mode(parent, 0o700)?;

        if remote_dir.exists() {
            if self.remote_origin_matches(remote_dir, url)? {
                return Ok(());
            }
            remove_path(remote_dir)?;
        }

        let temp_remote_dir = unique_path(parent, ".grepo-remote");
        if let Err(error) = self.initialize_remote_cache(&temp_remote_dir, url) {
            let _ = remove_path(&temp_remote_dir);
            return Err(error);
        }
        fs::rename(&temp_remote_dir, remote_dir).map_err(|e| {
            GrepoError::Io(format!(
                "failed to move remote cache into place {} -> {}: {e}",
                temp_remote_dir.display(),
                remote_dir.display()
            ))
        })?;

        Ok(())
    }

    pub fn resolve_spec(&self, remote_dir: &Path, spec: ResolveSpec) -> Result<String> {
        match spec {
            ResolveSpec::DefaultBranch => self.fetch_default_head(remote_dir),
            ResolveSpec::Ref(ref_name) => self.fetch_ref(remote_dir, &ref_name),
        }
    }

    pub fn ensure_commit_available(&self, remote_dir: &Path, commit: &str) -> Result<()> {
        if self.has_commit(remote_dir, commit)? {
            return Ok(());
        }

        let fetch_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("fetch"),
                OsString::from("--no-tags"),
                OsString::from("origin"),
                OsString::from(commit),
            ],
        );
        run_command(self.program(), &fetch_args, None, None)?.check()?;
        Ok(())
    }

    pub fn materialize_snapshot(
        &self,
        remote_dir: &Path,
        commit: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let parent = target_dir.parent().ok_or_else(|| {
            GrepoError::Io(format!(
                "snapshot path has no parent: {}",
                target_dir.display()
            ))
        })?;
        ensure_dir_mode(parent, 0o700)?;
        let temp_checkout = unique_path(parent, ".grepo-checkout");

        let clone_args = self.base_args([
            OsString::from("clone"),
            OsString::from("--shared"),
            OsString::from("--no-checkout"),
            OsString::from("--no-tags"),
            remote_dir.as_os_str().to_os_string(),
            temp_checkout.as_os_str().to_os_string(),
        ]);
        run_command(self.program(), &clone_args, None, None)?.check()?;

        let checkout_args = self.base_args([
            OsString::from("-C"),
            temp_checkout.as_os_str().to_os_string(),
            OsString::from("checkout"),
            OsString::from("--detach"),
            OsString::from("--force"),
            OsString::from(commit),
        ]);
        if let Err(error) = run_command(self.program(), &checkout_args, None, None)?.check() {
            let _ = remove_path(&temp_checkout);
            return Err(error);
        }

        let git_dir = temp_checkout.join(".git");
        fs::remove_dir_all(&git_dir).map_err(|e| {
            GrepoError::Io(format!(
                "failed to strip .git from {}: {e}",
                temp_checkout.display()
            ))
        })?;

        fs::rename(&temp_checkout, target_dir).map_err(|e| {
            GrepoError::Io(format!(
                "failed to move snapshot into place {} -> {}: {e}",
                temp_checkout.display(),
                target_dir.display()
            ))
        })?;

        Ok(())
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    fn initialize_remote_cache(&self, remote_dir: &Path, url: &str) -> Result<()> {
        let init_args = self.base_args([
            OsString::from("init"),
            OsString::from("--bare"),
            remote_dir.as_os_str().to_os_string(),
        ]);
        run_command(self.program(), &init_args, None, None)?.check()?;
        ensure_dir_mode(remote_dir, 0o700)?;

        let add_remote_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("remote"),
                OsString::from("add"),
                OsString::from("origin"),
                OsString::from(url),
            ],
        );
        run_command(self.program(), &add_remote_args, None, None)?.check()?;

        Ok(())
    }

    fn remote_origin_matches(&self, remote_dir: &Path, expected_url: &str) -> Result<bool> {
        let args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("config"),
                OsString::from("--get"),
                OsString::from("remote.origin.url"),
            ],
        );
        let output = run_command(self.program(), &args, None, None)?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(output.stdout.trim() == expected_url)
    }

    fn fetch_default_head(&self, remote_dir: &Path) -> Result<String> {
        let fetch_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("fetch"),
                OsString::from("--prune"),
                OsString::from("--no-tags"),
                OsString::from("origin"),
                OsString::from("+HEAD:refs/heads/grepo-head"),
            ],
        );
        run_command(self.program(), &fetch_args, None, None)?.check()?;

        let rev_parse_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("rev-parse"),
                OsString::from("refs/heads/grepo-head"),
            ],
        );
        let output = run_command(self.program(), &rev_parse_args, None, None)?.check()?;
        Ok(output.stdout.trim().to_string())
    }

    fn fetch_ref(&self, remote_dir: &Path, ref_name: &str) -> Result<String> {
        let fetch_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("fetch"),
                OsString::from("--no-tags"),
                OsString::from("origin"),
                OsString::from(ref_name),
            ],
        );
        run_command(self.program(), &fetch_args, None, None)?.check()?;

        let rev_parse_args = self.git_dir_args(
            remote_dir,
            [OsString::from("rev-parse"), OsString::from("FETCH_HEAD")],
        );
        let output = run_command(self.program(), &rev_parse_args, None, None)?.check()?;
        Ok(output.stdout.trim().to_string())
    }

    fn has_commit(&self, remote_dir: &Path, commit: &str) -> Result<bool> {
        let probe = format!("{commit}^{{commit}}");
        let args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("cat-file"),
                OsString::from("-e"),
                OsString::from(probe),
            ],
        );
        let output = run_command(self.program(), &args, None, None)?;
        Ok(output.status.success())
    }

    fn base_args(&self, tail: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
        let mut args = Vec::new();
        args.push(OsString::from("-c"));
        args.push(OsString::from("core.hooksPath=/dev/null"));
        args.extend(tail);
        args
    }

    fn git_dir_args(
        &self,
        git_dir: &Path,
        tail: impl IntoIterator<Item = OsString>,
    ) -> Vec<OsString> {
        let mut args = self.base_args([OsString::from(format!("--git-dir={}", git_dir.display()))]);
        args.extend(tail);
        args
    }
}

pub fn validate_ref_name(ref_name: &str) -> Result<()> {
    if ref_name.is_empty()
        || ref_name.starts_with('-')
        || ref_name.starts_with('/')
        || ref_name.ends_with('/')
        || ref_name.ends_with('.')
        || ref_name.contains("//")
        || ref_name.contains("..")
        || ref_name.contains("@{")
    {
        return Err(GrepoError::InvalidRef(ref_name.to_string()));
    }

    if ref_name.as_bytes().iter().any(|byte| {
        byte.is_ascii_control()
            || matches!(
                *byte,
                b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'
            )
    }) {
        return Err(GrepoError::InvalidRef(ref_name.to_string()));
    }

    for component in ref_name.split('/') {
        if component.is_empty()
            || component.starts_with('.')
            || component.ends_with(".lock")
            || component.ends_with('.')
        {
            return Err(GrepoError::InvalidRef(ref_name.to_string()));
        }
    }

    Ok(())
}

pub fn validate_commit_oid(commit: &str) -> Result<()> {
    let valid_length = matches!(commit.len(), 40 | 64);
    if !valid_length || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GrepoError::InvalidCommit(commit.to_string()));
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(GrepoError::Io(format!(
                "failed to inspect {}: {e}",
                path.display()
            )));
        }
    };

    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
            .map_err(|e| GrepoError::Io(format!("failed to remove {}: {e}", path.display())))?;
    } else {
        fs::remove_file(path)
            .map_err(|e| GrepoError::Io(format!("failed to remove {}: {e}", path.display())))?;
    }

    Ok(())
}
