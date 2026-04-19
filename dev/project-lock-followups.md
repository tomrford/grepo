# Project Lock Follow-Ups

- npm lockfiles: pick the extraction layer for `package-lock.json`, then map direct dependencies to existing `npm:*` grepo entries.
- pnpm lockfiles: implement a grepo-owned parser around `importers` and `packages`; avoid pinning the feature to one exact `lockfileVersion`.
- Bun lockfiles: evaluate `chaste-bun` against grepo's thinner "name -> exact version" contract before adding the dependency.
- Yarn lockfiles: evaluate `chaste-yarn` for Classic and Berry support, then decide whether grepo wants one shared path or format-specific handling.
- Project-lock CLI/docs: extend `--project-lock` support docs and tests as each JavaScript lockfile lands.
