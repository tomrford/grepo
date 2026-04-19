---
name: grepo-subagent
description: Delegate repo-grounded research to a subagent using grepo-managed snapshots. Use when the user says to run a grepo subagent, have a subagent inspect a project in `grepo/`, or answer a research question against code already pinned under a `grepo/` alias.
---

# Grepo Subagent

Keep orchestration in the root agent. Ensure `grepo` is available, the current project has `grepo/.lock`, and the needed aliases already exist under `grepo/<alias>`. If you need the grepo contract, load the grepo skill or run `grepo skill`.

Spawn one focused subagent for the research step. Pass the concrete question plus the absolute `grepo/<alias>` path or paths it should inspect. Tell the subagent to stay read-only, search the provided trees directly, and return grounded findings with exact file paths and concise evidence.

Use the root agent to decide which aliases matter, run `grepo add` / `grepo sync` / `grepo update` if setup is missing, and integrate the subagent's findings into the final answer.
