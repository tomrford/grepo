## Project Overview

grepo is a Rust CLI for managing project-local read-only reference repos under `grepo/`.

Current shape:

- `grepo/.lock` is the tracked source of truth.
- `grepo/<alias>` are generated symlinks into shared cached snapshots.
- Snapshots are plain read-only trees with `.git` stripped.
- Two backends: `git` (clone + snapshot a commit) and `tarball` (fetch + extract a sha256-verified archive).
- Git sources shell out to the user's `git` CLI. Tarball sources fetch over HTTPS via `ureq`.
- Package-manager sources (`npm:`, `cargo:`) resolve through the public registries: npm maps a published version to its upstream git commit (+`repository.directory` as `subdir`); cargo downloads the crate tarball from crates.io.
- Optional `subdir` carves out a subtree of the resolved source as the snapshot root (git-backed entries only).
- Mutating commands take an exclusive file lock on `grepo/.mutate.lock`.

## Development Environment

- Use `nix develop` for the toolchain.
- Build: `cargo build`
- Test: `cargo test`
- Full git-backed integration suite: `cargo test --features git-integration-tests`
- Format: `cargo fmt`
- Lint: `cargo clippy`
