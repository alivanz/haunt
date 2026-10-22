//! The tab sidebar: a tree of tabs and their panes.
//!
//! The tree is flattened into rows (`rows`), which is what selection,
//! mouse hit-testing and drawing all index into.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::pane::PaneId;

/// Total sidebar width, including the border column on its right.
pub const WIDTH: u16 = 32;

/// What the sidebar needs to know about a tab.
pub struct TabEntry<'a> {
    pub name: &'a str,
    pub collapsed: bool,
    pub active: bool,
    pub panes: Vec<PaneEntry<'a>>,
}

pub struct PaneEntry<'a> {
    pub id: PaneId,
    pub title: &'a str,
    pub focused: bool,
}

/// What a sidebar row points at. Tabs are referred to by index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tab(usize),
    Pane(usize, PaneId),
}

/// Flatten the tree into rows: each tab, followed by its panes unless the
/// tab is collapsed.
pub fn rows(tabs: &[TabEntry]) -> Vec<Target> {
    let mut out = Vec::new();
    for (i, tab) in tabs.iter().enumerate() {
        out.push(Target::Tab(i));
        if !tab.collapsed {
            out.extend(tab.panes.iter().map(|p| Target::Pane(i, p.id)));
        }
    }
    out
}

/// Move the selection by `delta` rows, clamped to the list.
pub fn move_selection(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (selected as i64 + delta as i64).clamp(0, len as i64 - 1) as usize
}

/// First visible row, chosen so `selected` stays on screen.
pub fn scroll_offset(selected: usize, height: usize) -> usize {
    if height == 0 {
        0
    } else {
        selected.saturating_sub(height - 1)
    }
}

/// Truncate `s` to at most `width` display columns, ending in "…" when cut.
pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Text of one row, without styling. Pane rows get tree connectors.
fn row_text(tabs: &[TabEntry], target: Target, width: usize) -> String {
    let (prefix, label) = match target {
        Target::Tab(i) => {
            let tab = &tabs[i];
            let arrow = if tab.collapsed { '▸' } else { '▾' };
            (format!(" {arrow} "), tab.name.to_string())
        }
        Target::Pane(i, id) => {
            let panes = &tabs[i].panes;
            let last = panes.last().map(|p| p.id) == Some(id);
            let title = panes.iter().find(|p| p.id == id).map_or("", |p| p.title);
            (
                format!("   {} ", if last { '└' } else { '├' }),
                title.to_string(),
            )
        }
    };
    let label = truncate(&label, width.saturating_sub(prefix.width()));
    format!("{prefix}{label}")
}

/// Mode hint shown on the sidebar's last line.
pub struct Status<'a> {
    pub text: &'a str,
    pub highlight: bool,
}

pub struct View<'a> {
    pub tabs: &'a [TabEntry<'a>],
    /// Selected row while the sidebar has focus.
    pub selected: Option<usize>,
    /// Row to keep in view when the list is taller than the sidebar.
    pub anchor: usize,
    /// Tab being renamed and the text typed so far.
    pub rename: Option<(usize, &'a str)>,
    pub status: Status<'a>,
}

/// Height available for rows (the last line is the status line).
pub fn list_height(area: Rect) -> usize {
    area.height.saturating_sub(1) as usize
}

/// Draw the sidebar. Returns where the cursor should go while renaming.
pub fn render(view: &View, area: Rect, buf: &mut Buffer) -> Option<Position> {
    if area.width < 2 || area.height == 0 {
        return None;
    }
    let inner = Rect {
        width: area.width - 1,
        ..area
    };
    let text_width = inner.width as usize;
    let border = Style::new().fg(Color::DarkGray);
    for y in area.top()..area.bottom() {
        buf[(area.right() - 1, y)].set_symbol("│").set_style(border);
    }

    let rows = rows(view.tabs);
    let height = list_height(area);
    let offset = scroll_offset(view.anchor, height);
    let mut cursor = None;

    for (n, &target) in rows.iter().enumerate().skip(offset).take(height) {
        let y = inner.y + (n - offset) as u16;
        let mut style = match target {
            Target::Tab(i) if view.tabs[i].active => Style::new()
                .bg(Color::Indexed(237))
                .add_modifier(Modifier::BOLD),
            Target::Tab(_) => Style::new(),
            Target::Pane(i, id) => {
                let tab = &view.tabs[i];
                let focused = tab.panes.iter().any(|p| p.id == id && p.focused);
                if tab.active && focused {
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray)
                }
            }
        };
        if view.selected == Some(n) {
            style = Style::new()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD);
        }

        let text = match (target, view.rename) {
            (Target::Tab(i), Some((r, typed))) if i == r => {
                // Show the end of the text being typed, so the cursor stays visible.
                let room = text_width.saturating_sub(4);
                let shown = tail(typed, room);
                cursor = Some(Position::new(inner.x + 3 + shown.width() as u16, y));
                style = Style::new()
                    .bg(Color::Indexed(237))
                    .add_modifier(Modifier::UNDERLINED);
                format!(" ✎ {shown}")
            }
            _ => row_text(view.tabs, target, text_width),
        };
        let line = Rect::new(inner.x, y, inner.width, 1);
        buf.set_style(line, style);
        buf.set_stringn(inner.x, y, &text, text_width, style);
    }

    let status_style = if view.status.highlight {
        Style::new()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let y = inner.bottom() - 1;
    buf.set_style(Rect::new(inner.x, y, inner.width, 1), status_style);
    buf.set_stringn(
        inner.x,
        y,
        truncate(view.status.text, text_width),
        text_width,
        status_style,
    );
    cursor
}

/// The last `width` columns of `s`.
fn tail(s: &str, width: usize) -> &str {
    let mut used = 0;
    for (i, ch) in s.char_indices().rev() {
        used += ch.width().unwrap_or(0);
        if used > width {
            return &s[i + ch.len_utf8()..];
        }
    }
    s
}

/// Map a click at screen row `y` to a row index, given the drawn area and
/// the anchor row that `render` scrolled to.
pub fn row_at(area: Rect, y: u16, anchor: usize, row_count: usize) -> Option<usize> {
    let height = list_height(area);
    if y < area.y || (y - area.y) as usize >= height {
        return None;
    }
    let n = scroll_offset(anchor, height) + (y - area.y) as usize;
    (n < row_count).then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab<'a>(name: &'a str, collapsed: bool, panes: &[PaneId]) -> TabEntry<'a> {
        TabEntry {
            name,
            collapsed,
            active: false,
            panes: panes
                .iter()
                .map(|&id| PaneEntry {
                    id,
                    title: "sh",
                    focused: false,
                })
                .collect(),
        }
    }

    #[test]
    fn rows_flatten_tree_and_skip_collapsed() {
        let tabs = vec![
            tab("a", false, &[1, 2]),
            tab("b", true, &[3]),
            tab("c", false, &[4]),
        ];
        assert_eq!(
            rows(&tabs),
            vec![
                Target::Tab(0),
                Target::Pane(0, 1),
                Target::Pane(0, 2),
                Target::Tab(1),
                Target::Tab(2),
                Target::Pane(2, 4),
            ]
        );
    }

    #[test]
    fn selection_is_clamped() {
        assert_eq!(move_selection(0, -1, 5), 0);
        assert_eq!(move_selection(3, 1, 5), 4);
        assert_eq!(move_selection(4, 1, 5), 4);
        assert_eq!(move_selection(2, -1, 5), 1);
        assert_eq!(move_selection(7, 0, 0), 0);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
        assert_eq!(truncate("a long tab name", 8), "a long …");
        assert_eq!(truncate("abc", 1), "…");
        assert_eq!(truncate("abc", 0), "");
        // Wide chars count as two columns.
        assert_eq!(truncate("日本語テキスト", 7), "日本語…");
    }

    #[test]
    fn row_text_draws_tree() {
        let tabs = vec![tab("work", false, &[1, 2]), tab("x", true, &[3])];
        assert_eq!(row_text(&tabs, Target::Tab(0), 23), " ▾ work");
        assert_eq!(row_text(&tabs, Target::Pane(0, 1), 23), "   ├ sh");
        assert_eq!(row_text(&tabs, Target::Pane(0, 2), 23), "   └ sh");
        assert_eq!(row_text(&tabs, Target::Tab(1), 23), " ▸ x");
        let long = vec![tab("a very long tab name indeed", false, &[])];
        let text = row_text(&long, Target::Tab(0), 12);
        assert_eq!(text, " ▾ a very l…");
        assert_eq!(text.width(), 12);
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        assert_eq!(scroll_offset(3, 10), 0);
        assert_eq!(scroll_offset(12, 10), 3);
        let area = Rect::new(0, 0, 24, 11); // 10 list rows + status line
        assert_eq!(row_at(area, 0, 12, 20), Some(3));
        assert_eq!(row_at(area, 10, 0, 20), None); // status line
        assert_eq!(row_at(area, 2, 0, 2), None); // past the last row
    }

    #[test]
    fn tail_keeps_end_of_text() {
        assert_eq!(tail("hello world", 5), "world");
        assert_eq!(tail("hi", 5), "hi");
    }
}
