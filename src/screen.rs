//! A ratatui backend that renders into a byte buffer instead of a real
//! terminal, so a frame can go to stdout or over a socket to a client.
//!
//! Its size is whatever we set, never queried from a terminal: a background
//! server has no terminal, and asking one for the cursor position would
//! mean reading from stdin.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

pub struct Screen {
    inner: CrosstermBackend<Shared>,
    /// The same buffer `inner` writes to.
    out: Shared,
    size: Size,
    cursor: Position,
}

/// A byte buffer with two owners: the crossterm backend writes into it, and
/// `Screen::take` empties it.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub type Terminal = ratatui::Terminal<Screen>;

impl Screen {
    pub fn new(size: Size) -> Screen {
        let out = Shared::default();
        Screen {
            inner: CrosstermBackend::new(out.clone()),
            out,
            size,
            cursor: Position::ORIGIN,
        }
    }

    /// The next draw notices the change and redraws everything.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    /// Everything written since the last call.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut *self.out.0.borrow_mut())
    }
}

/// Lets extra commands (like the cursor shape) go into the same frame.
impl Write for Screen {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.out.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for Screen {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor = position.into();
        self.inner.set_cursor_position(self.cursor)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::default(),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Paragraph;

    #[test]
    fn renders_into_buffer_and_resizes() {
        let mut t = Terminal::new(Screen::new(Size::new(10, 2))).unwrap();
        t.draw(|f| f.render_widget(Paragraph::new("hi"), f.area()))
            .unwrap();
        let out = String::from_utf8(t.backend_mut().take()).unwrap();
        assert!(out.contains("hi"));
        assert!(t.backend_mut().take().is_empty());

        // A new size is picked up on the next draw.
        t.backend_mut().set_size(Size::new(20, 3));
        let frame = t.draw(|_| {}).unwrap();
        assert_eq!(frame.area.width, 20);
    }
}
