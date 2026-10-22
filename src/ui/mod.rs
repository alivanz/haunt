//! Drawing. `pane_view` renders terminal contents, `sidebar` the tab tree;
//! this module splits the screen and draws pane dividers.

pub mod help;
pub mod pane_view;
pub mod sidebar;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::layout::{Divider, Split};

/// Split the screen into (sidebar, main area). The sidebar gives way when
/// the window is very narrow, and takes no space when hidden.
pub fn screen_areas(area: Rect, show_sidebar: bool) -> (Rect, Rect) {
    let side = if show_sidebar {
        sidebar::WIDTH.min(area.width / 3)
    } else {
        0
    };
    let sidebar = Rect {
        width: side,
        ..area
    };
    let main = Rect {
        x: area.x + side,
        width: area.width - side,
        ..area
    };
    (sidebar, main)
}

/// Draw split dividers. Segments touching the focused pane are highlighted
/// so it's visible which pane has focus.
pub fn render_dividers(dividers: &[Divider], focused: Option<Rect>, buf: &mut Buffer) {
    let normal = Style::new().fg(Color::DarkGray);
    let active = Style::new().fg(Color::Cyan);
    for d in dividers {
        let symbol = match d.split {
            Split::Vertical => "│",
            Split::Horizontal => "─",
        };
        for y in d.area.top()..d.area.bottom() {
            for x in d.area.left()..d.area.right() {
                let touches = focused.is_some_and(|r| touches_rect(r, x, y));
                buf[(x, y)]
                    .set_symbol(symbol)
                    .set_style(if touches { active } else { normal });
            }
        }
    }
}

/// Whether cell (x, y) sits directly next to `r` (on any side).
fn touches_rect(r: Rect, x: u16, y: u16) -> bool {
    let in_cols = x >= r.left() && x < r.right();
    let in_rows = y >= r.top() && y < r.bottom();
    (in_rows && (x + 1 == r.left() || x == r.right()))
        || (in_cols && (y + 1 == r.top() || y == r.bottom()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_areas_reserve_sidebar() {
        let (s, m) = screen_areas(Rect::new(0, 0, 100, 30), true);
        assert_eq!(s, Rect::new(0, 0, 32, 30));
        assert_eq!(m, Rect::new(32, 0, 68, 30));
        // Narrow window: sidebar shrinks to a third.
        let (s, m) = screen_areas(Rect::new(0, 0, 30, 10), true);
        assert_eq!((s.width, m.width), (10, 20));
        // Hidden: main area gets everything.
        let (s, m) = screen_areas(Rect::new(0, 0, 100, 30), false);
        assert_eq!((s.width, m), (0, Rect::new(0, 0, 100, 30)));
    }

    #[test]
    fn divider_touching() {
        let r = Rect::new(5, 5, 10, 4);
        assert!(touches_rect(r, 4, 6)); // left edge
        assert!(touches_rect(r, 15, 8)); // right edge
        assert!(touches_rect(r, 7, 9)); // below
        assert!(!touches_rect(r, 4, 9)); // diagonal corner
        assert!(!touches_rect(r, 20, 6));
    }
}
