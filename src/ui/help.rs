//! The Ctrl+Shift+H popup listing all shortcuts.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::input::HELP;

/// Draw the popup centered on the screen, over everything else.
pub fn render(f: &mut Frame) {
    let key_width = HELP.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(
                    format!(" {key:<key_width$}  "),
                    Style::new().fg(Color::Cyan),
                ),
                Span::raw(format!("{what} ")),
            ])
        })
        .collect();

    // Content plus 1 cell of border on each side.
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 2;
    let height = lines.len() as u16 + 2;
    let area = centered(f.area(), width, height);

    let block = Block::bordered()
        .title(" haunt · any key closes ")
        .border_style(Style::new().fg(Color::Cyan))
        .title_style(Style::new().add_modifier(Modifier::BOLD));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// A `width` x `height` rect in the middle of `area`, shrunk to fit.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_fits_and_centers() {
        assert_eq!(
            centered(Rect::new(0, 0, 100, 30), 40, 10),
            Rect::new(30, 10, 40, 10)
        );
        // Bigger than the screen: clamp to it.
        assert_eq!(
            centered(Rect::new(0, 0, 20, 5), 40, 10),
            Rect::new(0, 0, 20, 5)
        );
    }
}
