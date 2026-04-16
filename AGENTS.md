## Project Overview

grepo is a Rust CLI for managing project-local read-only reference repos under `grepo/`.

Current shape:

- `grepo/.lock` is the tracked source of truth.
- `grepo/<alias>` are generated symlinks into shared cached snapshots.
- snapshots are plain read-only trees with `.git` stripped.
- Git operations shell out to the user’s `git` CLI.
- mutating commands take an exclusive file lock on `grepo/.mutate.lock`.

## Development Environment

- Use `nix develop` for the toolchain.
- Build: `cargo build`
- Test: `cargo test`
- Full git-backed integration suite: `cargo test --features git-integration-tests`
- Format: `cargo fmt`
- Lint: `cargo clippy`
