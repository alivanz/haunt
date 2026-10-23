//! Keybindings, plus encoding of keys and mouse events into the byte
//! sequences a program running in a PTY expects.

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/// Commands bound to Ctrl+Shift shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NewTab,
    ClosePane,
    /// Panes side by side, divided by a vertical line.
    SplitVertical,
    /// Panes stacked, divided by a horizontal line.
    SplitHorizontal,
    RenameTab,
    FocusSidebar,
    ToggleSidebar,
    Help,
    /// Cycle through tabs (Ctrl+Shift+Up/Down), wrapping around.
    PrevTab,
    NextTab,
    /// Cycle through the current tab's panes (Ctrl+Shift+Left/Right).
    PrevPane,
    NextPane,
    Quit,
}

/// Map a Ctrl+Shift+<key> press to a command.
///
/// Telling Ctrl+Shift+T apart from Ctrl+T needs the kitty keyboard protocol,
/// which `main.rs` turns on. The terminal then reports the unshifted key with
/// the Shift modifier (`t`, `\`, `-`); the shifted forms (`T`, `|`, `_`)
/// are accepted too in case a terminal sends those instead.
pub fn shortcut(key: &KeyEvent) -> Option<Action> {
    let mods = key.modifiers;
    if !mods.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        || mods.contains(KeyModifiers::ALT)
    {
        return None;
    }
    let c = match key.code {
        KeyCode::Up => return Some(Action::PrevTab),
        KeyCode::Down => return Some(Action::NextTab),
        KeyCode::Left => return Some(Action::PrevPane),
        KeyCode::Right => return Some(Action::NextPane),
        KeyCode::Char(c) => c,
        _ => return None,
    };
    Some(match c.to_ascii_lowercase() {
        't' => Action::NewTab,
        'w' => Action::ClosePane,
        '\\' | '|' => Action::SplitVertical,
        '-' | '_' => Action::SplitHorizontal,
        'h' => Action::Help,
        'r' => Action::RenameTab,
        's' => Action::FocusSidebar,
        'b' => Action::ToggleSidebar,
        'q' => Action::Quit,
        _ => return None,
    })
}

/// Lines of the Ctrl+Shift+H help popup. Keep in sync with `shortcut`.
pub const HELP: &[(&str, &str)] = &[
    ("Ctrl+Shift+T", "new tab"),
    ("Ctrl+Shift+W", "close pane"),
    ("Ctrl+Shift+|", "split right"),
    ("Ctrl+Shift+_", "split down"),
    ("Ctrl+Shift+↑/↓", "previous/next tab"),
    ("Ctrl+Shift+←/→", "previous/next pane"),
    ("Ctrl+Shift+R", "rename tab"),
    ("Ctrl+Shift+S", "focus sidebar"),
    ("Ctrl+Shift+B", "hide/show sidebar"),
    ("Ctrl+Shift+H", "this help"),
    ("Ctrl+Shift+Q", "quit (a -s session is kept)"),
    ("", ""),
    ("sidebar", "j/k move, ⏎ go, ␣ fold, esc back"),
    ("mouse", "click to switch, wheel scrolls"),
];

/// Encode a key press for the program. Follows xterm conventions, which is
/// what TERM=xterm-256color promises.
pub fn encode_key(key: &KeyEvent, mode: TermMode) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    // Application cursor mode (DECCKM, used by vim, less, ...) changes
    // arrow keys from CSI A to SS3 A.
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
    let modifier_param =
        1 + mods.contains(KeyModifiers::SHIFT) as u8 + (alt as u8) * 2 + (ctrl as u8) * 4;

    // Cursor-style keys: CSI <final>, SS3 <final> or CSI 1;<mod> <final>.
    let cursor_key = |c: char| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[1;{modifier_param}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    // Tilde-style keys: CSI <n> ~ or CSI <n>;<mod> ~.
    let tilde_key = |n: u8| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[{n};{modifier_param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let mut bytes = match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                ctrl_char(c)?
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![if ctrl { 0x08 } else { 0x7f }],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => cursor_key('A'),
        KeyCode::Down => cursor_key('B'),
        KeyCode::Right => cursor_key('C'),
        KeyCode::Left => cursor_key('D'),
        KeyCode::Home => cursor_key('H'),
        KeyCode::End => cursor_key('F'),
        KeyCode::Insert => tilde_key(2),
        KeyCode::Delete => tilde_key(3),
        KeyCode::PageUp => tilde_key(5),
        KeyCode::PageDown => tilde_key(6),
        KeyCode::F(n @ 1..=4) => {
            let c = (b'P' + n - 1) as char;
            if modifier_param > 1 {
                format!("\x1b[1;{modifier_param}{c}").into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde_key([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        _ => return None,
    };

    // Alt sends an ESC prefix, but only for keys that didn't already fold
    // the modifier into their escape sequence.
    if alt
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Ctrl+<char> as a C0 control byte.
fn ctrl_char(c: char) -> Option<Vec<u8>> {
    let b = match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return Some(c.to_string().into_bytes()),
    };
    Some(vec![b])
}

/// Encode a mouse event for a program that enabled mouse reporting.
/// `col`/`row` are 0-based and relative to the pane. Returns `None` if the
/// program's mouse mode doesn't ask for this kind of event.
pub fn encode_mouse(
    kind: MouseEventKind,
    mods: KeyModifiers,
    col: u16,
    row: u16,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    // (button code, is release)
    let (mut code, release) = match kind {
        MouseEventKind::Down(b) => (button_code(b), false),
        MouseEventKind::Up(b) => (button_code(b), true),
        MouseEventKind::Drag(b) => {
            if !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
                return None;
            }
            (button_code(b) + 32, false)
        }
        MouseEventKind::Moved => {
            if !mode.contains(TermMode::MOUSE_MOTION) {
                return None;
            }
            (3 + 32, false)
        }
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mods.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        code += 16;
    }

    let (x, y) = (col as u32 + 1, row as u32 + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
    } else {
        // Legacy X10 encoding: release has no button info, and coordinates
        // are single bytes offset by 32, so they cap out at 223.
        let code = if release { 3 + (code & !3) } else { code };
        let enc = |v: u32| (v.min(223) + 32) as u8;
        Some(vec![0x1b, b'[', b'M', 32 + code as u8, enc(x), enc(y)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn shortcuts_need_ctrl_and_shift() {
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(shortcut(&key(KeyCode::Char('t'), cs)), Some(Action::NewTab));
        assert_eq!(shortcut(&key(KeyCode::Char('T'), cs)), Some(Action::NewTab));
        assert_eq!(
            shortcut(&key(KeyCode::Char('\\'), cs)),
            Some(Action::SplitVertical)
        );
        assert_eq!(
            shortcut(&key(KeyCode::Char('|'), cs)),
            Some(Action::SplitVertical)
        );
        assert_eq!(
            shortcut(&key(KeyCode::Char('-'), cs)),
            Some(Action::SplitHorizontal)
        );
        assert_eq!(shortcut(&key(KeyCode::Char('h'), cs)), Some(Action::Help));
        assert_eq!(shortcut(&key(KeyCode::Down, cs)), Some(Action::NextTab));
        assert_eq!(shortcut(&key(KeyCode::Left, cs)), Some(Action::PrevPane));
        // Arrows without both modifiers belong to the program.
        assert_eq!(shortcut(&key(KeyCode::Down, KeyModifiers::CONTROL)), None);
        assert_eq!(shortcut(&key(KeyCode::Down, KeyModifiers::NONE)), None);
        // Plain Ctrl+T, Shift+T and Ctrl+Alt+Shift+T go to the program.
        assert_eq!(
            shortcut(&key(KeyCode::Char('t'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            shortcut(&key(KeyCode::Char('T'), KeyModifiers::SHIFT)),
            None
        );
        assert_eq!(
            shortcut(&key(KeyCode::Char('t'), cs | KeyModifiers::ALT)),
            None
        );
    }

    #[test]
    fn ctrl_letters_become_control_bytes() {
        let k = key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(encode_key(&k, TermMode::empty()), Some(vec![3]));
    }

    #[test]
    fn arrows_respect_app_cursor_mode() {
        let k = key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(encode_key(&k, TermMode::empty()).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(&k, TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        let k = key(KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(encode_key(&k, TermMode::APP_CURSOR).unwrap(), b"\x1b[1;5D");
    }

    #[test]
    fn alt_prefixes_escape() {
        let k = key(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(encode_key(&k, TermMode::empty()).unwrap(), b"\x1bb");
    }

    #[test]
    fn mouse_sgr_and_legacy() {
        let m = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let down = MouseEventKind::Down(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(
            encode_mouse(down, KeyModifiers::NONE, 4, 9, m).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            encode_mouse(up, KeyModifiers::NONE, 4, 9, m).unwrap(),
            b"\x1b[<0;5;10m"
        );
        let m = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            encode_mouse(up, KeyModifiers::NONE, 0, 0, m).unwrap(),
            b"\x1b[M#!!"
        );
        // Plain click mode doesn't report motion.
        assert_eq!(
            encode_mouse(MouseEventKind::Moved, KeyModifiers::NONE, 0, 0, m),
            None
        );
        // No mouse mode at all: nothing.
        assert_eq!(
            encode_mouse(down, KeyModifiers::NONE, 0, 0, TermMode::empty()),
            None
        );
    }
}
