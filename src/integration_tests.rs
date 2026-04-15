use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    let code = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["add", "docs", remote.to_str().unwrap()],
    )
    .unwrap();
    assert_eq!(code, std::process::ExitCode::SUCCESS);

    let lockfile = fs::read_to_string(workspace.join("grepo/.lock")).unwrap();
    assert!(lockfile.contains("[repos.docs]"));
    assert!(lockfile.contains("track = \"default\""));
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
    let code = run_for_test(
        workspace.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["update", "a"],
    )
    .unwrap();
    assert_eq!(code, std::process::ExitCode::SUCCESS);

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

    let code = run_for_test(
        root.path.clone(),
        cache_root.clone(),
        state_root.clone(),
        "git".into(),
        &["gc"],
    )
    .unwrap();
    assert_eq!(code, std::process::ExitCode::SUCCESS);
    assert!(rooted_snapshot.exists());
    assert!(remote_cache.exists());
    assert!(!stale_snapshot.exists());
    assert!(!stale_remote.exists());
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
