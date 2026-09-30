# moon-down

A terminal download manager: a ratatui TUI driving a self-owned local `aria2c`
child over JSON-RPC, with package-based queue semantics, per-host premium
accounts, and completion-gated auto-extraction.

## Status

v1 runs. The binary lives in `crates/ui` and shows the queue, a detail pane, a
history pane of already-downloaded packages, and a log tail. It now drives a
real managed-local `aria2c` when present, with a clear DEMO fallback when not.

```bash
nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#aria2 -c cargo build
./target/debug/moon-down                          # state in $XDG_STATE_HOME/moon-down
./target/debug/moon-down --state-dir /tmp/md-demo # throwaway state
./target/debug/moon-down --aria-bin /run/current-system/sw/bin/aria2c  # explicit
```

Keys: `1`-`6` views, `j`/`k`/`gg`/`G`/`Ctrl-D/U/F/B` + arrows move, `Enter`/`h`/`l` expand/collapse, `space` pause/resume, `d`
remove, `D` delete with files, `r` retry, `x` prune, `e` retry extraction, `a`
add (live input, one URI per line), `q` or `Ctrl-C` quit.

With `aria2c` on `PATH` the status bar shows `LIVE aria2c 127.0.0.1:PORT` and ticks
a single authenticated POST batch (`tellActive/tellWaiting/tellStopped/getGlobalStat`);
without it the bar shows `DEMO: simulated progress` and the log explains how to
install (`nix shell nixpkgs#aria2`). History stays visible from `state.json` in
both modes.

| crate | what it owns |
| --- | --- |
| `crates/core` | queue state, persistence, settings, accounts, plugin trait, extraction logic |
| `crates/engine` | managed-local `aria2c` child: spawn, poll, enqueue, stop, respawn, extract orchestration |
| `crates/ui` | ratatui queue inspector, settings screen, and the `moon-down` binary |

The full v1 contract lives in [issue #14](https://github.com/cernoh/moon-down/issues/14).
Its tickets are tracked in issues #15–#20.

## Design in one screen

- **Engine** — the app spawns its own `aria2c` with a per-run ephemeral secret
  written to a mode-0600 conf file, on the first free port in 6800–6899 bound to
  localhost. A lock file on the state dir makes a second instance exit rather
  than corrupt the session. The child is tied to the TUI pid, so it dies with
  the UI even on `SIGKILL`. An unexpected death respawns once with the session
  file, then shows a visible error instead of looping.
- **Queue** — a package is the add unit: one paste is one package, each
  torrent or metalink file is its own. A package's status is the worst case of
  its members (error > active > paused > complete), progress is completed bytes
  over total bytes, and status and progress are always derived, never cached.
  `user:pass@` credentials are stripped from URIs at enqueue time, so no secret
  reaches state, logs, or the session file.
- **Persistence** — one JSON state file written atomically (temp file plus
  rename), on a debounce after structural changes, on a 30 s interval, and on
  shutdown. The app never rebuilds packages by scanning the download directory.
- **Accounts** — one record per host (plugin, host, username, enabled) with the
  secret in the OS keyring under `moon-down:{plugin}:{host}`. Secrets are mapped
  to HTTP options at send time and never embedded in a URI. A missing keyring on
  a headless host is a loud error, not a silent downgrade.
- **Extraction** — starts once every member of a package is complete, live feed
  first with a poll-tick fallback. Zip and the tar family extract in Rust; 7z
  and rar go through a detected `7zz`/`7z` binary (`unrar` is never used). A
  missing binary is an `ExtractFailed` with an install hint, and failures keep
  the archives for manual retry.

## Nix flake

Consumable from any flake:

```nix
# flake.nix inputs
inputs.moon-down.url = "github:cernoh/moon-down";
```
```bash
nix run github:cernoh/moon-down -- --help
nix profile install github:cernoh/moon-down
# overlay
nixpkgs.overlays = [ inputs.moon-down.overlays.default ]; # -> pkgs.moon-down
```

Local:

```bash
nix build .#moon-down        # -> result/bin/moon-down
nix run . -- --help
nix flake check --all-systems
```

## Build and test

`cargo` is not on the default `PATH` in this dev environment:

```bash
nix shell nixpkgs#cargo nixpkgs#rustc -c cargo test
```

Tests run in parallel by default and must never mutate process-global state to
force a condition — pass the value in instead. Mutating `PATH` races sibling
tests that spawn real binaries. See `AGENTS.md` for the rest of the contract.

## Working on it

Branches use [worktrunk](https://github.com/pitrak/worktrunk):

```bash
wt switch --create issue-21-something
# ... work ...
wt merge main          # squash, rebase, fast-forward main, drop the worktree
```

Do not commit on `main` directly.

## Licence

MIT — see [LICENSE](LICENSE).
