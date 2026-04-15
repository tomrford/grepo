use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::git::{Git, ResolveSpec};
use crate::manifest::{LockEntry, Lockfile, TrackMode};
use crate::mutation_lock::MutationLock;
use crate::store::{GcReport, Store, remove_managed_symlink, replace_symlink};
use crate::util::{
    Result, cache_root, current_dir, ensure_dir, err, is_valid_alias, state_root, write_atomic,
};

pub fn main_entry() -> ExitCode {
    match run_env(std::env::args_os().collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_env(args: Vec<OsString>) -> Result<ExitCode> {
    let command = Command::parse(args)?;
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
            return Err(err(format!(
                "cannot initialize grepo root because {} is not a directory",
                grepo_dir.display()
            )));
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
    Remove { aliases: Vec<String> },
    Sync,
    Update { aliases: Vec<String> },
    Gc,
    Help,
}

#[derive(Clone, Debug)]
struct AddArgs {
    alias: String,
    url: String,
    ref_name: Option<String>,
    commit: Option<String>,
}

impl Command {
    fn parse(args: Vec<OsString>) -> Result<Self> {
        let mut parts = args.into_iter();
        let _program = parts.next();
        let Some(command) = parts.next() else {
            return Ok(Self::Help);
        };

        match command.to_string_lossy().as_ref() {
            "init" => {
                if parts.next().is_some() {
                    return Err(err("unexpected extra arguments for `grepo init`"));
                }
                Ok(Self::Init)
            }
            "add" => {
                let alias = required_arg(parts.next(), "alias for `grepo add`")?;
                let url = required_arg(parts.next(), "url for `grepo add`")?;
                if !is_valid_alias(&alias) {
                    return Err(err(format!("invalid alias: {alias}")));
                }

                let mut ref_name = None;
                let mut commit = None;
                while let Some(flag) = parts.next() {
                    match flag.to_string_lossy().as_ref() {
                        "--ref" => {
                            ref_name = Some(required_arg(parts.next(), "value for `--ref`")?)
                        }
                        "--commit" => {
                            commit = Some(required_arg(parts.next(), "value for `--commit`")?)
                        }
                        other => {
                            return Err(err(format!(
                                "unexpected argument for `grepo add`: {other}"
                            )));
                        }
                    }
                }

                if ref_name.is_some() && commit.is_some() {
                    return Err(err(
                        "`grepo add` accepts either `--ref` or `--commit`, not both",
                    ));
                }

                Ok(Self::Add(AddArgs {
                    alias,
                    url,
                    ref_name,
                    commit,
                }))
            }
            "remove" => {
                let aliases = collect_aliases(parts, "grepo remove")?;
                Ok(Self::Remove { aliases })
            }
            "sync" => {
                if parts.next().is_some() {
                    return Err(err("unexpected extra arguments for `grepo sync`"));
                }
                Ok(Self::Sync)
            }
            "update" => Ok(Self::Update {
                aliases: collect_optional_aliases(parts)?,
            }),
            "gc" => {
                if parts.next().is_some() {
                    return Err(err("unexpected extra arguments for `grepo gc`"));
                }
                Ok(Self::Gc)
            }
            "--help" | "-h" | "help" => Ok(Self::Help),
            other => Err(err(format!("unknown command: {other}"))),
        }
    }

    fn run(self, context: &AppContext) -> Result<ExitCode> {
        match self {
            Self::Init => init(context),
            Self::Add(args) => add(context, args),
            Self::Remove { aliases } => remove(context, &aliases),
            Self::Sync => sync(context),
            Self::Update { aliases } => update(context, &aliases),
            Self::Gc => gc(context),
            Self::Help => {
                print_help();
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}

fn init(context: &AppContext) -> Result<ExitCode> {
    let root = ProjectRoot::create_at(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    store.refresh_root(&context.git, &root.lock_path)?;
    println!("initialized {}", root.grepo_dir.display());
    Ok(ExitCode::SUCCESS)
}

fn add(context: &AppContext, args: AddArgs) -> Result<ExitCode> {
    let root = match ProjectRoot::discover(&context.cwd) {
        Some(root) => root,
        None => ProjectRoot::create_at(&context.cwd)?,
    };
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;

    let mut lockfile = root.load_lockfile()?;
    let mut entry = LockEntry::new(args.alias.clone(), args.url);
    if let Some(ref_name) = args.ref_name {
        entry.ref_name = Some(ref_name);
    } else if let Some(commit) = args.commit {
        entry.commit = Some(commit);
        entry.track = TrackMode::Pinned;
    } else {
        entry.track = TrackMode::DefaultBranch;
    }

    let (entry, snapshot_path) = realize_entry(context, &store, &entry, false)?;
    lockfile.upsert(entry);
    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;
    replace_symlink(&root.grepo_dir.join(&args.alias), &snapshot_path)?;
    println!("synced {} -> {}", args.alias, snapshot_path.display());
    Ok(ExitCode::SUCCESS)
}

fn remove(context: &AppContext, aliases: &[String]) -> Result<ExitCode> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;

    for alias in aliases {
        if !lockfile.remove(alias) {
            return Err(err(format!("alias not found: {alias}")));
        }
    }

    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;

    let mut failed = false;
    for alias in aliases {
        if let Err(error) = remove_managed_symlink(&root.grepo_dir.join(alias)) {
            failed = true;
            eprintln!("warning: {error}");
        }
    }

    if failed {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn sync(context: &AppContext) -> Result<ExitCode> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;
    let mut failed = false;
    let mut dirty_lock = false;

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
                println!("synced {} -> {}", alias, snapshot_path.display());
            }
            Err(error) => {
                failed = true;
                eprintln!("warning: failed to sync {}: {error}", alias);
            }
        }
    }

    let keep = lockfile.aliases().into_iter().collect::<BTreeSet<_>>();
    remove_leftover_links(&root, &keep)?;
    if dirty_lock {
        lockfile.write(&root.lock_path)?;
    }
    store.refresh_root(&context.git, &root.lock_path)?;

    if failed {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn update(context: &AppContext, aliases: &[String]) -> Result<ExitCode> {
    let root = required_root(&context.cwd)?;
    let _lock = root.lock_mutation()?;
    let store = prepared_store(context)?;
    let mut lockfile = root.load_lockfile()?;
    let selected = lockfile.select_aliases(aliases)?;
    let mut failed = false;

    for alias in selected {
        let Some(entry) = lockfile.get(&alias).cloned() else {
            continue;
        };
        match realize_entry(context, &store, &entry, true) {
            Ok((updated_entry, snapshot_path)) => {
                lockfile.upsert(updated_entry);
                replace_symlink(&root.grepo_dir.join(&alias), &snapshot_path)?;
                println!("updated {} -> {}", alias, snapshot_path.display());
            }
            Err(error) => {
                failed = true;
                eprintln!("warning: failed to update {}: {error}", alias);
            }
        }
    }

    lockfile.write(&root.lock_path)?;
    store.refresh_root(&context.git, &root.lock_path)?;
    if failed {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn gc(context: &AppContext) -> Result<ExitCode> {
    let store = prepared_store(context)?;
    let report = store.gc(&context.git)?;
    print_gc_report(&report);
    Ok(ExitCode::SUCCESS)
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
            return Err(err(format!(
                "alias {} is pinned but has no commit",
                updated.alias
            )));
        }
    }

    let commit = updated
        .commit
        .as_deref()
        .ok_or_else(|| err(format!("alias {} has no commit", updated.alias)))?;
    let snapshot_path = store.ensure_snapshot_for_commit(&context.git, &updated.url, commit)?;
    Ok((updated, snapshot_path))
}

fn resolve_tracking_commit(
    context: &AppContext,
    store: &Store,
    entry: &LockEntry,
) -> Result<String> {
    let remote_dir = store.ensure_remote_cache(&context.git, &entry.url)?;
    let spec = match (
        &entry.track,
        entry.ref_name.as_deref(),
        entry.commit.as_deref(),
    ) {
        (TrackMode::DefaultBranch, _, _) => ResolveSpec::DefaultBranch,
        (_, Some(ref_name), _) => ResolveSpec::Ref(ref_name.to_string()),
        (TrackMode::Pinned, _, Some(commit)) => ResolveSpec::Commit(commit.to_string()),
        (TrackMode::Pinned, _, None) => {
            return Err(err(format!(
                "alias {} is pinned but missing a commit",
                entry.alias
            )));
        }
    };
    context.git.resolve_spec(&remote_dir, spec)
}

fn remove_leftover_links(root: &ProjectRoot, keep: &BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(&root.grepo_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') || keep.contains(name) {
            continue;
        }

        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            remove_managed_symlink(&path)?;
            println!("removed {}", path.display());
        }
    }
    Ok(())
}

fn print_gc_report(report: &GcReport) {
    for path in &report.removed_snapshots {
        println!("deleted snapshot {}", path.display());
    }
    for path in &report.removed_remotes {
        println!("deleted remote {}", path.display());
    }
    for path in &report.removed_roots {
        println!("deleted stale root {}", path.display());
    }
}

fn required_root(start: &Path) -> Result<ProjectRoot> {
    ProjectRoot::discover(start)
        .ok_or_else(|| err(format!("no grepo root found from {}", start.display())))
}

fn prepared_store(context: &AppContext) -> Result<Store> {
    let store = context.store();
    store.prepare()?;
    Ok(store)
}

fn collect_aliases(parts: impl Iterator<Item = OsString>, command: &str) -> Result<Vec<String>> {
    let aliases = collect_optional_aliases(parts)?;
    if aliases.is_empty() {
        return Err(err(format!("missing alias names for `{command}`")));
    }
    Ok(aliases)
}

fn collect_optional_aliases(parts: impl Iterator<Item = OsString>) -> Result<Vec<String>> {
    let mut aliases = Vec::new();
    for part in parts {
        let alias = part.to_string_lossy().into_owned();
        if !is_valid_alias(&alias) {
            return Err(err(format!("invalid alias: {alias}")));
        }
        aliases.push(alias);
    }
    Ok(aliases)
}

fn print_help() {
    println!(
        "\
grepo

Usage:
  grepo init
  grepo add <alias> <url> [--ref <ref> | --commit <commit>]
  grepo remove <alias>...
  grepo sync
  grepo update [alias...]
  grepo gc"
    );
}

fn required_arg(value: Option<OsString>, description: &str) -> Result<String> {
    value
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or_else(|| err(format!("missing {description}")))
}

#[cfg(test)]
pub(crate) fn run_for_test(
    cwd: PathBuf,
    cache_root: PathBuf,
    state_root: PathBuf,
    git_program: OsString,
    args: &[&str],
) -> Result<ExitCode> {
    let mut argv = vec![OsString::from("grepo")];
    argv.extend(args.iter().map(OsString::from));
    let command = Command::parse(argv)?;
    let context = AppContext {
        cwd,
        cache_root,
        state_root,
        git: Git::new(git_program),
    };
    command.run(&context)
}
