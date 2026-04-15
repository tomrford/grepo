# grepo v1 spec

`grepo` is a local-first context manager for recurring read-only reference repos.

The point is modest: instead of repeatedly cloning reference repos into ad hoc temp directories, a project owns a single `grepo/` directory, tracks a compact lockfile there, and gets stable local symlinks for the repos it wants around often.

## v1 shape

- Rust CLI
- tool-owned `grepo/` directory
- tracked `grepo/.lock`
- tracked `grepo/.gitignore`
- generated symlinks in `grepo/<alias>`
- shared local cache for remotes and snapshots
- OS-native state directory for GC roots
- nearest-ancestor discovery for `grepo/.lock`
- explicit `sync` versus `update`
- GC rooted on lockfile symlinks, not age heuristics
- no direnv integration
- no daemon
- no mounts

## Goals

- Stable project-local paths for recurring reference repos.
- Minimal CLI-only management; no hand-edited manifest required.
- Read-only consumption by default.
- Shared local store with dedupe by URL plus commit.
- Exact, principled GC based on rooted lockfiles.
- OS-native cache and state locations.

## Non-goals

- Hosted or multi-user backend.
- FUSE, FSKit, or other mount layers.
- A search index.
- Write access into referenced repos.
- Full dependency capture like Nix.

## Project layout

Each project root owns a `grepo/` directory:

```text
my-project/
  grepo/
    .gitignore
    .lock
    mint -> /Users/tomford/Library/Caches/grepo/snapshots/<url-hash>/<snapshot-hash>
    polarion -> /Users/tomford/Library/Caches/grepo/snapshots/<url-hash>/<snapshot-hash>
```

`grepo/.gitignore` is always:

```gitignore
*
!.gitignore
!.lock
```

That keeps the lockfile tracked while leaving generated symlinks untracked.

## Root discovery

`grepo` searches upward from the current working directory for the nearest `grepo/.lock`.

- `grepo init` always creates a new root in the current directory, even if a parent root exists.
- `grepo add` creates a root in the current directory if no ancestor root exists.
- other project-scoped commands operate on the nearest ancestor root.

This allows nested `grepo/` roots when wanted, while keeping the common case zero-config.

## Lockfile

The tracked source of truth is `grepo/.lock`.

Format is a small TOML subset:

```toml
[repos.mint]
url = "git@github.com:tomrford/mint.git"
track = "default"
commit = "4e019e37011e778fea85b9dd04d396e9db105ac3"

[repos.polarion]
url = "git@github.com:tomrford/polarionmcp.git"
ref = "main"
commit = "abc123..."

[repos.agent_can]
url = "git@github.com:tomrford/agent-can.git"
commit = "8c27c9b..."
```

Per-entry fields:

- `url` is required.
- `track = "default"` means follow the remote default branch on `update`.
- `ref = "..."` means follow that named ref on `update`.
- `commit = "..."` is the currently locked commit.
- `commit` without `track` or `ref` means an exact pin.

The tool owns the file format and rewrites it canonically.

## Command semantics

### `grepo init`

- creates `grepo/`, `grepo/.lock`, and `grepo/.gitignore` in the current directory
- registers the new lockfile as a GC root

### `grepo add <alias> <url> [--ref <ref> | --commit <commit>]`

- finds the nearest ancestor root, or creates a new root in `cwd` if none exists
- adds or replaces the alias in `grepo/.lock`
- eagerly resolves the commit and materializes the snapshot
- updates `grepo/<alias>` immediately

Default behavior with no flags is:

- `track = "default"`
- resolve current remote `HEAD`
- write that commit into the lockfile

### `grepo remove <alias>...`

- removes one or more aliases by name
- deletes the matching generated symlinks
- rewrites `grepo/.lock`

### `grepo sync`

- realizes the commits already locked in `grepo/.lock`
- fills missing commits only when an entry has `track = "default"` or `ref = "..."`
- creates or updates `grepo/<alias>` symlinks
- removes leftover non-hidden symlinks under `grepo/` that are not present in the lockfile

`sync` does not advance moving refs when a commit is already locked.

### `grepo update [alias...]`

- advances tracked entries to the latest commit
- if aliases are provided, only those aliases are updated
- if no aliases are provided, all updateable aliases are updated
- exact commit pins are left unchanged
- updated commits are written back to `grepo/.lock`
- symlinks are updated to the new snapshots

### `grepo gc`

- reads all rooted lockfiles from the state directory
- computes the reachable set of `url + commit` snapshots
- deletes every unreachable snapshot
- deletes every remote cache with no reachable snapshots
- removes stale GC-root symlinks whose lockfile target no longer exists

There are no GC tuning flags in v1. GC is a full prune to the minimum reachable set.

## Shared storage layout

Default cache root:

- macOS: `~/Library/Caches/grepo`
- Linux: `${XDG_CACHE_HOME:-~/.cache}/grepo`

Default state root:

- macOS: `~/Library/Application Support/grepo`
- Linux: `${XDG_STATE_HOME:-~/.local/state}/grepo` when available, otherwise the local data directory

Layout:

```text
<state-root>/
  roots/
    <project-hash>.lock -> /abs/project/grepo/.lock

<cache-root>/
  remotes/
    <url-hash>.git/
  snapshots/
    <url-hash>/
      <snapshot-hash>/
```

Notes:

- `roots/` are durable GC roots and therefore live in state, not cache.
- `remotes/` are shared bare Git caches keyed by raw URL string.
- `snapshots/` are read-only checked-out trees keyed by raw URL string plus commit.
- no URL canonicalization in v1; SSH and HTTPS forms of the same repo are treated as different URLs.

## Read-only posture

The intent is “reference repos are not where you edit.”

v1 enforces that pragmatically:

- snapshots are plain checked-out files, not Git worktrees
- `.git` is stripped from snapshots
- write bits are removed from files and directories in snapshots
- updates happen by repointing symlinks at different snapshots

This is not Nix-store immutability. It is just enough friction to discourage accidental edits while staying simple and portable across Unix-like systems.

## Git handling

v1 shells out to the normal Git CLI instead of reimplementing transport logic.

That keeps:

- SSH auth in the user’s existing Git setup
- HTTPS auth in the user’s existing Git setup
- ref and default-branch resolution in Git
- implementation complexity bounded

Important defaults:

- no recursive submodules
- no Git LFS handling
- no `.git` directory in snapshots
- `grepo/` is a tool-owned namespace for non-hidden symlinks; `sync`, `add`, and `update` may replace or prune them

## Future work, if the tool proves useful

- richer lockfile metadata if the current four fields stop being enough
- shell hooks or direnv integration if a future global mount/root model needs it
- optional search helpers
- stronger locking around concurrent syncs
