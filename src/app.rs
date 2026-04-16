use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Command as CliCommand, GcArgs, RemoveArgs, UpdateArgs};
use crate::error::{GrepoError, Result};
use crate::git::{Git, ResolveSpec, validate_commit_oid, validate_ref_name};
use crate::manifest::{LockEntry, LockMode, Lockfile};
use crate::mutation_lock::MutationLock;
use crate::output::RunReport;
use crate::store::{
    GcReport, Store, is_managed_symlink_name, read_dir_paths, remove_managed_symlink,
    replace_symlink, symlink_metadata_if_exists,
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
            return Err(GrepoError::RootPathNotDirectory(grepo_dir));
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
        Lockfile::load(&self.lock_path)
    }

    fn lock_mutation(&self) -> Result<MutationLock> {
        MutationLock::acquire(&self.grepo_dir)
    }
}

#[derive(Clone, Debug)]
enum Command {
    Init,
    Add(AddArgs),
    List,
    Remove { aliases: Vec<String> },
    Sync,
    Update { aliases: Vec<String> },
    Gc { verbose: bool },
    Skill,
}

#[derive(Clone, Debug)]
struct AddArgs {
    alias: String,
    url: String,
    ref_name: Option<String>,
    commit: Option<String>,
    force: bool,
}

#[derive(Clone, Debug)]
struct RealizedAlias {
    entry: LockEntry,
    snapshot_path: PathBuf,
}

impl TryFrom<CliCommand> for Command {
    type Error = GrepoError;

    fn try_from(value: CliCommand) -> Result<Self> {
        match value {
            CliCommand::Init => Ok(Self::Init),
            CliCommand::Add(args) => Ok(Self::Add(AddArgs {
                alias: validate_alias(args.alias)?,
                url: args.url,
                ref_name: validate_optional_ref(args.ref_name)?,
                commit: validate_optional_commit(args.commit)?,
                force: args.force,
            })),
            CliCommand::List => Ok(Self::List),
            CliCommand::Remove(RemoveArgs { aliases }) => Ok(Self::Remove {
                aliases: validate_aliases(aliases)?,
            }),
            CliCommand::Sync => Ok(Self::Sync),
            CliCommand::Update(UpdateArgs { aliases }) => Ok(Self::Update {
                aliases: validate_aliases(aliases)?,
            }),
            CliCommand::Gc(GcArgs { verbose }) => Ok(Self::Gc { verbose }),
            CliCommand::Skill => Ok(Self::Skill),
        }
    }
}

impl Command {
    fn run(self, context: &AppContext) -> Result<RunReport> {
        match self {
            Self::Init => init(context),
            Self::Add(args) => add(context, args),
            Self::List => list(context),
            Self::Remove { aliases } => remove(context, &aliases),
            Self::Sync => sync(context),
            Self::Update { aliases } => update(context, &aliases),
            Self::Gc { verbose } => gc(context, verbose),
            Self::Skill => skill(),
        }
    }
}

const SKILL_MD: &str = include_str!("../skill/grepo/SKILL.md");

fn skill() -> Result<RunReport> {
    let mut report = RunReport::success();
    report.stdout_line(SKILL_MD.trim_end());
    Ok(report)
}

fn init(context: &AppContext) -> Result<RunReport> {
    let existed = context.cwd.join("grepo/.lock").is_file();
    let root = ProjectRoot::create_at(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let _store_lock = store.lock_mutation()?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut report = RunReport::success();
    let status = if existed {
        "already initialized"
    } else {
        "initialized"
    };
    report.stdout_line(format!("{status} {}", root.grepo_dir.display()));
    Ok(report)
}

fn add(context: &AppContext, args: AddArgs) -> Result<RunReport> {
    let root = match ProjectRoot::discover(&context.cwd) {
        Some(root) => root,
        None => ProjectRoot::create_at(&context.cwd)?,
    };
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let _store_lock = store.lock_mutation()?;

    let mut lockfile = root.load_lockfile()?;
    let existing = lockfile.get(&args.alias).cloned();
    if existing.is_some() && !args.force {
        return Err(GrepoError::AliasExists(args.alias));
    }
    let mut entry = LockEntry::new(args.alias.clone(), args.url);
    if let Some(ref_name) = args.ref_name {
        entry.mode = LockMode::Ref { ref_name };
    } else if let Some(commit) = args.commit {
        entry.commit = Some(commit);
        entry.mode = LockMode::Exact;
    } else {
        entry.mode = LockMode::Default;
    }

    let realized = realize_entry(context, &store, &entry, false)?;
    apply_realized_alias(&root, &mut lockfile, existing.as_ref(), &realized)?;
    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut report = RunReport::success();
    let verb = if existing.is_some() {
        "replaced"
    } else {
        "added"
    };
    report.stdout_line(format!(
        "{verb} {} -> {}",
        realized.entry.alias,
        realized.snapshot_path.display()
    ));
    Ok(report)
}

fn list(context: &AppContext) -> Result<RunReport> {
    let root = required_root(&context.cwd)?;
    let lockfile = root.load_lockfile()?;
    let mut report = RunReport::success();
    let rendered = render_list(&lockfile);
    if !rendered.is_empty() {
        report.stdout_line(rendered);
    }
    Ok(report)
}

fn remove(context: &AppContext, aliases: &[String]) -> Result<RunReport> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let _store_lock = store.lock_mutation()?;
    let mut lockfile = root.load_lockfile()?;
    let selected = lockfile.select_aliases(aliases)?;

    for alias in &selected {
        let removed = lockfile.remove(alias);
        debug_assert!(removed, "selected alias should exist in lockfile");
    }

    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut report = RunReport::success();
    for alias in &selected {
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
    let _store_lock = store.lock_mutation()?;
    let mut lockfile = root.load_lockfile()?;
    let mut dirty_lock = false;
    let mut report = RunReport::success();

    for alias in lockfile.aliases() {
        let Some(entry) = lockfile.get(&alias).cloned() else {
            continue;
        };
        match realize_entry(context, &store, &entry, false) {
            Ok(realized) => {
                match apply_realized_alias(&root, &mut lockfile, Some(&entry), &realized) {
                    Ok(()) => {
                        if realized.entry != entry {
                            dirty_lock = true;
                        }
                        report.stdout_line(format!(
                            "synced {} -> {}",
                            realized.entry.alias,
                            realized.snapshot_path.display()
                        ));
                    }
                    Err(error) => report.warn_line(format!("failed to sync {alias}: {error}")),
                }
            }
            Err(error) => report.warn_line(format!("failed to sync {alias}: {error}")),
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
    let _store_lock = store.lock_mutation()?;
    let mut lockfile = root.load_lockfile()?;
    let selected = lockfile.select_aliases(aliases)?;
    let explicit_aliases = !aliases.is_empty();
    let mut report = RunReport::success();
    let mut dirty_lock = false;

    for alias in selected {
        let Some(entry) = lockfile.get(&alias).cloned() else {
            continue;
        };
        if !entry.can_update() {
            if explicit_aliases {
                report.stdout_line(format!("skipped {alias}: exact pin"));
            }
            continue;
        }
        match realize_entry(context, &store, &entry, true) {
            Ok(realized) => {
                match apply_realized_alias(&root, &mut lockfile, Some(&entry), &realized) {
                    Ok(()) => {
                        if realized.entry != entry {
                            dirty_lock = true;
                        }
                        report.stdout_line(format!(
                            "updated {} -> {}",
                            realized.entry.alias,
                            realized.snapshot_path.display()
                        ));
                    }
                    Err(error) => report.warn_line(format!("failed to update {alias}: {error}")),
                }
            }
            Err(error) => report.warn_line(format!("failed to update {alias}: {error}")),
        }
    }

    if dirty_lock {
        lockfile.write(&root.lock_path)?;
        store.refresh_root(&context.git, &root.lock_path)?;
    }
    Ok(report)
}

fn gc(context: &AppContext, verbose: bool) -> Result<RunReport> {
    let store = prepared_store(context)?;
    let _store_lock = store.lock_mutation()?;
    let gc_report = store.gc(&context.git)?;
    let mut report = RunReport::success();
    append_gc_report(&mut report, &gc_report, verbose);
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
) -> Result<RealizedAlias> {
    validate_git_entry(entry)?;
    let mut updated = entry.clone();
    if refresh_tracking || updated.commit.is_none() {
        if updated.can_update() {
            updated.commit = Some(resolve_tracking_commit(context, store, &updated)?);
        } else if updated.commit.is_none() {
            return Err(GrepoError::MissingCommit(updated.alias.clone()));
        }
    }

    let commit = updated
        .commit
        .as_deref()
        .ok_or_else(|| GrepoError::MissingCommit(updated.alias.clone()))?;
    let snapshot_path = store.ensure_snapshot_for_commit(&context.git, &updated.url, commit)?;
    Ok(RealizedAlias {
        entry: updated,
        snapshot_path,
    })
}

fn resolve_tracking_commit(
    context: &AppContext,
    store: &Store,
    entry: &LockEntry,
) -> Result<String> {
    let spec = match &entry.mode {
        LockMode::Default => ResolveSpec::DefaultBranch,
        LockMode::Ref { ref_name } => ResolveSpec::Ref(ref_name.clone()),
        LockMode::Exact => return Err(GrepoError::MissingCommit(entry.alias.clone())),
    };
    store.with_remote_cache(&context.git, &entry.url, |remote_dir| {
        context.git.resolve_spec(remote_dir, spec)
    })
}

fn prune_leftover_links(
    root: &ProjectRoot,
    keep: &BTreeSet<String>,
    report: &mut RunReport,
) -> Result<()> {
    for path in read_dir_paths(&root.grepo_dir)? {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_managed_symlink_name(name) || keep.contains(name) {
            continue;
        }

        if symlink_metadata_if_exists(&path)?
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        {
            remove_managed_symlink(&path)?;
            report.stdout_line(format!("removed {}", path.display()));
        }
    }
    Ok(())
}

fn append_gc_report(report: &mut RunReport, gc_report: &GcReport, verbose: bool) {
    let total_deleted = gc_report.removed_snapshots.len()
        + gc_report.removed_remotes.len()
        + gc_report.removed_roots.len();
    if total_deleted > 0 {
        report.stdout_line(format!(
            "deleted {}, {}, {}",
            format_count(gc_report.removed_snapshots.len(), "snapshot"),
            format_count(gc_report.removed_remotes.len(), "remote"),
            format_count(gc_report.removed_roots.len(), "stale root")
        ));
    }
    for warning in &gc_report.warnings {
        report.warn_line(warning.clone());
    }
    if !verbose {
        return;
    }
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

fn apply_realized_alias(
    root: &ProjectRoot,
    lockfile: &mut Lockfile,
    previous: Option<&LockEntry>,
    realized: &RealizedAlias,
) -> Result<()> {
    replace_symlink(
        &root.grepo_dir.join(&realized.entry.alias),
        &realized.snapshot_path,
    )?;
    if previous != Some(&realized.entry) {
        lockfile.upsert(realized.entry.clone());
    }
    Ok(())
}

fn render_list(lockfile: &Lockfile) -> String {
    let mut sections = Vec::new();
    for entry in lockfile.entries() {
        let mut lines = vec![
            format!("[repos.{}]", entry.alias),
            format!("url = {:?}", entry.url),
        ];
        match &entry.mode {
            LockMode::Default => lines.push("mode = \"default\"".to_string()),
            LockMode::Ref { ref_name } => {
                lines.push("mode = \"ref\"".to_string());
                lines.push(format!("ref = {:?}", ref_name));
            }
            LockMode::Exact => {
                lines.push("mode = \"exact\"".to_string());
                if let Some(commit) = &entry.commit {
                    lines.push(format!("commit = {:?}", commit));
                }
            }
        }
        sections.push(lines.join("\n"));
    }
    sections.join("\n\n")
}

fn format_count(count: usize, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{suffix}")
}

fn required_root(start: &Path) -> Result<ProjectRoot> {
    ProjectRoot::discover(start).ok_or_else(|| GrepoError::NoProjectRoot(start.to_path_buf()))
}

fn prepared_store(context: &AppContext) -> Result<Store> {
    let store = context.store();
    store.prepare()?;
    Ok(store)
}

fn validate_alias(alias: String) -> Result<String> {
    if is_valid_alias(&alias) {
        Ok(alias)
    } else {
        Err(GrepoError::InvalidAlias(alias))
    }
}

fn validate_aliases(aliases: Vec<String>) -> Result<Vec<String>> {
    aliases.into_iter().map(validate_alias).collect()
}

fn validate_optional_ref(ref_name: Option<String>) -> Result<Option<String>> {
    ref_name
        .map(|value| {
            validate_ref_name(&value)?;
            Ok(value)
        })
        .transpose()
}

fn validate_optional_commit(commit: Option<String>) -> Result<Option<String>> {
    commit
        .map(|value| {
            validate_commit_oid(&value)?;
            Ok(value)
        })
        .transpose()
}

fn validate_git_entry(entry: &LockEntry) -> Result<()> {
    match &entry.mode {
        LockMode::Default => {}
        LockMode::Ref { ref_name } => validate_ref_name(ref_name)?,
        LockMode::Exact => {}
    }
    if let Some(commit) = &entry.commit {
        validate_commit_oid(commit)?;
    }
    Ok(())
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
