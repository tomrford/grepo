---
name: grepo
description: "Guide for working with grepo, a Rust CLI that manages project-local read-only reference repositories. Use this skill whenever a project contains a `grepo/` directory with a `.lock` file, when entries under `grepo/<alias>` appear as symlinks into a shared cache, when the user mentions grepo / `grepo add` / `grepo sync` / `grepo update`, or when you notice that code you are reading lives inside a read-only snapshot tree under `grepo/`. Explains the commands, the lockfile, and what the symlinked trees actually are."
---

# grepo

grepo pins recurring read-only reference repositories into a project-local `grepo/` directory. `grepo/.lock` is the tracked source of truth; each `grepo/<alias>` is a generated symlink into a shared cached snapshot (a plain read-only tree with `.git` stripped). Git operations shell out to the user's `git` CLI.

## What the code under `grepo/<alias>` actually is

If you are reading this skill because you are looking at files inside `grepo/<alias>`, note:

- The path is a symlink into a shared grepo cache outside the project.
- The tree is **read-only** and has **no `.git` directory**. `git log`, `git blame`, `git status` inside it will not work. Use the upstream repo (URL recorded in `grepo/.lock`) for history.
- The tree is a snapshot of one specific commit, recorded in `grepo/.lock` under `[repos.<alias>].commit`.
- Do not edit files here. Changes will not persist across `grepo sync` / `grepo update` and may be wiped when the symlink is retargeted.
- If the user asks you to change something that lives under `grepo/<alias>`, the change belongs upstream in that project, not here.

## Lockfile

`grepo/.lock` is TOML, tool-owned, rewritten canonically. One section per alias:

```toml
[repos.pinned]
url = "git@github.com:example/thing.git"
mode = "exact"
commit = "deadbeef..."
```

Modes:

- `mode = "default"` — `update` advances to the remote default branch's current HEAD.
- `mode = "ref"` + `ref = "..."` — `update` advances to the named branch or tag.
- `mode = "exact"` + `commit = "..."` — pinned; `update` leaves it alone.

`commit` is always present once the entry has been materialized.

## Commands

- **`grepo init`** — create `grepo/` and an empty `grepo/.lock` in the current directory.
- **`grepo add <alias> <url>`** — register an alias and materialize immediately. Flags: `--ref <branch-or-tag>` (tracking ref), `--commit <sha>` (exact pin; mutually exclusive with `--ref`), `--force` (replace an existing alias).
- **`grepo list`** — print configured aliases and how they track upstream.
- **`grepo remove <alias>...`** — drop aliases from the lockfile and delete their symlinks.
- **`grepo sync`** — materialize the commits already recorded in the lockfile. Idempotent.
- **`grepo update [alias...]`** — fetch upstream, advance tracking entries (`default` / `ref` modes) to the latest commit, rewrite the lockfile, and retarget symlinks. Exact pins are skipped. Omit aliases to update every tracking entry.
- **`grepo gc`** — prune cache snapshots and state entries that no project's lockfile still references. `--verbose` lists each deleted path.

Mutating commands serialize on `grepo/.mutate.lock`; a second concurrent invocation will wait.

## Workflow

```sh
grepo init
grepo add mint git@github.com:tomrford/mint.git
grepo add polarion git@github.com:tomrford/polarionmcp.git --ref main
grepo list
grepo update            # later, to advance tracking entries
grepo gc                # occasionally, to reclaim disk
```

## Gotchas

- `grepo/<alias>` is read-only — writes fail with `EACCES`. This is intentional.
- No `.git` inside the snapshot. Use the upstream URL from `.lock` for history or blame.
- `grepo add` without `--force` refuses to replace an existing alias.
- `grepo sync` does not advance commits. Use `grepo update` to move tracking entries.
- The `grepo/` directory and its contents are tool-owned. Only `grepo/.lock` should be committed; the symlinks are generated.
