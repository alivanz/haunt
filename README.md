# haunt

A small terminal multiplexer that runs inside your terminal and shows your
tabs as a tree in a sidebar on the left, with each tab's panes listed under it.

```
 ▾ dev                          │➜  ~ vim main.rs        │➜  ~ htop
   ├ vim main.rs                │                        │
   └ htop                       │                        │
 ▸ logs                         │                        │
 ▾ git                          │                        │
   └ lazygit                    │                        │
                                │                        │
 ctrl+shift+h: help             │                        │
```

haunt is deliberately simple: there's no config file, no themes, and no
detach/attach. Everything is a constant in the source, so change the code if
you want different behavior.

## Features

- **Tree-style tab sidebar.** Fold and unfold tabs, click to switch, and long
  names are cut off with "…".
- **Splits.** Put panes side by side or stacked, and move between them.
- **Real terminal emulation** via [alacritty_terminal](https://crates.io/crates/alacritty_terminal):
  vim, htop, less and lazygit work, with truecolor, alternate screen, mouse
  reporting and cursor shapes.
- **Per-pane scrollback** (10,000 lines) with the mouse wheel.
- **Mouse support.** Click tabs and panes in the sidebar, click a pane to
  focus it, and scroll over any pane.
- **Single-chord shortcuts** (Ctrl+Shift+…), with no prefix key to press first.
- **Always restores your terminal** on exit, even if haunt crashes.

## Requirements

- **Rust 1.88 or newer** ([rustup.rs](https://rustup.rs)).
- **macOS or Linux.** It has been tested on macOS. Linux should work, and
  Windows isn't supported.
- **A terminal that supports the
  [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)**,
  such as Ghostty, kitty, WezTerm, foot or Alacritty. haunt needs it to tell
  Ctrl+Shift+T apart from Ctrl+T. In terminals without it, such as macOS
  Terminal.app, the shortcuts won't work.

## Install

```sh
git clone https://github.com/alivanz/haunt.git
cd haunt
./install.sh
```

`install.sh` builds haunt and installs the `haunt` binary to `~/.cargo/bin`,
which rustup normally adds to your `PATH`. Then run it from anywhere:

```sh
haunt
```

If you've changed the source, run `./install.sh` again to install the new
build. To try changes without installing, run `cargo run --release` in the
repo instead.

To uninstall, run `cargo uninstall haunt`.

## Usage

haunt starts with one tab running your login shell. Press **Ctrl+Shift+H**
at any time to see all the shortcuts.

| Keys             | Action                                           |
|------------------|--------------------------------------------------|
| `Ctrl+Shift+T`   | new tab                                          |
| `Ctrl+Shift+W`   | close the focused pane (closes the tab if it's the last pane) |
| `Ctrl+Shift+\|`  | split: new pane to the right                     |
| `Ctrl+Shift+_`   | split: new pane below                            |
| `Ctrl+Shift+↑/↓` | previous / next tab                              |
| `Ctrl+Shift+←/→` | previous / next pane in the current tab          |
| `Ctrl+Shift+R`   | rename the current tab (Enter saves, Esc cancels, Ctrl+U clears) |
| `Ctrl+Shift+S`   | focus the sidebar (press again to leave)         |
| `Ctrl+Shift+B`   | hide / show the sidebar                          |
| `Ctrl+Shift+H`   | show the shortcuts popup                         |
| `Ctrl+Shift+Q`   | quit                                             |

**In the sidebar:** `j`/`k` or the arrow keys move, `Enter` switches to the
selected tab or pane, `Space` folds or unfolds a tab, and `Esc` returns to
the panes.

**Mouse:**
- Click a tab or pane in the sidebar to switch to it, or click the `▸`/`▾`
  arrow to fold the tab.
- Click a pane to focus it.
- Scroll over a pane to move through its history. A yellow `[n/total]` marker
  shows how far back you are, and typing jumps back to the bottom.
- Programs that use the mouse (vim with `mouse=a`, htop, lazygit) receive
  clicks and scrolls directly.
- To select text with your terminal's own selection, hold **Shift** while
  dragging.

When a shell exits, its pane closes. When the last pane closes, haunt exits.

If a shortcut does nothing, your terminal probably has its own keybinding on
that key. Remove that binding in the terminal's settings, or change the key
in `src/input.rs`.

## Customizing

There's no config file. Edit the source and run `./install.sh` again.

| What                    | Where                                               |
|-------------------------|-----------------------------------------------------|
| Shortcuts               | `shortcut()` in `src/input.rs`; also update the `HELP` list next to it |
| Sidebar width           | `WIDTH` in `src/ui/sidebar.rs`                      |
| Scrollback length       | `SCROLLBACK` in `src/pane.rs`                       |
| Lines per scroll tick   | `SCROLL_LINES` in `src/app.rs`                      |
| Sidebar colors          | `render()` in `src/ui/sidebar.rs`                   |

## Code map

- `src/main.rs`: sets up the outer terminal and restores it on exit and on panic
- `src/app.rs`: app state (tabs, panes, modes) and the event loop
- `src/layout.rs`: the split tree and pane rectangles
- `src/pane.rs`: one PTY, shell and alacritty_terminal emulator per pane
- `src/input.rs`: shortcuts, and turning keys and mouse events into escape sequences
- `src/ui/sidebar.rs`: sidebar rows, selection, truncation and drawing
- `src/ui/pane_view.rs`: draws a terminal grid into the ratatui buffer
- `src/ui/help.rs`: the Ctrl+Shift+H popup
- `src/ui/mod.rs`: splits the screen and draws pane dividers

Run `cargo test` for the unit tests, which cover layout math, the sidebar
tree and selection, and key/mouse encoding.

## Known limitations

- Splits are always 50/50 and can't be resized.
- New panes start in the directory haunt was launched from.
- Pane names come from the title the running program sets. Shells that
  don't set one show "shell".
- No copy mode. Use your terminal's Shift+drag selection.
- Clipboard writes from programs (OSC 52) aren't passed to the outer terminal.

## Built with

[ratatui](https://ratatui.rs) · [crossterm](https://crates.io/crates/crossterm) ·
[portable-pty](https://crates.io/crates/portable-pty) ·
[alacritty_terminal](https://crates.io/crates/alacritty_terminal)

## License

[MIT](LICENSE)
