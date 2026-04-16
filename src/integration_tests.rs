use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

use fs4::fs_std::FileExt;

use crate::app::run_for_test;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir();
        let path = base.join(format!(
            "grepo-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn add_creates_root_and_eagerly_syncs_default_branch() {
    let root = TestDir::new("add");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);
    assert_eq!(report.stdout().len(), 1);
    assert!(report.stdout()[0].starts_with("added docs -> "));

    let lockfile = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert!(lockfile.contains("[repos.docs]"));
    assert!(lockfile.contains("mode = \"default\""));
    assert!(lockfile.contains("commit = "));

    let link = workspace.join("grepo/docs");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let resolved = fs::canonicalize(link).unwrap();
    assert_eq!(
        fs::read_to_string(resolved.join("README.md")).unwrap(),
        "hello\n"
    );
}

#[test]
fn init_reports_existing_root_on_second_run() {
    let root = TestDir::new("init-existing");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    let first = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["init"],
    )
    .unwrap();
    assert_eq!(
        first.stdout(),
        &[format!("initialized {}", workspace.join("grepo").display())]
    );

    let second = run_for_test(workspace, cache_root, state_root, "git".into(), &["init"]).unwrap();
    assert_eq!(
        second.stdout(),
        &[format!(
            "already initialized {}",
            root.path.join("workspace/grepo").display()
        )]
    );
}

#[test]
fn skill_prints_embedded_skill_markdown() {
    let root = TestDir::new("skill");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    let report = run_for_test(workspace, cache_root, state_root, "git".into(), &["skill"]).unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert_eq!(
        report.stdout(),
        &[include_str!("../skill/grepo/SKILL.md")
            .trim_end()
            .to_string()]
    );
    assert!(report.stderr().is_empty());
}

#[test]
fn add_does_not_write_lockfile_when_alias_path_collides() {
    let root = TestDir::new("add-collision");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(workspace.join("grepo/docs")).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let error = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap_err();
    assert_eq!(
        format!("{error}"),
        format!(
            "path collision at {}: expected a symlink managed by grepo",
            workspace.join("grepo/docs").display()
        )
    );

    let lockfile = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert!(!lockfile.contains("[repos.docs]"));
    assert!(workspace.join("grepo/docs").is_dir());
}

#[test]
fn add_rejects_existing_alias_without_force_and_force_replaces_it() {
    let root = TestDir::new("add-force");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote_a = root.path.join("a.git");
    let seed_a = root.path.join("seed-a");
    let remote_b = root.path.join("b.git");
    let seed_b = root.path.join("seed-b");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote_a, &seed_a, "README.md", "a\n");
    seed_remote_repo(&remote_b, &seed_b, "README.md", "b\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote_a.to_str().unwrap()],
    )
    .unwrap();

    let error = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote_b.to_str().unwrap()],
    )
    .unwrap_err();
    assert_eq!(
        format!("{error}"),
        "alias already exists: docs (use --force to replace)"
    );
    assert_eq!(
        fs::read_to_string(
            fs::canonicalize(workspace.join("grepo/docs"))
                .unwrap()
                .join("README.md")
        )
        .unwrap(),
        "a\n"
    );

    let report = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["add", "docs", remote_b.to_str().unwrap(), "--force"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert_eq!(report.stdout().len(), 1);
    assert!(report.stdout()[0].starts_with("replaced docs -> "));
    assert_eq!(
        fs::read_to_string(
            fs::canonicalize(workspace.join("grepo/docs"))
                .unwrap()
                .join("README.md")
        )
        .unwrap(),
        "b\n"
    );
}

#[test]
fn update_specific_alias_changes_only_targeted_entry() {
    let root = TestDir::new("update");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote_a = root.path.join("a.git");
    let seed_a = root.path.join("seed-a");
    let remote_b = root.path.join("b.git");
    let seed_b = root.path.join("seed-b");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote_a, &seed_a, "a.txt", "v1\n");
    seed_remote_repo(&remote_b, &seed_b, "b.txt", "v1\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "a", remote_a.to_str().unwrap()],
    )
    .unwrap();
    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "b", remote_b.to_str().unwrap()],
    )
    .unwrap();

    fs::write(seed_a.join("a.txt"), "v2\n").unwrap();
    git(Some(&seed_a), &["add", "a.txt"]);
    git(
        Some(&seed_a),
        &[
            "-c",
            "user.name=grepo",
            "-c",
            "user.email=grepo@example.com",
            "commit",
            "-m",
            "update",
        ],
    );
    git(Some(&seed_a), &["push"]);

    let before_b = fs::read_to_string(
        fs::canonicalize(workspace.join("grepo/b"))
            .unwrap()
            .join("b.txt"),
    )
    .unwrap();
    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["update", "a"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);

    let after_a = fs::read_to_string(
        fs::canonicalize(workspace.join("grepo/a"))
            .unwrap()
            .join("a.txt"),
    )
    .unwrap();
    let after_b = fs::read_to_string(
        fs::canonicalize(workspace.join("grepo/b"))
            .unwrap()
            .join("b.txt"),
    )
    .unwrap();
    assert_eq!(after_a, "v2\n");
    assert_eq!(before_b, after_b);
}

#[test]
fn sync_warns_on_path_collision_and_continues_other_aliases() {
    let root = TestDir::new("sync-collision");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote_a = root.path.join("a.git");
    let seed_a = root.path.join("seed-a");
    let remote_b = root.path.join("b.git");
    let seed_b = root.path.join("seed-b");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote_a, &seed_a, "a.txt", "a\n");
    seed_remote_repo(&remote_b, &seed_b, "b.txt", "b\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "a", remote_a.to_str().unwrap()],
    )
    .unwrap();
    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "b", remote_b.to_str().unwrap()],
    )
    .unwrap();

    let collision_path = workspace.join("grepo/a");
    fs::remove_file(&collision_path).unwrap();
    fs::create_dir(&collision_path).unwrap();

    let report = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["sync"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::from(1));
    assert_eq!(
        report.stderr(),
        &[format!(
            "warning: failed to sync a: path collision at {}: expected a symlink managed by grepo",
            collision_path.display()
        )]
    );
    assert!(
        report
            .stdout()
            .iter()
            .any(|line| line.starts_with("synced b -> "))
    );
}

#[test]
fn gc_prunes_unreachable_snapshots_and_remotes_from_rooted_lockfiles() {
    let root = TestDir::new("gc");
    let workspace = root.path.join("workspace");
    let nested = workspace.join("nested");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&nested).unwrap();

    seed_remote_repo(&remote, &seed, "file.txt", "v1\n");

    run_for_test(
        nested.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();

    let rooted_snapshot = fs::canonicalize(nested.join("grepo/docs")).unwrap();
    let remote_key_dir = rooted_snapshot.parent().unwrap().to_path_buf();
    let remote_cache = cache_root.join("remotes").join(format!(
        "{}.git",
        remote_key_dir.file_name().unwrap().to_string_lossy()
    ));

    let stale_snapshot = cache_root.join("snapshots/stale-url/stale-snapshot");
    fs::create_dir_all(stale_snapshot.join("nested")).unwrap();
    fs::write(stale_snapshot.join("nested/file.txt"), "stale\n").unwrap();
    let stale_remote = cache_root.join("remotes/stale.git");
    fs::create_dir_all(&stale_remote).unwrap();

    let report = run_for_test(
        root.path.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["gc"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);
    assert_eq!(
        report.stdout(),
        &["deleted 1 snapshot, 1 remote, 0 stale roots".to_string()]
    );
    assert!(rooted_snapshot.exists());
    assert!(remote_cache.exists());
    assert!(!stale_snapshot.exists());
    assert!(!stale_remote.exists());
}

#[test]
fn gc_verbose_includes_summary_and_paths() {
    let root = TestDir::new("gc-verbose");
    let workspace = root.path.join("workspace");
    let nested = workspace.join("nested");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&nested).unwrap();

    seed_remote_repo(&remote, &seed, "file.txt", "v1\n");

    run_for_test(
        nested.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();

    let stale_snapshot = cache_root.join("snapshots/stale-url/stale-snapshot");
    fs::create_dir_all(stale_snapshot.join("nested")).unwrap();
    fs::write(stale_snapshot.join("nested/file.txt"), "stale\n").unwrap();
    let stale_remote = cache_root.join("remotes/stale.git");
    fs::create_dir_all(&stale_remote).unwrap();

    let report = run_for_test(
        root.path.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["gc", "--verbose"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert_eq!(
        report.stdout()[0],
        "deleted 1 snapshot, 1 remote, 0 stale roots"
    );
    assert!(
        report
            .stdout()
            .iter()
            .any(|line| line.starts_with("deleted snapshot "))
    );
    assert!(
        report
            .stdout()
            .iter()
            .any(|line| line.starts_with("deleted remote "))
    );
}

#[test]
fn gc_warns_and_skips_unreadable_root_lockfiles() {
    let root = TestDir::new("gc-bad-root");
    let workspace = root.path.join("workspace");
    let nested = workspace.join("nested");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    let broken_project = root.path.join("broken-project");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(broken_project.join("grepo")).unwrap();

    seed_remote_repo(&remote, &seed, "file.txt", "v1\n");

    run_for_test(
        nested.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();

    let broken_lock = broken_project.join("grepo/.lock");
    fs::write(
        &broken_lock,
        r#"[repos.bad]
url = "git@example.com:bad/repo.git"
track = "default"
commit = "deadbeef"
"#,
    )
    .unwrap();
    let bad_root = state_root.join("roots/bad.lock");
    symlink(&broken_lock, &bad_root).unwrap();

    let report = run_for_test(
        root.path.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["gc"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::from(1));
    assert_eq!(report.stderr().len(), 1);
    assert!(report.stderr()[0].starts_with("warning: skipped rooted lockfile "));
    assert!(report.stderr()[0].contains(".lock: invalid grepo/.lock TOML:"));
    assert!(report.stderr()[0].contains("missing field `mode`"));
}

#[test]
fn list_omits_tracking_commits_but_keeps_exact_pins() {
    let root = TestDir::new("list");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(workspace.join("grepo")).unwrap();
    fs::write(
        workspace.join("grepo/.lock"),
        r#"[repos.default_branch]
url = "git@example.com:org/default.git"
mode = "default"
commit = "1111111111111111111111111111111111111111"

[repos.named_ref]
url = "git@example.com:org/ref.git"
mode = "ref"
ref = "main"
commit = "2222222222222222222222222222222222222222"

[repos.exact_pin]
url = "git@example.com:org/exact.git"
mode = "exact"
commit = "3333333333333333333333333333333333333333"
"#,
    )
    .unwrap();

    let report = run_for_test(workspace, cache_root, state_root, "git".into(), &["list"]).unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert_eq!(report.stdout().len(), 1);
    assert_eq!(
        report.stdout()[0],
        r#"[repos.default_branch]
url = "git@example.com:org/default.git"
mode = "default"

[repos.exact_pin]
url = "git@example.com:org/exact.git"
mode = "exact"
commit = "3333333333333333333333333333333333333333"

[repos.named_ref]
url = "git@example.com:org/ref.git"
mode = "ref"
ref = "main""#
    );
}

#[test]
fn remove_deletes_dangling_managed_symlink() {
    let root = TestDir::new("remove-dangling");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");
    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();

    let link = workspace.join("grepo/docs");
    let snapshot = fs::canonicalize(&link).unwrap();
    make_tree_writable(&snapshot);
    fs::remove_dir_all(&snapshot).unwrap();

    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["remove", "docs"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);
    assert_eq!(report.stdout(), &["removed docs".to_string()]);
    assert!(!link.exists());
    assert!(
        !fs::symlink_metadata(&link)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    );
}

#[test]
fn sync_prunes_dangling_symlinks_in_tool_owned_dir() {
    let root = TestDir::new("sync-dangling");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["init"],
    )
    .unwrap();

    let link = workspace.join("grepo/manual");
    symlink(workspace.join("missing"), &link).unwrap();

    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["sync"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);
    assert!(
        !fs::symlink_metadata(&link)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    );
}

#[test]
fn add_rejects_leading_dot_aliases() {
    let root = TestDir::new("dot-alias");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let error = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", ".lock", remote.to_str().unwrap()],
    )
    .unwrap_err();
    assert_eq!(format!("{error}"), "invalid alias: .lock");
    assert!(!workspace.join("grepo").exists());
}

#[test]
fn add_rejects_refs_that_start_with_dash() {
    let root = TestDir::new("dash-ref");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let error = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &[
            "add",
            "docs",
            remote.to_str().unwrap(),
            "--ref=--upload-pack=/bin/echo",
        ],
    )
    .unwrap_err();
    assert_eq!(format!("{error}"), "invalid ref: --upload-pack=/bin/echo");
    assert!(!workspace.join("grepo").exists());
}

#[test]
fn add_rejects_non_oid_commit_values() {
    let root = TestDir::new("bad-commit");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let error = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["add", "docs", remote.to_str().unwrap(), "--commit=--orphan"],
    )
    .unwrap_err();
    assert_eq!(format!("{error}"), "invalid commit: --orphan");
    assert!(!workspace.join("grepo").exists());
}

#[test]
fn add_ref_named_default_is_not_ambiguous_in_lockfile() {
    let root = TestDir::new("named-default-ref");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");
    git(Some(&seed), &["checkout", "-b", "default"]);
    git(Some(&seed), &["push", "-u", "origin", "default"]);

    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap(), "--ref", "default"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), std::process::ExitCode::SUCCESS);

    let lockfile = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert!(lockfile.contains("mode = \"ref\""));
    assert!(lockfile.contains("ref = \"default\""));
    assert!(!lockfile.contains("mode = \"default\""));
}

#[test]
fn remove_reports_missing_alias_cleanly() {
    let root = TestDir::new("remove-missing");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["init"],
    )
    .unwrap();

    let error = run_for_test(
        workspace,
        cache_root,
        state_root,
        "git".into(),
        &["remove", "docs"],
    )
    .unwrap_err();
    assert_eq!(format!("{error}"), "alias not found: docs");
}

#[test]
fn remove_validates_full_alias_list_before_removing_anything() {
    let root = TestDir::new("remove-partial");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote_a = root.path.join("a.git");
    let seed_a = root.path.join("seed-a");
    let remote_b = root.path.join("b.git");
    let seed_b = root.path.join("seed-b");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote_a, &seed_a, "a.txt", "a\n");
    seed_remote_repo(&remote_b, &seed_b, "b.txt", "b\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "a", remote_a.to_str().unwrap()],
    )
    .unwrap();
    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "b", remote_b.to_str().unwrap()],
    )
    .unwrap();

    let error = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["remove", "a", "missing", "b"],
    )
    .unwrap_err();
    assert_eq!(format!("{error}"), "alias not found: missing");

    assert!(
        fs::symlink_metadata(workspace.join("grepo/a"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::symlink_metadata(workspace.join("grepo/b"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    let lockfile = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert!(lockfile.contains("[repos.a]"));
    assert!(lockfile.contains("[repos.b]"));
}

#[test]
fn sync_reports_busy_mutation_lock() {
    let root = TestDir::new("busy-lock");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["init"],
    )
    .unwrap();
    let lock_path = workspace.join("grepo/.mutate.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    file.try_lock_exclusive().unwrap();

    let error = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["sync"],
    )
    .unwrap_err();
    assert_eq!(
        format!("{error}"),
        format!(
            "another grepo command is already mutating {}",
            workspace.join("grepo").display()
        )
    );
}

#[test]
fn gc_reports_busy_shared_store_lock() {
    let root = TestDir::new("busy-store-lock");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["init"],
    )
    .unwrap();

    let lock_path = state_root.join("locks/store.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    file.try_lock_exclusive().unwrap();

    let error = run_for_test(
        workspace,
        cache_root,
        state_root.clone(),
        "git".into(),
        &["gc"],
    )
    .unwrap_err();
    assert_eq!(
        format!("{error}"),
        format!(
            "another grepo command is already mutating shared store {}",
            state_root.display()
        )
    );
}

#[test]
fn init_leaves_mutation_lock_file_empty() {
    let root = TestDir::new("empty-mutation-lock");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(&workspace).unwrap();

    run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["init"],
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(workspace.join("grepo/.mutate.lock")).unwrap(),
        ""
    );
}

#[test]
fn update_warns_on_path_collision_and_keeps_failed_alias_pinned_to_old_commit() {
    let root = TestDir::new("update-collision");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote_a = root.path.join("a.git");
    let seed_a = root.path.join("seed-a");
    let remote_b = root.path.join("b.git");
    let seed_b = root.path.join("seed-b");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote_a, &seed_a, "a.txt", "v1\n");
    seed_remote_repo(&remote_b, &seed_b, "b.txt", "v1\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "a", remote_a.to_str().unwrap()],
    )
    .unwrap();
    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "b", remote_b.to_str().unwrap()],
    )
    .unwrap();

    let lock_before = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    let commit_a_before =
        extract_lock_commit(&lock_before, "a").expect("existing commit for alias a");

    fs::write(seed_a.join("a.txt"), "v2\n").unwrap();
    git(Some(&seed_a), &["add", "a.txt"]);
    git(
        Some(&seed_a),
        &[
            "-c",
            "user.name=grepo",
            "-c",
            "user.email=grepo@example.com",
            "commit",
            "-m",
            "update-a",
        ],
    );
    git(Some(&seed_a), &["push"]);

    fs::write(seed_b.join("b.txt"), "v2\n").unwrap();
    git(Some(&seed_b), &["add", "b.txt"]);
    git(
        Some(&seed_b),
        &[
            "-c",
            "user.name=grepo",
            "-c",
            "user.email=grepo@example.com",
            "commit",
            "-m",
            "update-b",
        ],
    );
    git(Some(&seed_b), &["push"]);

    let collision_path = workspace.join("grepo/a");
    fs::remove_file(&collision_path).unwrap();
    fs::create_dir(&collision_path).unwrap();

    let report = run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["update"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::from(1));
    assert_eq!(
        report.stderr(),
        &[format!(
            "warning: failed to update a: path collision at {}: expected a symlink managed by grepo",
            collision_path.display()
        )]
    );
    assert!(
        report
            .stdout()
            .iter()
            .any(|line| line.starts_with("updated b -> "))
    );

    let lock_after = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert_eq!(extract_lock_commit(&lock_after, "a"), Some(commit_a_before));

    let updated_b = fs::read_to_string(
        fs::canonicalize(workspace.join("grepo/b"))
            .unwrap()
            .join("b.txt"),
    )
    .unwrap();
    assert_eq!(updated_b, "v2\n");
}

#[test]
fn update_explicit_exact_pin_reports_skip_instead_of_false_success() {
    let root = TestDir::new("update-exact");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");
    let commit = git_output(Some(&seed), &["rev-parse", "HEAD"]);

    run_for_test(
        workspace.clone(),
        cache_root,
        state_root,
        "git".into(),
        &["add", "docs", remote.to_str().unwrap(), "--commit", &commit],
    )
    .unwrap();

    let report = run_for_test(
        workspace,
        root.path.join("cache"),
        root.path.join("state"),
        "git".into(),
        &["update", "docs"],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert_eq!(report.stdout(), &["skipped docs: exact pin".to_string()]);
    assert!(report.stderr().is_empty());
}

#[test]
fn add_repairs_half_initialized_remote_cache() {
    let root = TestDir::new("repair-remote-cache");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "README.md", "hello\n");

    let remote_key = git_hash_string(remote.to_str().unwrap());
    let broken_cache = cache_root.join("remotes").join(format!("{remote_key}.git"));
    fs::create_dir_all(broken_cache.parent().unwrap()).unwrap();
    git(None, &["init", "--bare", broken_cache.to_str().unwrap()]);

    let report = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root,
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();
    assert_eq!(report.exit_code(), ExitCode::SUCCESS);
    assert!(report.stdout()[0].starts_with("added docs -> "));
    assert_eq!(
        git_output(
            None,
            &[
                "--git-dir",
                broken_cache.to_str().unwrap(),
                "config",
                "--get",
                "remote.origin.url"
            ]
        ),
        remote.to_str().unwrap()
    );
}

#[test]
fn sync_warns_on_invalid_ref_before_running_git() {
    let root = TestDir::new("invalid-lock-ref");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    fs::create_dir_all(workspace.join("grepo")).unwrap();
    fs::write(
        workspace.join("grepo/.lock"),
        r#"[repos.docs]
url = "git@example.com:org/docs.git"
mode = "ref"
ref = "--upload-pack=/bin/echo"
"#,
    )
    .unwrap();

    let report = run_for_test(workspace, cache_root, state_root, "git".into(), &["sync"]).unwrap();
    assert_eq!(report.exit_code(), ExitCode::from(1));
    assert_eq!(
        report.stderr(),
        &["warning: failed to sync docs: invalid ref: --upload-pack=/bin/echo".to_string()]
    );
}

#[test]
fn add_uses_owner_only_store_permissions() {
    let root = TestDir::new("store-perms");
    let workspace = root.path.join("workspace");
    let cache_root = root.path.join("cache");
    let state_root = root.path.join("state");
    let remote = root.path.join("remote.git");
    let seed = root.path.join("seed");
    fs::create_dir_all(&workspace).unwrap();

    seed_remote_repo(&remote, &seed, "secret.txt", "secret\n");

    run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();

    let snapshot_dir = fs::canonicalize(workspace.join("grepo/docs")).unwrap();
    let snapshot_file = snapshot_dir.join("secret.txt");
    assert_eq!(mode_bits(&cache_root), 0o700);
    assert_eq!(mode_bits(&state_root), 0o700);
    assert_eq!(mode_bits(&snapshot_dir), 0o500);
    assert_eq!(mode_bits(&snapshot_file), 0o400);
}

fn seed_remote_repo(remote: &Path, seed: &Path, file_name: &str, contents: &str) {
    git(None, &["init", "--bare", remote.to_str().unwrap()]);
    git(
        None,
        &["clone", remote.to_str().unwrap(), seed.to_str().unwrap()],
    );
    git(Some(seed), &["checkout", "-b", "main"]);
    fs::write(seed.join(file_name), contents).unwrap();
    git(Some(seed), &["add", file_name]);
    git(
        Some(seed),
        &[
            "-c",
            "user.name=grepo",
            "-c",
            "user.email=grepo@example.com",
            "commit",
            "-m",
            "seed",
        ],
    );
    git(Some(seed), &["push", "-u", "origin", "main"]);
}

fn git(cwd: Option<&Path>, args: &[&str]) {
    let mut command = Command::new("git");
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let status = command.args(args).status().unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn git_output(cwd: Option<&Path>, args: &[&str]) -> String {
    let mut command = Command::new("git");
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command.args(args).output().unwrap();
    assert!(output.status.success(), "git {:?} failed", args);
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn git_hash_string(value: &str) -> String {
    let mut command = Command::new("git");
    command.args(["hash-object", "--stdin"]);
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "git hash-object failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn extract_lock_commit(lockfile: &str, alias: &str) -> Option<String> {
    let section = format!("[repos.{alias}]");
    let mut lines = lockfile.lines();
    while let Some(line) = lines.next() {
        if line.trim() != section {
            continue;
        }
        for entry in lines.by_ref() {
            let trimmed = entry.trim();
            if trimmed.starts_with('[') {
                break;
            }
            if let Some(commit) = trimmed.strip_prefix("commit = ") {
                return commit
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .map(str::to_string);
            }
        }
        break;
    }
    None
}

fn make_tree_writable(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path).unwrap();
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            continue;
        }

        let mut permissions = metadata.permissions();
        permissions.set_mode(permissions.mode() | 0o700);
        fs::set_permissions(&path, permissions).unwrap();

        if file_type.is_dir() {
            for entry in fs::read_dir(&path).unwrap() {
                stack.push(entry.unwrap().path());
            }
        }
    }
}

fn mode_bits(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}
