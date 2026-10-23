//! Turn a terminal grid (scrollback + screen) into text with ANSI color
//! codes, so a saved session can be fed back into a fresh emulator and look
//! the same.

use alacritty_terminal::Grid;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// Attribute flags that change how text looks (and so need SGR codes).
const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::DIM)
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::INVERSE)
    .union(Flags::HIDDEN)
    .union(Flags::STRIKEOUT);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: Flags,
}

const DEFAULT_PEN: Pen = Pen {
    fg: Color::Named(NamedColor::Foreground),
    bg: Color::Named(NamedColor::Background),
    flags: Flags::empty(),
};

/// The whole grid, oldest line first. Lines that wrapped are joined, so the
/// text re-wraps to whatever width it is replayed at. Trailing blank lines
/// are dropped.
pub fn to_ansi(grid: &Grid<Cell>) -> String {
    let top = grid.topmost_line().0;
    let Some(last) = (top..=grid.bottommost_line().0)
        .rev()
        .find(|&l| !is_blank_line(grid, Line(l)))
    else {
        return String::new();
    };

    let cols = grid.columns();
    let mut out = String::new();
    let mut pen = DEFAULT_PEN;
    for l in top..=last {
        let row = &grid[Line(l)];
        let wrapped = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        // A wrapped line's trailing spaces are real content; otherwise
        // stop after the last non-blank cell.
        let end = if wrapped {
            cols
        } else {
            (0..cols)
                .rev()
                .find(|&c| !is_blank(&row[Column(c)]))
                .map_or(0, |c| c + 1)
        };
        for c in 0..end {
            let cell = &row[Column(c)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let cell_pen = Pen {
                fg: cell.fg,
                bg: cell.bg,
                flags: cell.flags & STYLE_FLAGS,
            };
            if cell_pen != pen {
                push_sgr(&mut out, cell_pen);
                pen = cell_pen;
            }
            out.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(zw) = cell.zerowidth() {
                out.extend(zw);
            }
        }
        if !wrapped && l != last {
            // Reset before the newline so a background color doesn't bleed
            // into the next line.
            if pen != DEFAULT_PEN {
                out.push_str("\x1b[0m");
                pen = DEFAULT_PEN;
            }
            out.push_str("\r\n");
        }
    }
    if pen != DEFAULT_PEN {
        out.push_str("\x1b[0m");
    }
    out
}

fn is_blank(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\0')
        && cell.bg == Color::Named(NamedColor::Background)
        && !cell
            .flags
            .intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
}

fn is_blank_line(grid: &Grid<Cell>, line: Line) -> bool {
    let row = &grid[line];
    (0..grid.columns()).all(|c| is_blank(&row[Column(c)]))
}

/// One SGR sequence that resets, then sets everything in `pen`.
fn push_sgr(out: &mut String, pen: Pen) {
    let mut codes = vec!["0".to_string()];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if pen.flags.contains(flag) {
            codes.push(code.to_string());
        }
    }
    if pen.flags.intersects(Flags::ALL_UNDERLINES) {
        codes.push("4".to_string());
    }
    if let Some(c) = color_code(pen.fg, false) {
        codes.push(c);
    }
    if let Some(c) = color_code(pen.bg, true) {
        codes.push(c);
    }
    out.push_str("\x1b[");
    out.push_str(&codes.join(";"));
    out.push('m');
}

/// SGR parameters for a color, or `None` for the default color.
fn color_code(color: Color, background: bool) -> Option<String> {
    let base = if background { 40 } else { 30 };
    let ext = if background { 48 } else { 38 };
    match color {
        Color::Spec(rgb) => Some(format!("{ext};2;{};{};{}", rgb.r, rgb.g, rgb.b)),
        Color::Indexed(i) => Some(format!("{ext};5;{i}")),
        Color::Named(named) => {
            let index = match named {
                NamedColor::DimBlack => 0,
                NamedColor::DimRed => 1,
                NamedColor::DimGreen => 2,
                NamedColor::DimYellow => 3,
                NamedColor::DimBlue => 4,
                NamedColor::DimMagenta => 5,
                NamedColor::DimCyan => 6,
                NamedColor::DimWhite => 7,
                n if (n as usize) < 16 => n as usize,
                // Foreground, Background, Cursor, ...: the default color.
                _ => return None,
            };
            Some(if index < 8 {
                format!("{}", base + index)
            } else {
                format!("{}", base + 60 + index - 8) // bright: 90-97 / 100-107
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::Term;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use crate::pane::TermSize;

    fn term(cols: u16, rows: u16, input: &str) -> Term<VoidListener> {
        let mut t = Term::new(Config::default(), &TermSize { cols, rows }, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut t, input.as_bytes());
        t
    }

    #[test]
    fn plain_text_and_colors() {
        let t = term(10, 4, "hello\r\n\x1b[31mred\x1b[0m ok\r\n");
        assert_eq!(to_ansi(t.grid()), "hello\r\n\x1b[0;31mred\x1b[0m ok");
    }

    #[test]
    fn truecolor_bold_and_bright() {
        let t = term(20, 2, "\x1b[1;38;2;255;100;0mX\x1b[0;92mY");
        assert_eq!(
            to_ansi(t.grid()),
            "\x1b[0;1;38;2;255;100;0mX\x1b[0;92mY\x1b[0m"
        );
    }

    #[test]
    fn wrapped_lines_are_joined() {
        // 15 chars in a 10-column terminal wrap onto a second row.
        let t = term(10, 4, "0123456789abcde\r\nnext");
        assert_eq!(to_ansi(t.grid()), "0123456789abcde\r\nnext");
    }

    #[test]
    fn includes_scrollback() {
        let t = term(10, 2, "one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(to_ansi(t.grid()), "one\r\ntwo\r\nthree\r\nfour");
    }

    #[test]
    fn empty_grid() {
        assert_eq!(to_ansi(term(10, 3, "").grid()), "");
    }

    #[test]
    fn replay_roundtrip() {
        let input = "\x1b[44m blue bg \x1b[0m\r\n\x1b[4munder\x1b[24m plain 日本\r\nlast";
        let first = to_ansi(term(12, 3, input).grid());
        let second = to_ansi(term(12, 3, &first).grid());
        assert_eq!(first, second);
    }
}
