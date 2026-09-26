<div align="center">

# ⛴ Herdr Ferry

**Move live panes and tabs, or merge whole Herdr workspaces.**

[![Herdr 0.8.0+](https://img.shields.io/badge/Herdr-0.8.0%2B-6c71c4)](https://herdr.dev)
[![Rust](https://img.shields.io/badge/built%20with-Rust-b7410e)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

</div>

Ferry is a native Rust popup for the occasional move that should be deliberate but painless. It has no `fzf`, Node, or shell-script dependency.

Press `prefix+m`, then make three choices:

1. Move panes, move whole tabs, or merge a workspace.
2. Accept the focused pane/current tab/current workspace, or fuzzy-search every live source.
3. Pick the destination.

Pane and tab source screens use `fzf`-style multi-selection without depending on `fzf`: press `Space` or `Tab` to check rows and `Ctrl-a` to check every visible match. Pressing `Enter` without checking anything keeps the highlighted row as the single default.

Pane moves can target any existing tab, a new tab in any workspace, or a new workspace. Whole-tab moves target another workspace or a new one. Workspace merge appends every source tab to an existing destination in tab order; Herdr removes the source workspace once its final live pane has moved. The popup stays session-modal while you choose, so it never alters the tiled layout.

## Install

Requires macOS or Linux (including WSL), Herdr 0.8.0 or newer, and a Rust toolchain. Install and enable Ferry, then add its conflict-checked keybinding:

On Windows, run Herdr and this plugin inside WSL. Native Windows is not supported.

```sh
herdr plugin install shadowfax92/herdr-ferry --yes
herdr plugin action invoke shadowfax.ferry.install-keybindings
```

The installer adds this conflict-checked binding to `~/.config/herdr/config.toml`, creates a backup before changing an existing config, and reloads Herdr:

```toml
[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "shadowfax.ferry.open"
description = "Move panes or tabs, or merge workspaces with Ferry"
```

It preserves unrelated configuration, is idempotent, and refuses to replace an occupied built-in or custom key.

To work on a local checkout instead:

```sh
cargo build --release --locked
herdr plugin link . --enabled
```

## Controls

| Key | Action |
| --- | --- |
| `p` / `t` / `w` | Choose panes, tabs, or workspace merge on the first screen |
| Type | Fuzzy-filter sources or destinations |
| `Up` / `Down` | Navigate results |
| `Space` / `Tab` | Toggle a pane or tab source and advance |
| `Shift-Tab` | Toggle a pane or tab source and move back |
| `Ctrl-a` | Toggle all visible pane or tab sources |
| `Enter` | Continue with checked rows, or use the highlighted row by itself |
| `Alt-d` | Move a pane into an existing tab with a down split |
| `Esc` | Go back one screen, then close |
| `Ctrl-c` | Close immediately |

Typing on the destination screen names a new tab or workspace when its `＋` row is chosen. Existing matches stay above the creation rows.

## How whole-tab moves work

Herdr exposes live pane moves but no atomic cross-workspace tab move. Ferry reads the source tab's reported pane rectangles and split ratios, validates that every pane still belongs to that tab, moves one live pane into a new destination tab, then replays the split tree around it.

Pane processes, shells, scrollback, and running agents are relocated rather than restarted. Cross-workspace pane IDs can change; Ferry follows the IDs returned by each move before placing the next pane.

A whole-tab move is necessarily a short sequence of server operations. Ferry preflights every selected tab before moving the first pane. If a later operation fails, Ferry leaves every process alive and reports exactly how many complete tabs and panes reached the destination; it does not attempt a risky automatic rollback.

## Workspace merge and named sessions

Ferry merges workspaces inside the current Herdr session. It preserves each tab as a tab, appends them in source order, and uses the same layout-preserving batch engine described above.

Herdr named sessions are separate server processes and Herdr 0.8 has no cross-session pane-transfer command. Ferry therefore does not claim to merge named sessions; that would require a transfer primitive in Herdr itself.

## Close and Clear

The move shortcut stays `prefix+m`. Open the separate destructive workflow with:

```sh
herdr plugin action invoke shadowfax.ferry.open-close
herdr plugin action invoke shadowfax.ferry.clear-ft
```

Close panes, Close tabs, and Close workspaces fuzzy-search live targets and support
Space/Tab/Shift-Tab selection and Ctrl-a for visible matches. Clear workspace
selects exactly one workspace. Enter opens a review of workspace/tab/pane IDs,
terminal identities, foreground program names and PIDs, and cwd. Scroll the review
with arrow or page keys; type `close` or `clear` and press Enter to confirm. Esc
goes back and Ctrl-c cancels before confirmation. F5 refreshes the target list.

`clear-ft` finds the exact label `ft` in the current Herdr session. One match opens
its review directly; duplicate labels require choosing one workspace by ID. A
missing label produces an actionable notification. It never creates a session
named `ft` or silently clears several workspaces.

Clear snapshots old tabs, creates a new `shell` tab with `--no-focus`, verifies its
live identity, and only then closes reviewed terminals. The shell uses the first
reviewed pane's cwd (or the user's home when none was reported). Its workspace ID
stays the same; concurrent new tabs and the keeper are excluded from closure.
A failed keeper creation leaves old terminals untouched. A partial failure leaves
the keeper in place.

Herdr removes empty tabs and workspaces when their last pane closes. Ferry uses
that behavior instead of broad tab/workspace deletion, so it never expands a
confirmed selection to include new descendants. Closing the last pane of a root worktree workspace can cascade to its linked
workspaces. Ferry rejects closing all panes of that root while linked workspaces
exist, even if Herdr confirmation is disabled: close the linked workspaces first
or use Clear. Ferry never authorizes an implicit whole-group close. There is no undo for terminated processes.

Before each mutation Ferry checks live canonical IDs, terminal IDs, selected
membership, and shell/foreground process identity. Moved or replaced targets,
new members of selected old tabs, or changed foreground programs stop remaining
work for a fresh review. Already absent terminals are skipped. Herdr does not
provide an atomic compare-and-close API: an external change in the small interval
between validation and closure remains possible. Foreground inspection does not
list every detached/background descendant of a terminal.

Execution is a Herdr-owned plugin action (`shadowfax.ferry.execute-close`), so
closing the invoking pane, tab, workspace, or popup cannot terminate the worker.
The caller is ordered last. After confirmation, closing the popup does **not**
cancel execution. Completed, already-closed, and failed pane counts appear in the
result and notification. Private snapshots and full error reports are retained in
`$HERDR_PLUGIN_STATE_DIR/close-jobs/` (`*.report.txt` and `*.result.json`). Claimed
requests are never retried automatically; a `*.running.json` without a result
means execution is still running or was interrupted and must be inspected before
starting a fresh review. Unclaimed confirmations expire after 60 seconds.

Menu can launch the workflows without duplicating their confirmation or close
logic. Add these entries to your Menu config, choosing unused keys:

```toml
[[items]]
type = "shell"
key = "k"
label = "Close…"
command = '"$HERDR_BIN_PATH" plugin action invoke shadowfax.ferry.open-close'
mode = "detached"

[[items]]
type = "shell"
key = "F"
label = "Clear ft…"
command = '"$HERDR_BIN_PATH" plugin action invoke shadowfax.ferry.clear-ft'
mode = "detached"
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python3 tests/real_herdr.py --evidence-dir /tmp/ferry-test-evidence
```

The optional real-Herdr gate starts its own named server and background PTY client
with isolated config/state, links only that test environment, and stops only its
owned session. It checks real process death, keeper/cwd preservation, unselected
terminal/focus preservation, and worker survival when its invoker closes.

## Remove

Delete Ferry's `[[keys.command]]` block, reload Herdr, then uninstall it:

```sh
herdr server reload-config
herdr plugin uninstall shadowfax.ferry
```

Use `herdr plugin unlink shadowfax.ferry` instead for a local checkout.

## License

[MIT](LICENSE)
