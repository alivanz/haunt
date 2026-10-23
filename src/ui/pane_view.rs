//! Copies an alacritty `Term` grid into a ratatui buffer.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

/// Draw the visible part of the terminal (taking the scrollback offset into
/// account) into `area`. Returns the cursor position on screen, if the cursor
/// is visible.
pub fn render<L: EventListener>(term: &Term<L>, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let grid = term.grid();
    let colors = term.colors();
    let offset = grid.display_offset() as i32;
    let rows = (grid.screen_lines() as u16).min(area.height);
    let cols = (grid.columns() as u16).min(area.width);

    for y in 0..rows {
        // Line(0) is the top of the live screen; negative lines are history.
        let line = Line(y as i32 - offset);
        let row = &grid[line];
        for x in 0..cols {
            let cell = &row[Column(x as usize)];
            // The second half of a double-width char: ratatui skips it on
            // its own when the previous symbol is wide.
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let Some(out) = buf.cell_mut((area.x + x, area.y + y)) else {
                continue;
            };

            let mut symbol = String::new();
            symbol.push(printable(cell.c));
            if let Some(zw) = cell.zerowidth() {
                symbol.extend(zw);
            }
            out.set_symbol(&symbol);
            out.set_style(cell_style(cell.fg, cell.bg, cell.flags, colors));
        }
    }

    if offset != 0 {
        // Scrolled back into history: show where we are, top right.
        let label = format!("[{}/{}]", offset, grid.history_size());
        let x = area.right().saturating_sub(label.len() as u16).max(area.x);
        let style = Style::new().fg(Color::Black).bg(Color::Yellow);
        buf.set_stringn(x, area.y, &label, area.width as usize, style);
        return None;
    }
    if !term.mode().contains(TermMode::SHOW_CURSOR) {
        return None;
    }
    let Point { line, column } = grid.cursor.point;
    let (cx, cy) = (column.0 as u16, line.0 as u16);
    (cx < cols && cy < rows).then(|| Position::new(area.x + cx, area.y + cy))
}

/// The character to draw for a cell. Empty cells hold `\0`, and the cell
/// where a tab started holds `\t` (kept for copy/paste). Sending either to
/// the outer terminal would move its cursor and shift everything drawn after
/// it on that line.
pub fn printable(c: char) -> char {
    if c.is_control() { ' ' } else { c }
}

fn cell_style(fg: AnsiColor, bg: AnsiColor, flags: Flags, colors: &Colors) -> Style {
    let mut m = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        m |= Modifier::BOLD;
    }
    if flags.contains(Flags::ITALIC) {
        m |= Modifier::ITALIC;
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        m |= Modifier::UNDERLINED;
    }
    if flags.contains(Flags::DIM) {
        m |= Modifier::DIM;
    }
    if flags.contains(Flags::INVERSE) {
        m |= Modifier::REVERSED;
    }
    if flags.contains(Flags::HIDDEN) {
        m |= Modifier::HIDDEN;
    }
    if flags.contains(Flags::STRIKEOUT) {
        m |= Modifier::CROSSED_OUT;
    }
    Style::new()
        .fg(map_color(fg, colors))
        .bg(map_color(bg, colors))
        .add_modifier(m)
}

/// Map an emulator color to an outer-terminal color. The 16 ANSI colors and
/// default fg/bg are passed through as palette references so the outer
/// terminal's theme still applies, unless the program overrode the palette
/// (OSC 4), in which case we use the exact RGB it asked for.
fn map_color(color: AnsiColor, colors: &Colors) -> Color {
    match color {
        AnsiColor::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        AnsiColor::Indexed(i) => palette(i as usize, colors),
        AnsiColor::Named(named) => match named {
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => Color::Reset,
            // Dim variants (only produced internally) map back to the
            // normal color; the DIM modifier carries the dimming.
            NamedColor::DimBlack => palette(0, colors),
            NamedColor::DimRed => palette(1, colors),
            NamedColor::DimGreen => palette(2, colors),
            NamedColor::DimYellow => palette(3, colors),
            NamedColor::DimBlue => palette(4, colors),
            NamedColor::DimMagenta => palette(5, colors),
            NamedColor::DimCyan => palette(6, colors),
            NamedColor::DimWhite => palette(7, colors),
            other => palette(other as usize, colors),
        },
    }
}

fn palette(index: usize, colors: &Colors) -> Color {
    match colors[index] {
        Some(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        None => Color::Indexed(index as u8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use crate::pane::TermSize;

    #[test]
    fn tabs_are_drawn_as_spaces() {
        let size = TermSize { cols: 20, rows: 2 };
        let mut term = Term::new(Config::default(), &size, VoidListener);
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"a\tb");

        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        render(&term, area, &mut buf);
        let row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(row, format!("a{}b{}", " ".repeat(7), " ".repeat(11)));
    }
}
