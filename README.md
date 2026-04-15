# grepo

`grepo` is a small Rust CLI for recurring read-only reference repos.

It owns a `grepo/` directory in your project, keeps a tracked `grepo/.lock`, materializes read-only snapshots in a shared local cache, keeps GC roots in OS-native state storage, and exposes project-local symlinks like `grepo/mint`.

Current shape:

- `grepo/` is the tool-owned root in a project.
- `grepo/.lock` is the tracked source of truth.
- `grepo/.gitignore` keeps only `.lock` and `.gitignore` tracked.
- `grepo/<alias>` are generated symlinks.
- `grepo add` eagerly resolves and syncs the new entry.
- `grepo sync` realizes locked commits exactly.
- `grepo update` advances default-branch or named-ref entries and can target specific aliases.
- `grepo gc` fully prunes unreachable snapshots and remotes based on rooted lockfiles.

Storage follows OS norms:

- macOS cache: `~/Library/Caches/grepo`
- macOS state: `~/Library/Application Support/grepo`
- Linux cache: `${XDG_CACHE_HOME:-~/.cache}/grepo`
- Linux state: `${XDG_STATE_HOME:-~/.local/state}/grepo` when available, otherwise the local data directory

Quick start:

```sh
grepo init
grepo add mint git@github.com:tomrford/mint.git
grepo add polarion git@github.com:tomrford/polarionmcp.git --ref main
grepo update
```

Example `grepo/.lock`:

```toml
[repos.mint]
url = "git@github.com:tomrford/mint.git"
track = "default"
commit = "4e019e37011e778fea85b9dd04d396e9db105ac3"

[repos.polarion]
url = "git@github.com:tomrford/polarionmcp.git"
ref = "main"
commit = "abc123..."
```

Command surface:

```text
grepo init
grepo add <alias> <url> [--ref <ref> | --commit <commit>]
grepo remove <alias>...
grepo sync
grepo update [alias...]
grepo gc
```

More detail lives in [spec.md](/Users/tomford/code/projects/grepo/spec.md).
