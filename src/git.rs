use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::util::{Result, ensure_dir, err, run_command};

#[derive(Clone, Debug)]
pub struct Git {
    program: OsString,
}

#[derive(Clone, Debug)]
pub enum ResolveSpec {
    DefaultBranch,
    Ref(String),
    Commit(String),
}

impl Git {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub fn hash_string(&self, value: &str) -> Result<String> {
        let args = vec![OsString::from("hash-object"), OsString::from("--stdin")];
        let output = run_command(self.program(), &args, None, Some(value.as_bytes()))?
            .success(self.program(), &args)?;
        Ok(output.stdout.trim().to_string())
    }

    pub fn ensure_remote_cache(&self, remote_dir: &Path, url: &str) -> Result<()> {
        if remote_dir.exists() {
            return Ok(());
        }

        let parent = remote_dir.parent().ok_or_else(|| {
            err(format!(
                "remote cache path has no parent: {}",
                remote_dir.display()
            ))
        })?;
        ensure_dir(parent)?;

        let init_args = self.base_args([
            OsString::from("init"),
            OsString::from("--bare"),
            remote_dir.as_os_str().to_os_string(),
        ]);
        run_command(self.program(), &init_args, None, None)?.success(self.program(), &init_args)?;

        let add_remote_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("remote"),
                OsString::from("add"),
                OsString::from("origin"),
                OsString::from(url),
            ],
        );
        run_command(self.program(), &add_remote_args, None, None)?
            .success(self.program(), &add_remote_args)?;

        Ok(())
    }

    pub fn resolve_spec(&self, remote_dir: &Path, spec: ResolveSpec) -> Result<String> {
        match spec {
            ResolveSpec::DefaultBranch => self.fetch_default_head(remote_dir),
            ResolveSpec::Ref(ref_name) => self.fetch_ref(remote_dir, &ref_name),
            ResolveSpec::Commit(commit) => {
                self.ensure_commit_available(remote_dir, &commit)?;
                Ok(commit)
            }
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
        run_command(self.program(), &fetch_args, None, None)?
            .success(self.program(), &fetch_args)?;
        Ok(())
    }

    pub fn materialize_snapshot(
        &self,
        remote_dir: &Path,
        commit: &str,
        target_dir: &Path,
    ) -> Result<()> {
        let parent = target_dir.parent().ok_or_else(|| {
            err(format!(
                "snapshot path has no parent: {}",
                target_dir.display()
            ))
        })?;
        ensure_dir(parent)?;
        let temp_checkout = crate::util::unique_path(parent, ".grepo-checkout");

        let clone_args = self.base_args([
            OsString::from("clone"),
            OsString::from("--shared"),
            OsString::from("--no-checkout"),
            OsString::from("--no-tags"),
            remote_dir.as_os_str().to_os_string(),
            temp_checkout.as_os_str().to_os_string(),
        ]);
        run_command(self.program(), &clone_args, None, None)?
            .success(self.program(), &clone_args)?;

        let checkout_args = self.base_args([
            OsString::from("-C"),
            temp_checkout.as_os_str().to_os_string(),
            OsString::from("checkout"),
            OsString::from("--detach"),
            OsString::from("--force"),
            OsString::from(commit),
        ]);
        if let Err(error) = run_command(self.program(), &checkout_args, None, None)?
            .success(self.program(), &checkout_args)
        {
            let _ = std::fs::remove_dir_all(&temp_checkout);
            return Err(error);
        }

        std::fs::remove_dir_all(temp_checkout.join(".git")).map_err(|error| {
            err(format!(
                "failed to strip .git from {}: {error}",
                temp_checkout.display()
            ))
        })?;

        std::fs::rename(&temp_checkout, target_dir).map_err(|error| {
            let _ = std::fs::remove_dir_all(&temp_checkout);
            err(format!(
                "failed to move snapshot into place {} -> {}: {error}",
                temp_checkout.display(),
                target_dir.display()
            ))
        })?;

        Ok(())
    }

    pub fn program(&self) -> &OsStr {
        &self.program
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
        run_command(self.program(), &fetch_args, None, None)?
            .success(self.program(), &fetch_args)?;

        let rev_parse_args = self.git_dir_args(
            remote_dir,
            [
                OsString::from("rev-parse"),
                OsString::from("refs/heads/grepo-head"),
            ],
        );
        let output = run_command(self.program(), &rev_parse_args, None, None)?
            .success(self.program(), &rev_parse_args)?;
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
        run_command(self.program(), &fetch_args, None, None)?
            .success(self.program(), &fetch_args)?;

        let rev_parse_args = self.git_dir_args(
            remote_dir,
            [OsString::from("rev-parse"), OsString::from("FETCH_HEAD")],
        );
        let output = run_command(self.program(), &rev_parse_args, None, None)?
            .success(self.program(), &rev_parse_args)?;
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
