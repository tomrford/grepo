---
name: grepo
description: "Guide for working with grepo, a Rust CLI that manages project-local read-only reference repositories. Use this skill whenever a project contains a `grepo/` directory with a `.lock` file, when entries under `grepo/<alias>` appear as symlinks into a shared cache, when the user mentions grepo / `grepo add` / `grepo sync` / `grepo update`, or when you notice that code you are reading lives inside a read-only snapshot tree under `grepo/`. Explains the commands, the lockfile, sources (git URL / npm / cargo), and what the symlinked trees actually are."
---

# grepo

grepo pins recurring read-only reference sources into a project-local `grepo/` directory. `grepo/.lock` is the tracked source of truth; each `grepo/<alias>` is a generated symlink into a shared cached snapshot (a plain read-only tree with `.git` stripped).

Sources can come from three places:

- a raw git URL (the user's `git` CLI handles transport and auth),
- an npm package (resolved through the npm registry to its upstream git commit, optionally with a `subdir`),
- a cargo crate (downloaded as a sha256-verified tarball from crates.io).

## What the code under `grepo/<alias>` actually is

If you are reading this skill because you are looking at files inside `grepo/<alias>`, note:

- The path is a symlink into a shared grepo cache outside the project.
- The tree is **read-only** and has **no `.git` directory**. `git log`, `git blame`, `git status` inside it will not work. For git-backed entries, use the upstream repo (URL recorded in `grepo/.lock`) for history. For cargo tarball entries, there is no git history — only the published crate contents.
- The tree is a snapshot of one specific commit (git backend) or one specific published archive (tarball backend), recorded in `grepo/.lock`.
- It may be a subtree of the upstream source if the entry has a `subdir`.
- Do not edit files here. Changes will not persist across `grepo sync` / `grepo update` and may be wiped when the symlink is retargeted.
- If the user asks you to change something that lives under `grepo/<alias>`, the change belongs upstream in that project, not here.

## Lockfile

`grepo/.lock` is TOML, tool-owned, rewritten canonically. One section per alias.

Git backend (default — no `backend` key):

```toml
[repos.trpc-server]
source = "npm:@trpc/server@11.6.0"   # optional, present when added via --npm
url = "https://github.com/trpc/trpc.git"
subdir = "packages/server"           # optional, snapshot is just this subtree
mode = "exact"
commit = "91e45f614fa266a06bc99f677d576793ba949c2b"
```

Modes (git backend only):

- `mode = "default"` — `update` advances to the remote default branch's current HEAD.
- `mode = "ref"` + `ref = "..."` — `update` advances to the named branch or tag.
- `mode = "exact"` + `commit = "..."` — pinned; `update` leaves it alone.

`commit` is always present once the entry has been materialized.

Tarball backend (cargo crates today):

```toml
[repos.serde]
backend = "tarball"
source = "cargo:serde@1.0.197"
url = "https://crates.io/api/v1/crates/serde/1.0.197/download"
sha256 = "3fb1c873e1b9b056a4dc4c0c198b24c3ffa059243875552b2bd0933b1aee4ce2"
```

Tarball entries have no `mode` / `commit`; they are pinned by `sha256`.

Update semantics for package sources: a `source` string without a version (e.g. `npm:zod`, `cargo:serde`) is movable — `update` re-queries the registry and advances to the newest release. A versioned `source` (e.g. `npm:zod@3.22.4`) stays pinned regardless of `mode`.

## Commands

- **`grepo init`** — create `grepo/` and an empty `grepo/.lock` in the current directory.
- **`grepo add <alias> <source-flag> [options]`** — register an alias and materialize immediately. Exactly one source flag is required:
  - `--url <git-url>` — raw git URL. Pair with `--ref <branch-or-tag>` (tracking ref) or `--commit <sha>` (exact pin). `--ref` and `--commit` are mutually exclusive and only valid with `--url`.
  - `--npm <spec>` — npm package: `zod`, `chalk@5.3.0`, `@trpc/server@11.6.0`. Use a concrete registry version when you include one; ranges / dist-tags are rejected. Packages that don't publish `gitHead` (e.g. React, Babel) cannot be resolved this way — fall back to `--url` against the upstream repo.
  - `--cargo <spec>` — cargo crate: `serde`, `serde@1.0.197`, `clap@4.6.1`. Use a concrete registry version when you include one.
  - `--subdir <path>` — snapshot only this subdirectory of the resolved source. Not valid with `--cargo`.
  - `--force` — replace an existing alias.
- **`grepo list`** — print configured aliases, their source (if any), URL, subdir, and how they track upstream.
- **`grepo remove <alias>...`** — drop aliases from the lockfile and delete their symlinks.
- **`grepo sync`** — materialize the commits / tarballs already recorded in the lockfile. Idempotent.
- **`grepo update [alias...] [--project-lock <PATH>]`** — advance movable entries and rewrite the lockfile. For git entries, that means `default` / `ref` modes fetch and advance. For package-sourced entries, movable means the `source` has no version (`npm:react`, `cargo:serde`); versioned package specs and `exact` git pins are left alone. Omit aliases to update every movable entry. With `--project-lock`, grepo synchronizes existing package-sourced entries to versions pinned in the project lockfile before materializing them. Current support is `Cargo.lock`.
- **`grepo gc`** — prune cache snapshots and state entries that no project's lockfile still references. `--verbose` lists each deleted path.
- **`grepo skill`** — print this skill document to stdout.

Mutating commands serialize on `grepo/.mutate.lock`; a second concurrent invocation will wait.

## Workflow

```sh
grepo init
grepo add mint --url git@github.com:tomrford/mint.git
grepo add polarion --url git@github.com:tomrford/polarionmcp.git --ref main
grepo add zod --npm zod
grepo add trpc-server --npm @trpc/server@11.6.0
grepo add serde --cargo serde@1.0.197
grepo list
grepo update            # later, to advance movable entries
grepo update --project-lock Cargo.lock
grepo gc                # occasionally, to reclaim disk
```

## Gotchas

- `grepo/<alias>` is read-only — writes fail with `EACCES`. This is intentional.
- No `.git` inside the snapshot. For git-backed entries, use the upstream URL from `.lock` for history or blame. Cargo tarball entries have no upstream git history available through grepo.
- `grepo add` requires a source flag (`--url`, `--npm`, or `--cargo`); there is no positional URL argument.
- `--ref` / `--commit` are only valid with `--url`. `--subdir` is rejected with `--cargo`.
- Package specs must be concrete registry versions. `zod@^3`, `zod@latest`, and similar range/tag forms are rejected.
- `grepo add` without `--force` refuses to replace an existing alias.
- `grepo sync` does not advance commits. Use `grepo update` to move movable entries.
- `grepo update --project-lock` currently supports `Cargo.lock` paths only.
- npm packages that don't publish `gitHead` metadata cannot be resolved to an exact commit; grepo will refuse and suggest using `--url` against the upstream repo directly.
- The `grepo/` directory and its contents are tool-owned. Only `grepo/.lock` should be committed; the symlinks are generated.
