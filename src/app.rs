use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use thiserror::Error;

use crate::cli::{Cli, Command as CliCommand, RemoveArgs, UpdateArgs};
use crate::error::{GrepoError, Result};
use crate::git::{Git, ResolveSpec};
use crate::manifest::{LockEntry, LockMode, Lockfile};
use crate::mutation_lock::MutationLock;
use crate::output::RunReport;
use crate::store::{
    GcReport, Store, is_managed_symlink_name, remove_managed_symlink, replace_symlink,
};
use crate::util::{cache_root, current_dir, ensure_dir, is_valid_alias, state_root, write_atomic};

pub fn main_entry() -> ExitCode {
    match run_env(std::env::args_os().collect()) {
        Ok(report) => {
            let exit_code = report.exit_code();
            if let Err(error) = report.print() {
                eprintln!("error: failed to write command output: {error}");
                return ExitCode::from(1);
            }
            exit_code
        }
        Err(GrepoError::Cli(error)) => {
            let exit_code = error.exit_code();
            let _ = error.print();
            ExitCode::from(exit_code as u8)
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_env(args: Vec<OsString>) -> Result<RunReport> {
    let cli = Cli::try_parse_from(args)?;
    let command = Command::try_from(cli.command)?;
    let cwd = current_dir()?;
    let context = AppContext {
        cwd,
        cache_root: cache_root()?,
        state_root: state_root()?,
        git: Git::new("git"),
    };
    command.run(&context)
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Git(#[from] crate::git::GitError),

    #[error(transparent)]
    Manifest(#[from] crate::manifest::ManifestError),

    #[error(transparent)]
    MutationLock(#[from] crate::mutation_lock::MutationLockError),

    #[error(transparent)]
    Store(#[from] crate::store::StoreError),

    #[error(transparent)]
    Util(#[from] crate::util::UtilError),

    #[error("invalid alias: {alias}")]
    InvalidAlias { alias: String },

    #[error("cannot initialize grepo root because {path} is not a directory")]
    RootPathNotDirectory { path: PathBuf },

    #[error("no grepo root found from {start}")]
    NoProjectRoot { start: PathBuf },

    #[error("alias not found: {alias}")]
    AliasNotFound { alias: String },

    #[error("alias {alias} is exact but has no commit")]
    MissingExactCommit { alias: String },

    #[error("alias {alias} has no commit")]
    MissingCommit { alias: String },

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

    #[error("failed to inspect {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Clone, Debug)]
struct AppContext {
    cwd: PathBuf,
    cache_root: PathBuf,
    state_root: PathBuf,
    git: Git,
}

impl AppContext {
    fn store(&self) -> Store {
        Store::new(self.cache_root.clone(), self.state_root.clone())
    }
}

#[derive(Clone, Debug)]
struct ProjectRoot {
    grepo_dir: PathBuf,
    lock_path: PathBuf,
}

impl ProjectRoot {
    fn discover(start: &Path) -> Option<Self> {
        for dir in start.ancestors() {
            let grepo_dir = dir.join("grepo");
            let lock_path = grepo_dir.join(".lock");
            if lock_path.is_file() {
                return Some(Self {
                    grepo_dir,
                    lock_path,
                });
            }
        }
        None
    }

    fn create_at(project_dir: &Path) -> Result<Self> {
        let grepo_dir = project_dir.join("grepo");
        if grepo_dir.exists() && !grepo_dir.is_dir() {
            return Err(AppError::RootPathNotDirectory { path: grepo_dir }.into());
        }
        ensure_dir(&grepo_dir)?;

        let lock_path = grepo_dir.join(".lock");
        if !lock_path.exists() {
            write_atomic(&lock_path, "")?;
        }
        write_atomic(&grepo_dir.join(".gitignore"), "*\n!.gitignore\n!.lock\n")?;

        Ok(Self {
            grepo_dir,
            lock_path,
        })
    }

    fn load_lockfile(&self) -> Result<Lockfile> {
        if !self.lock_path.exists() {
            return Ok(Lockfile::default());
        }
        Ok(Lockfile::load(&self.lock_path)?)
    }

    fn lock_mutation(&self) -> Result<MutationLock> {
        Ok(MutationLock::acquire(&self.grepo_dir)?)
    }
}

#[derive(Clone, Debug)]
enum Command {
    Init,
    Add(AddArgs),
    Remove { aliases: Vec<String> },
    Sync,
    Update { aliases: Vec<String> },
    Gc,
}

#[derive(Clone, Debug)]
struct AddArgs {
    alias: String,
    url: String,
    ref_name: Option<String>,
    commit: Option<String>,
}

impl TryFrom<CliCommand> for Command {
    type Error = AppError;

    fn try_from(value: CliCommand) -> std::result::Result<Self, Self::Error> {
        match value {
            CliCommand::Init => Ok(Self::Init),
            CliCommand::Add(args) => Ok(Self::Add(AddArgs {
                alias: validate_alias(args.alias)?,
                url: args.url,
                ref_name: args.ref_name,
                commit: args.commit,
            })),
            CliCommand::Remove(RemoveArgs { aliases }) => Ok(Self::Remove {
                aliases: validate_aliases(aliases)?,
            }),
            CliCommand::Sync => Ok(Self::Sync),
            CliCommand::Update(UpdateArgs { aliases }) => Ok(Self::Update {
                aliases: validate_aliases(aliases)?,
            }),
            CliCommand::Gc => Ok(Self::Gc),
        }
    }
}

impl Command {
    fn run(self, context: &AppContext) -> Result<RunReport> {
        match self {
            Self::Init => init(context),
            Self::Add(args) => add(context, args),
            Self::Remove { aliases } => remove(context, &aliases),
            Self::Sync => sync(context),
            Self::Update { aliases } => update(context, &aliases),
            Self::Gc => gc(context),
        }
    }
}

fn init(context: &AppContext) -> Result<RunReport> {
    let root = ProjectRoot::create_at(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut report = RunReport::success();
    report.stdout_line(format!("initialized {}", root.grepo_dir.display()));
    Ok(report)
}

fn add(context: &AppContext, args: AddArgs) -> Result<RunReport> {
    let root = match ProjectRoot::discover(&context.cwd) {
        Some(root) => root,
        None => ProjectRoot::create_at(&context.cwd)?,
    };
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;

    let mut lockfile = root.load_lockfile()?;
    let mut entry = LockEntry::new(args.alias.clone(), args.url);
    if let Some(ref_name) = args.ref_name {
        entry.mode = LockMode::Ref { ref_name };
    } else if let Some(commit) = args.commit {
        entry.commit = Some(commit);
        entry.mode = LockMode::Exact;
    } else {
        entry.mode = LockMode::Default;
    }

    let (entry, snapshot_path) = realize_entry(context, &store, &entry, false)?;
    lockfile.upsert(entry);
    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;
    replace_symlink(&root.grepo_dir.join(&args.alias), &snapshot_path)?;

    let mut report = RunReport::success();
    report.stdout_line(format!(
        "added {} -> {}",
        args.alias,
        snapshot_path.display()
    ));
    Ok(report)
}

fn remove(context: &AppContext, aliases: &[String]) -> Result<RunReport> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;

    for alias in aliases {
        if !lockfile.remove(alias) {
            return Err(AppError::AliasNotFound {
                alias: alias.clone(),
            }
            .into());
        }
    }

    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut report = RunReport::success();
    for alias in aliases {
        if let Err(error) = remove_managed_symlink(&root.grepo_dir.join(alias)) {
            report.warn_line(error.to_string());
        } else {
            report.stdout_line(format!("removed {alias}"));
        }
    }

    Ok(report)
}

fn sync(context: &AppContext) -> Result<RunReport> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;
    let mut dirty_lock = false;
    let mut report = RunReport::success();

    for alias in lockfile.aliases() {
        let Some(entry) = lockfile.get(&alias).cloned() else {
            continue;
        };
        match realize_entry(context, &store, &entry, false) {
            Ok((updated_entry, snapshot_path)) => {
                if updated_entry != entry {
                    dirty_lock = true;
                    lockfile.upsert(updated_entry.clone());
                }
                replace_symlink(&root.grepo_dir.join(&alias), &snapshot_path)?;
                report.stdout_line(format!("synced {} -> {}", alias, snapshot_path.display()));
            }
            Err(error) => report.warn_line(format!("failed to sync {}: {error}", alias)),
        }
    }

    let keep = lockfile.aliases().into_iter().collect::<BTreeSet<_>>();
    prune_leftover_links(&root, &keep, &mut report)?;
    if dirty_lock {
        lockfile.write(&root.lock_path)?;
    }
    store.refresh_root(&context.git, &root.lock_path)?;
    Ok(report)
}

fn update(context: &AppContext, aliases: &[String]) -> Result<RunReport> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;
    let selected = lockfile.select_aliases(aliases)?;
    let mut report = RunReport::success();

    for alias in selected {
        let Some(entry) = lockfile.get(&alias).cloned() else {
            continue;
        };
        match realize_entry(context, &store, &entry, true) {
            Ok((updated_entry, snapshot_path)) => {
                lockfile.upsert(updated_entry);
                replace_symlink(&root.grepo_dir.join(&alias), &snapshot_path)?;
                report.stdout_line(format!("updated {} -> {}", alias, snapshot_path.display()));
            }
            Err(error) => report.warn_line(format!("failed to update {}: {error}", alias)),
        }
    }

    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;
    Ok(report)
}

fn gc(context: &AppContext) -> Result<RunReport> {
    let store = prepared_store(context)?;
    let gc_report = store.gc(&context.git)?;
    let mut report = RunReport::success();
    append_gc_report(&mut report, &gc_report);
    if report.stdout().is_empty() && report.stderr().is_empty() {
        report.stdout_line("gc: nothing to remove");
    }
    Ok(report)
}

fn realize_entry(
    context: &AppContext,
    store: &Store,
    entry: &LockEntry,
    refresh_tracking: bool,
) -> Result<(LockEntry, PathBuf)> {
    let mut updated = entry.clone();
    if refresh_tracking || updated.commit.is_none() {
        if updated.can_update() {
            updated.commit = Some(resolve_tracking_commit(context, store, &updated)?);
        } else if updated.commit.is_none() {
            return Err(AppError::MissingExactCommit {
                alias: updated.alias.clone(),
            }
            .into());
        }
    }

    let commit = updated
        .commit
        .as_deref()
        .ok_or_else(|| AppError::MissingCommit {
            alias: updated.alias.clone(),
        })?;
    let snapshot_path = store.ensure_snapshot_for_commit(&context.git, &updated.url, commit)?;
    Ok((updated, snapshot_path))
}

fn resolve_tracking_commit(
    context: &AppContext,
    store: &Store,
    entry: &LockEntry,
) -> Result<String> {
    let remote_dir = store.ensure_remote_cache(&context.git, &entry.url)?;
    let spec = match &entry.mode {
        LockMode::Default => ResolveSpec::DefaultBranch,
        LockMode::Ref { ref_name } => ResolveSpec::Ref(ref_name.clone()),
        LockMode::Exact => {
            return Err(AppError::MissingExactCommit {
                alias: entry.alias.clone(),
            }
            .into());
        }
    };
    Ok(context.git.resolve_spec(&remote_dir, spec)?)
}

fn prune_leftover_links(
    root: &ProjectRoot,
    keep: &BTreeSet<String>,
    report: &mut RunReport,
) -> Result<()> {
    for entry in fs::read_dir(&root.grepo_dir).map_err(|source| AppError::ReadDir {
        path: root.grepo_dir.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| AppError::ReadDirEntry {
            path: root.grepo_dir.clone(),
            source,
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_managed_symlink_name(name) || keep.contains(name) {
            continue;
        }

        if fs::symlink_metadata(&path)
            .map_err(|source| AppError::Metadata {
                path: path.clone(),
                source,
            })?
            .file_type()
            .is_symlink()
        {
            remove_managed_symlink(&path)?;
            report.stdout_line(format!("removed {}", path.display()));
        }
    }
    Ok(())
}

fn append_gc_report(report: &mut RunReport, gc_report: &GcReport) {
    for path in &gc_report.removed_snapshots {
        report.stdout_line(format!("deleted snapshot {}", path.display()));
    }
    for path in &gc_report.removed_remotes {
        report.stdout_line(format!("deleted remote {}", path.display()));
    }
    for path in &gc_report.removed_roots {
        report.stdout_line(format!("deleted stale root {}", path.display()));
    }
}

fn required_root(start: &Path) -> Result<ProjectRoot> {
    ProjectRoot::discover(start).ok_or_else(|| {
        AppError::NoProjectRoot {
            start: start.to_path_buf(),
        }
        .into()
    })
}

fn prepared_store(context: &AppContext) -> Result<Store> {
    let store = context.store();
    store.prepare()?;
    Ok(store)
}

fn validate_alias(alias: String) -> std::result::Result<String, AppError> {
    if is_valid_alias(&alias) {
        Ok(alias)
    } else {
        Err(AppError::InvalidAlias { alias })
    }
}

fn validate_aliases(aliases: Vec<String>) -> std::result::Result<Vec<String>, AppError> {
    aliases.into_iter().map(validate_alias).collect()
}

#[cfg(test)]
pub(crate) fn run_for_test(
    cwd: PathBuf,
    cache_root: PathBuf,
    state_root: PathBuf,
    git_program: OsString,
    args: &[&str],
) -> Result<RunReport> {
    let cli = Cli::try_parse_from(
        std::iter::once(OsString::from("grepo"))
            .chain(args.iter().map(OsString::from))
            .collect::<Vec<_>>(),
    )?;
    let command = Command::try_from(cli.command)?;
    let context = AppContext {
        cwd,
        cache_root,
        state_root,
        git: Git::new(git_program),
    };
    command.run(&context)
}
