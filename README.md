# grepo

`grepo` is a small Rust CLI for keeping recurring reference repositories under a project-local `grepo/` directory.

It keeps `grepo/.lock` as the tracked source of truth, materializes read-only snapshots in a shared local cache, and exposes stable symlinks like `grepo/mint` inside the project.

Current behavior:

- `grepo/` is tool-owned.
- `grepo/.lock` is rewritten canonically by the tool.
- `grepo/<alias>` entries are generated symlinks and may be replaced or pruned by `sync`.
- `add` resolves and materializes immediately, and refuses to replace an existing alias unless `--force` is passed.
- `list` prints a concise view of configured aliases.
- `sync` realizes the commits already recorded in `grepo/.lock`.
- `update` advances tracked entries and rewrites `grepo/.lock`.
- `gc` prunes unreachable snapshots, remote caches, and stale rooted lockfiles; `--verbose` includes per-path detail.

The lockfile supports three states per alias:

- `mode = "default"` follows the remote default branch on `update`.
- `mode = "ref"` plus `ref = "..."` follows that named ref on `update`.
- `mode = "exact"` plus `commit = "..."` is an exact pin.

Storage follows OS conventions:

- cache: `~/Library/Caches/grepo` on macOS, `${XDG_CACHE_HOME:-~/.cache}/grepo` on Linux
- state: `~/Library/Application Support/grepo` on macOS, `${XDG_STATE_HOME:-~/.local/state}/grepo` on Linux when available, otherwise the local data directory

Quick start:

```sh
grepo init
grepo add mint git@github.com:tomrford/mint.git
grepo add polarion git@github.com:tomrford/polarionmcp.git --ref main
grepo list
grepo update
```

Example `grepo/.lock`:

```toml
[repos.mint]
url = "git@github.com:tomrford/mint.git"
mode = "default"
commit = "4e019e37011e778fea85b9dd04d396e9db105ac3"

[repos.polarion]
url = "git@github.com:tomrford/polarionmcp.git"
mode = "ref"
ref = "main"
commit = "abc123..."
```
