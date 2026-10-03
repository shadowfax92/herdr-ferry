<div align="center">

# ⛴ Herdr Ferry

**Move, merge, or kill live Herdr panes, tabs, and workspaces.**

[![Herdr 0.8.0+](https://img.shields.io/badge/Herdr-0.8.0%2B-6c71c4)](https://herdr.dev)
[![Rust](https://img.shields.io/badge/built%20with-Rust-b7410e)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

</div>

Ferry is a native Rust popup for the occasional move or cleanup that should be deliberate but painless. It has no `fzf`, Node, or shell-script dependency.

Press `prefix+m`, then make three choices:

1. Move panes, move whole tabs, or merge a workspace — or kill panes, tabs, or workspaces.
2. Accept the focused pane/current tab/current workspace, or fuzzy-search every live one.
3. Pick the destination, or review the kill and confirm it with `y`.

Pane and tab sources, and every kill picker, use `fzf`-style multi-selection without depending on `fzf`: press `Space` or `Tab` to check rows and `Ctrl-a` to check every visible match. Pressing `Enter` without checking anything keeps the highlighted row as the single default.

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
| `p` / `t` / `w` | Move panes, move tabs, or merge a workspace from the first screen |
| `P` / `T` / `W` | Kill panes, tabs, or workspaces from the first screen |
| Type | Fuzzy-filter sources, destinations, or kill targets |
| `Up` / `Down` | Navigate results |
| `Space` / `Tab` | Toggle a row and advance (pane and tab sources, kill pickers) |
| `Shift-Tab` | Toggle a row and move back |
| `Ctrl-a` | Toggle all visible rows |
| `Enter` | Continue with checked rows, or use the highlighted row by itself; never kills |
| `y` | Kill everything on the review screen |
| `n` | Leave the review screen with selections kept |
| `Alt-d` | Move a pane into an existing tab with a down split |
| `Esc` | Go back one screen, then close |
| `Ctrl-c` | Close immediately |

Typing on the destination screen names a new tab or workspace when its `＋` row is chosen. Existing matches stay above the creation rows.

## Killing panes, tabs, and workspaces

Choose a kill row (or press `P`, `T`, or `W`), check targets, and press `Enter` to review them. The review lists every target and what goes with it: contained tabs and panes, agents that are working or blocked, and whether the pane Ferry was opened from is included. Only `y` kills; `n` or `Esc` returns to the picker with your selection intact.

A kill is Herdr's own close: it ends every process running in each pane, and a tab or workspace disappears with its last pane. On `y`, Ferry hands the confirmed list to a detached process in its own session and closes the popup, so killing the tab or workspace you opened Ferry from still completes every confirmed close. The outcome arrives as a Herdr notification.

Right before closing, Ferry re-reads Herdr. Targets that are already gone are counted, not failed; a tab or workspace that gained a pane after your review is skipped; one failure does not stop the rest. Kills are not atomic, and killed processes cannot be restored.

Closing a worktree root workspace also closes its linked worktree workspaces. Ferry refuses a kill that would do that implicitly. Select the linked workspaces as well (Ferry closes them before the root), or kill them first. If a linked workspace is still open when Ferry reaches its root, because that workspace's own close was skipped or failed, the root is skipped too.

## How whole-tab moves work

Herdr exposes live pane moves but no atomic cross-workspace tab move. Ferry reads the source tab's reported pane rectangles and split ratios, validates that every pane still belongs to that tab, moves one live pane into a new destination tab, then replays the split tree around it.

Pane processes, shells, scrollback, and running agents are relocated rather than restarted. Cross-workspace pane IDs can change; Ferry follows the IDs returned by each move before placing the next pane.

A whole-tab move is necessarily a short sequence of server operations. Ferry preflights every selected tab before moving the first pane. If a later operation fails, Ferry leaves every process alive and reports exactly how many complete tabs and panes reached the destination; it does not attempt a risky automatic rollback.

## Workspace merge and named sessions

Ferry merges workspaces inside the current Herdr session. It preserves each tab as a tab, appends them in source order, and uses the same layout-preserving batch engine described above.

Herdr named sessions are separate server processes and Herdr 0.8 has no cross-session pane-transfer command. Ferry therefore does not claim to merge named sessions; that would require a transfer primitive in Herdr itself.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

## Remove

Delete Ferry's `[[keys.command]]` block, reload Herdr, then uninstall it:

```sh
herdr server reload-config
herdr plugin uninstall shadowfax.ferry
```

Use `herdr plugin unlink shadowfax.ferry` instead for a local checkout.

## License

[MIT](LICENSE)
