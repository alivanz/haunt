# haunt

A small terminal multiplexer that runs inside your terminal and shows tabs
as a tree in a sidebar on the left.

```
 ▾ tab 1               │➜  ~ vim                │➜  ~ htop
   ├ vim               │                        │
   └ htop              │                        │
 ▸ build               │                        │
 ctrl+shift+h: help    │                        │
```

## Build & run

```sh
cargo build --release
./target/release/haunt
```

To run `haunt` from anywhere, install it to `~/.cargo/bin`:

```sh
./install.sh
```

Run `./install.sh` again after changing the source; the installed binary
doesn't update by itself.

`cargo test` runs the unit tests (layout math, sidebar tree/selection, key
and mouse encoding).

## Keybindings

Every command is a single Ctrl+Shift chord.

| Keys                   | Action                                   |
|------------------------|------------------------------------------|
| `Ctrl+Shift+T`         | new tab                                  |
| `Ctrl+Shift+W`         | close focused pane (closes tab if last)  |
| `Ctrl+Shift+\|`        | split: new pane to the right             |
| `Ctrl+Shift+_`         | split: new pane below                    |
| `Ctrl+Shift+↑/↓`      | previous/next tab                        |
| `Ctrl+Shift+←/→`      | previous/next pane in this tab           |
| `Ctrl+Shift+R`         | rename tab (Enter ok, Esc cancel, Ctrl+u clear) |
| `Ctrl+Shift+S`         | focus sidebar (press again to leave)     |
| `Ctrl+Shift+B`         | hide/show sidebar                        |
| `Ctrl+Shift+H`         | show this list in a popup                |
| `Ctrl+Shift+Q`         | quit                                     |

These need a terminal that speaks the kitty keyboard protocol (Ghostty,
kitty, WezTerm, foot...), which haunt switches on at startup; elsewhere
Ctrl+Shift+T is indistinguishable from Ctrl+T. If a shortcut does nothing,
check that Ghostty doesn't have its own keybind on it.

In the sidebar: `j`/`k` (or arrows) move, `Enter` switches to the selected
tab/pane, `Space` folds/unfolds a tab, `Esc` goes back to the pane.

Mouse: click a tab or pane in the sidebar to switch to it (click the `▸`/`▾`
arrow to fold), click a pane to focus it, scroll the wheel over a pane to
scroll its history. Programs that ask for the mouse (vim `mouse=a`, htop,
lazygit) get mouse events directly. To select text with the terminal's
native selection, hold Shift while dragging (in Ghostty).

When a shell exits its pane closes; when the last pane closes, haunt exits.

## Code map

- `src/main.rs` – terminal setup/restore (also on panic)
- `src/app.rs` – state (tabs, panes, modes) and the event loop
- `src/layout.rs` – split tree and pane rectangles
- `src/pane.rs` – PTY + shell + alacritty_terminal emulator per pane
- `src/input.rs` – shortcuts (`shortcut()`); key/mouse → escape sequence encoding
- `src/ui/sidebar.rs` – sidebar tree rows, selection, truncation, drawing
- `src/ui/pane_view.rs` – draws an emulator grid into the ratatui buffer
- `src/ui/help.rs` – the Ctrl+Shift+H popup (its text is `HELP` in `input.rs`)
- `src/ui/mod.rs` – screen split and pane dividers

Defaults (sidebar width, scrollback size, colors, keys) are constants near the
top of the relevant module.
