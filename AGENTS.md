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

- `nix develop` is the canonical dev shell (see `flake.nix`): cargo, rustc,
  rustfmt, clippy, git, plus gtk3, libayatana-appindicator and p7zip for the
  optional tray and archive-extraction paths. The bare
  `nix shell nixpkgs#cargo nixpkgs#rustc` invocation below still works for
  the default (no-GUI) build.
- `flake.nix` carries a generated pkg-config shim for
  `libayatana-appindicator3-0.1`, because nixpkgs packages the shared library
  with no `.pc` file. `libappindicator` uses hand-written FFI and only needs the
  `.so` at link time, so a metadata-only shim is enough.
- To find a nix package attribute, use `nh search <term>`. Do not guess
  attribute names from memory: `libayatana-appindicator3`, `xorg.libXdo` and
  `unrar` were all wrong guesses, and `unrar` is unfree besides. The real names
  are `libayatana-appindicator`, `xdo`, and (not needed at all, since .rar
  routes through `7z`).
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
- `crates/engine/` — embedded aria2-rust daemon. `Daemon::ensure_running(state_dir,
  exe, download_dir)` writes a 0600 `aria2.conf` and `daemon.json` (port + secret),
  re-executes this binary with `--daemon` after `setsid` in `pre_exec`, and attaches by
  loopback JSON-RPC via `RpcClient` (ureq, no TLS: loopback only). `Tick::from_batch`
  parses the four-call batch and `Tick::apply` folds it into the queue. Enqueue
  requests keep credential stripping and token auth. Never let aria2-rust daemonize
  itself: `--daemon=true` double-forks from inside its tokio runtime and the child
  deadlocks on a vanished thread's lock, so the detach must happen before exec.
  Liveness is the RPC `getVersion` ping, not a pid file — aria2-rust only writes one in
  its own daemon mode, which we do not use. `daemon=true` stays off the conf file.
  Two guards in `Tick::apply` are load-bearing: a gid we do not own suppresses
  reconcile (another client's work must not mark our queue gone), and a terminal row is
  never written to (an errored member must not show live bytes).
- `crates/ui/` — ratatui queue inspector and the `moon-down` binary: one event loop
  owning state, render on state change only, queue always visible, keys
  1-6/j/k/gg/G/Ctrl-D/U/F/B/arrows/h/l/Enter/space/d/D/r/x/e/a, settings screen, plus a
  status bar and a completed-history pane. Row heights and the accent colour derive from
  the design seed documented in `crates/ui/src/main.rs` — change the seed, not the
  numbers. Three modes: plain `moon-down` is the TUI (ensures the daemon is up on first
  run, then only polls it), `moon-down --daemon` is the aria2-rust host that self-detaches
  and outlives the TUI, `moon-down --tray` is the system-tray icon. Progress is real, not
  simulated. Downloads land in `default_download_dir` (XDG_DOWNLOAD_DIR, then
  ~/Downloads/moon-down, then the state dir) — never inside the state dir when a real
  download dir exists. `--enable-rpc=true` must be on argv: aria2-rust refuses an
  RPC-only service with no download input otherwise
- CLI: `moon-down add <url>...` (one download per URL, `--name=`, `--dir=`) and
  `moon-down ls` (live: folds in a daemon tick rather than reading the persisted file).
  `add` stores the returned GID as the member handle immediately — that is the join key
  `Tick::apply` matches on, and skipping it is what makes a download invisible in the TUI.
  There is no demo seeding: a first run is an empty queue, because invented handles like
  `demo-live-1` can never match a real GID and only made the TUI look busy.
  The TUI reloads `state.json` every tick so CLI additions appear within ~2s, and
  persists immediately on any local key edit so the reload cannot undo it. Do not
  reintroduce a periodic flush: it would race the CLI for the same file.
- `crates/ui/src/tray.rs` — optional `tray` cargo feature; off by default so the plain
  build links no GUI libraries. A separate process, not part of the daemon: the daemon is
  a tokio host and GTK wants its own main loop, so a GTK hiccup must not be able to take
  downloads down. The icon is generated into raw RGBA (`Icon::from_rgba`) — no artwork
  file and no dependence on the user's icon theme. Menu events are drained from
  `MenuEvent::receiver()` on the main loop; muda's `set_event_handler` is unusable here
  because it demands `Send + Sync` and `MenuItem` is `Rc`-based. `tray-icon` and `muda`
  both default to a `libxdo` feature that nixpkgs cannot link (no `libxdo.so` is
  packaged); keep `default-features = false` on both, or the build fails at link time.
  That feature only gates muda's predefined Copy/Cut/Paste items, which this tray
  does not use.
- `.agents/skills/` — agent skills owned by this repo; `herdr/` documents driving the
  Herdr workspace CLI (layout IDs, tab/pane recipes, agent lifecycle). `herdr --skill`
  stays the upstream authority
- Root workspace `Cargo.toml` owns member list and shared package version

No child AGENTS.md yet — crates are single-file modules with no durable local
contracts of their own; add child docs when a crate gains its own workflow.
