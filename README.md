# grepo

`grepo` is a small Rust CLI for keeping recurring reference repositories under a project-local `grepo/` directory.

It keeps `grepo/.lock` as the tracked source of truth, materializes read-only snapshots in a shared local cache, and exposes stable symlinks like `grepo/mint` inside the project.

Sources can be raw git URLs, npm packages, or cargo crates. For git and npm, `grepo` shells out to your installed `git` for transport, auth, and private-repo access. For cargo (and any future tarball backend), `grepo` fetches over HTTPS and verifies a sha256. Treat repo URLs, registry responses, and checked-in `grepo/.lock` files as trusted inputs; `grepo` intentionally relies on your local git configuration instead of reimplementing credentials or transport policy.

Current behavior:

- `grepo/` is tool-owned.
- `grepo/.lock` is rewritten canonically by the tool.
- `grepo/<alias>` entries are generated symlinks and may be replaced or pruned by `sync`.
- `add` resolves and materializes immediately, and refuses to replace an existing alias unless `--force` is passed.
- `list` prints a concise view of configured aliases.
- `sync` realizes the commits (or tarballs) already recorded in `grepo/.lock`.
- `update` advances tracked entries and rewrites `grepo/.lock`.
- `update --project-lock <PATH>` synchronizes existing package-sourced entries to versions pinned in a project lockfile. Current support is `Cargo.lock`.
- `gc` prunes unreachable snapshots, remote caches, and stale rooted lockfiles; `--verbose` includes per-path detail.
- `skill` prints the bundled grepo skill markdown for agents that need the exact operating rules.

Sources (`grepo add`):

- `--url <git-url>` — any URL `git clone` accepts. Pair with `--ref <branch-or-tag>` to follow a ref, or `--commit <sha>` to pin exactly.
- `--npm <spec>` — an npm package as you'd pass to `npm install`: `zod`, `chalk@5.3.0`, `@trpc/server@11.6.0`. grepo reads the package's `gitHead` and `repository` metadata from the registry and snapshots that git commit. A scoped monorepo package's `repository.directory` becomes the default `subdir` (e.g. `@trpc/server` → `packages/server`). Packages that don't publish `gitHead` (React, Babel, many others) will be rejected with a message pointing you at `--url` instead.
- `--cargo <spec>` — a crate as you'd pass to `cargo add`: `serde`, `serde@1.0.197`. grepo downloads the published tarball from crates.io and verifies its sha256.
- `--subdir <path>` — snapshot only a subdirectory of the resolved source tree. Not valid with `--cargo`.

Package specs must be concrete registry versions. Ranges (`^1`, `~1.0`), wildcards, and dist-tags (`latest`) are rejected so that resolution is reproducible.

A package spec without a version (`--npm zod`) remains movable on `update` — grepo re-queries the registry and advances to the newest published release. A versioned spec (`--npm zod@3.22.4`) is pinned.

Lockfile states per alias:

- git backend (default):
  - `mode = "default"` follows the remote default branch on `update`.
  - `mode = "ref"` plus `ref = "..."` follows that named ref on `update`.
  - `mode = "exact"` plus `commit = "..."` is an exact pin using a full hex object id.
- tarball backend (`backend = "tarball"`):
  - Always pinned to a concrete `source` + `url` + `sha256`. `update` only moves if the `source` is versionless (e.g. `cargo:serde`).

Storage follows OS conventions:

- cache: `~/Library/Caches/grepo` on macOS, `${XDG_CACHE_HOME:-~/.cache}/grepo` on Linux
- state: `~/Library/Application Support/grepo` on macOS, `${XDG_STATE_HOME:-~/.local/state}/grepo` on Linux when available, otherwise the local data directory
- store directories and cached snapshots are owner-only by default

Quick start:

```sh
grepo init
grepo add mint --url git@github.com:tomrford/mint.git
grepo add polarion --url git@github.com:tomrford/polarionmcp.git --ref main
grepo add zod --npm zod
grepo add trpc-server --npm @trpc/server@11.6.0
grepo add serde --cargo serde@1.0.197
grepo list
grepo update
grepo update --project-lock Cargo.lock
```

Project-lock synchronization currently supports `Cargo.lock`. Follow-up work for npm, pnpm, Bun, and Yarn lockfiles is tracked in [`dev/project-lock-followups.md`](dev/project-lock-followups.md).

Development:

`cargo test` runs the fast default suite. The git-backed integration tests are opt-in and expect `git` on `PATH`: `cargo test --features git-integration-tests`.

Agent integration:

`grepo skill` prints the shipped [`skills/grepo/SKILL.md`](skills/grepo/SKILL.md) text to stdout so an agent can load the exact guidance without guessing the path.

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
commit = "9f4d8d7c6b5a4e3f20112233445566778899aabb"

[repos.trpc-server]
source = "npm:@trpc/server@11.6.0"
url = "https://github.com/trpc/trpc.git"
subdir = "packages/server"
mode = "exact"
commit = "91e45f614fa266a06bc99f677d576793ba949c2b"

[repos.serde]
backend = "tarball"
source = "cargo:serde@1.0.197"
url = "https://crates.io/api/v1/crates/serde/1.0.197/download"
sha256 = "3fb1c873e1b9b056a4dc4c0c198b24c3ffa059243875552b2bd0933b1aee4ce2"
```
