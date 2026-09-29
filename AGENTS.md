# DOX framework

- DOX is highly performant AGENTS.md hierarchy installed here
- Agent must follow DOX instructions across any edits

## Core Contract

- AGENTS.md files are binding work contracts for their subtrees
- Work products, source materials, instructions, records, assets, and durable
  docs must stay understandable from the nearest applicable AGENTS.md plus every
  parent AGENTS.md above it

## Read Before Editing

1. Read the root AGENTS.md
2. Identify every file or folder you expect to touch
3. Walk from the repository root to each target path
4. Read every AGENTS.md found along each route
5. If a parent AGENTS.md lists a child AGENTS.md whose scope contains the path,
   read that child and continue from there
6. Use the nearest AGENTS.md as the local contract and parent docs for repo-wide
   rules
7. If docs conflict, the closer doc controls local work details, but no child
   doc may weaken DOX

Do not rely on memory. Re-read the applicable DOX chain in the current session
before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

Update the closest owning AGENTS.md when a change affects:

- purpose, scope, ownership, or responsibilities
- durable structure, contracts, workflows, or operating rules
- required inputs, outputs, permissions, constraints, side effects, or artifacts
- user preferences about behavior, communication, process, organization, or
  quality
- AGENTS.md creation, deletion, move, rename, or index contents

Update parent docs when parent-level structure, ownership, workflow, or child
index changes. Update child docs when parent changes alter local rules. Remove
stale or contradictory text immediately. Small edits that do not change behavior
or contracts may leave docs unchanged, but the DOX pass still must happen.

## Hierarchy

- Root AGENTS.md is the DOX rail: project-wide instructions, global preferences,
  durable workflow rules, and the top-level Child DOX Index
- Child AGENTS.md files own domain-specific instructions and their own Child DOX
  Index
- Each parent explains what its direct children cover and what stays owned by
  the parent
- The closer a doc is to the work, the more specific and practical it must be

## Child Doc Shape

- Create a child AGENTS.md when a folder becomes a durable boundary with its own
  purpose, rules, responsibilities, workflow, materials, or quality standards
- Work Guidance must reflect the current standards of the project or user
  instructions; if there are no specific standards or instructions yet, leave it
  empty
- Verification must reflect an existing check; if no verification framework
  exists yet, leave it empty and update it when one exists

Default section order:
- Purpose
- Ownership
- Local Contracts
- Work Guidance
- Verification
- Child DOX Index

## Style

- Keep docs concise, current, and operational
- Document stable contracts, not diary entries
- Put broad rules in parent docs and concrete details in child docs
- Prefer direct bullets with explicit names
- Do not duplicate rules across many files unless each scope needs a local
  version
- Delete stale notes instead of explaining history
- Trim obvious statements, repeated rules, misplaced detail, and warnings for
  risks that no longer exist

## Closeout

1. Re-check changed paths against the DOX chain
2. Update nearest owning docs and any affected parents or children
3. Refresh every affected Child DOX Index
4. Remove stale or contradictory text
5. Run existing verification when relevant
6. Report any docs intentionally left unchanged and why

## Verification

- `cargo test` from the repo root covers all three crates (core, engine, ui)
- `cargo` is not on the default PATH: run it as `nix shell nixpkgs#cargo nixpkgs#rustc -c cargo test`
- Tests run in parallel by default. Tests must never mutate process-global state
  (`std::env::set_var`, current dir) to force a condition: pass the value in
  instead. Mutating `PATH` races sibling tests that spawn real binaries.
- Branches use worktrunk (`wt switch --create <branch>`); never commit on `main`

## User Preferences

- Merge finished work into `main` with `wt merge` once its tests pass
- Nothing is pushed without being asked

## Child DOX Index

- `crates/core/` — queue state, settings, accounts, extract logic: package/member
  model, worst-case rollup, byte-weighted progress, atomic persistence
  (.tmp+rename), stale→gone, retry handle-swap, per-host account records with
  keyring-only secrets, plugin trait, nine global settings, archive extraction
  (zip/tar in-Rust, 7z/rar via external binary); verification: `cargo test -p moon-down-core`
- `crates/engine/` — managed-local aria2c child: spawn with ephemeral secret and
  0600 conf, first free port in 6800-6899, state-dir lock, one batched poll tick,
  enqueue with credential stripping, polite/forced/kill stop, single respawn,
  extraction orchestration on blocking threads
- `crates/ui/` — ratatui queue inspector and the `moon-down` binary: one event loop
  owning state, render on state change only, queue always visible, keys
  1-6/j/k/space/d/D/r/x/e/a, settings screen, plus a status bar and a
  completed-history pane. Row heights and the accent colour derive from the
  design seed documented in `crates/ui/src/main.rs` — change the seed, not the
  numbers. The binary drives simulated progress (the status bar says DEMO) until
  the JSON-RPC transport lands
- `.agents/skills/` — agent skills owned by this repo; `herdr/` documents driving the
  Herdr workspace CLI (layout IDs, tab/pane recipes, agent lifecycle). `herdr --skill`
  stays the upstream authority
- Root workspace `Cargo.toml` owns member list and shared package version

No child AGENTS.md yet — crates are single-file modules with no durable local
contracts of their own; add child docs when a crate gains its own workflow.
