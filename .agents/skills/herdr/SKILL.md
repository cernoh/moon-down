---
name: herdr
description: "Drive the Herdr terminal workspace manager from the CLI - list and create workspaces, tabs, and panes, run commands in them, read output. Use when the user mentions Herdr, or asks to open a tab/workspace, run something in another pane, or read a pane's output. The installed binary is the authority; run `herdr --skill` for the full agent guide."
---

# Herdr

Herdr organizes terminals into **workspaces** -> **tabs** -> **panes** and exposes the
current session through the `herdr` CLI. Most control commands return JSON; read IDs
from responses, never guess them.

**Canonical reference:** `herdr --skill` prints the full upstream agent guide
(agents, wait semantics, alternate-screen caveats, safety rules). Read it when a task
goes beyond the recipes below. Never run bare `herdr` for discovery - it launches the TUI.

## Layout IDs

Opaque, stable, never reused: workspace `w1`, tab `w1:t1`, pane `w1:p1`.

Inherited in every managed pane:

```bash
printf '%s\n' "$HERDR_WORKSPACE_ID" "$HERDR_TAB_ID" "$HERDR_PANE_ID"
```

## Recipes

```bash
herdr workspace list                                    # id, label, focus, counts
herdr tab list --workspace "$HERDR_WORKSPACE_ID"
herdr pane list  --workspace "$HERDR_WORKSPACE_ID"

herdr tab create --workspace wA --cwd "$PWD" --no-focus # new tab in existing workspace
herdr pane split --current --direction right --cwd "$PWD" --no-focus

herdr pane run <pane-id> ls                             # sends text + Enter atomically
herdr pane read <pane-id> --source recent-unwrapped --lines 120
herdr pane wait-output <pane-id> --match "done" --timeout 120000
```

Read source: `visible` (viewport) | `recent` | `recent-unwrapped` (logs, no soft wraps)
| `detection`.

## Rules that bite

- `--no-focus` for background work unless the user asked to switch context.
- Target panes with `--current`, an explicit pane ID, or a unique agent name - never
  the UI-focused pane, which may belong to the user or another client.
- `herdr pane run` prints nothing; the output lives in the pane. Read it back.
- Alternate-screen agents (full-screen TUIs) leave no host scrollback: a bigger
  `--lines` will not recover them. Fall back to asking the agent to write a file.
- Don't close workspaces/tabs/panes you didn't create, and never `herdr server stop`
  from an active session.
- Server errors are JSON on stderr (exit 1); syntax errors exit 2.
- IDs are per-server. For a remote machine, run the commands on that host.

## Agents

`agent start` needs an existing idle shell pane and never creates layout. Agent names
match `[a-z][a-z0-9_-]{0,31}` and follow the pane occupant.

```bash
herdr agent start reviewer --kind codex --pane <pane-id>
herdr agent prompt reviewer "..." --wait --timeout 120000
herdr agent get reviewer
herdr agent read reviewer --source recent-unwrapped --lines 120
```

Lifecycle: `idle`/`done` = ready for input, `working`, `blocked` = approval or question
UI (inspect and ask the user; do not answer it yourself), `unknown` = present but
unclassified.
