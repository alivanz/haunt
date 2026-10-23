//! Application state and the main event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
use anyhow::Result;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};

use crate::input::{self, Action};
use crate::layout::{self, Computed, Node, Split};
use crate::pane::{Pane, PaneId, SpawnOptions, TermSize};
use crate::session::{self, SavedPane, SavedTab, Session};
use crate::ui::{self, sidebar};

pub type Terminal = ratatui::Terminal<CrosstermBackend<std::io::Stdout>>;

/// Lines moved per scroll wheel tick.
const SCROLL_LINES: i32 = 3;

/// How often a named session is saved while something changes. A crash
/// loses at most this much.
const SAVE_INTERVAL: Duration = Duration::from_secs(3);

/// A named session opened from the command line (`haunt -s NAME`).
pub struct OpenSession {
    pub name: String,
    /// Held for as long as haunt runs.
    pub lock: session::Lock,
    /// What was saved last time, if anything.
    pub saved: Option<Session>,
}

/// Autosave state for the open session.
struct Saving {
    name: String,
    path: PathBuf,
    _lock: session::Lock,
    /// Something changed since the last save.
    dirty: bool,
    last_save: Instant,
    /// The last save failed; shown in the sidebar.
    failed: bool,
}

/// Everything the main loop reacts to. Input and PTY reader threads send
/// these over one channel so the loop can block on a single receiver.
pub enum AppEvent {
    Input(Event),
    /// Some pane produced output; redraw.
    PaneOutput,
    PaneExited(PaneId),
    /// A program set (or reset) its pane's title.
    Title(PaneId, Option<String>),
}

pub struct Tab {
    pub name: String,
    pub layout: Node,
    pub focused: PaneId,
    /// Collapsed in the sidebar tree.
    pub collapsed: bool,
}

/// Where keyboard input goes.
enum Mode {
    /// Keys go to the focused pane.
    Normal,
    /// The sidebar has focus; `sidebar_sel` is the selected row.
    Sidebar,
    /// Renaming the active tab; holds the text typed so far.
    Rename(String),
    /// The help popup is open; any key closes it.
    Help,
}

pub struct App {
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_pane_id: PaneId,
    tabs_created: usize,
    mode: Mode,
    sidebar_sel: usize,
    /// Screen geometry from the last draw, used to size new panes and to
    /// hit-test mouse clicks.
    sidebar_area: Rect,
    /// Sidebar toggled off with `Ctrl+Shift+B`.
    sidebar_hidden: bool,
    main_area: Rect,
    computed: Computed,
    /// Pane that received the last mouse press; drags and the release go to
    /// it even if the pointer leaves its area (like a real terminal).
    mouse_grab: Option<PaneId>,
    /// Cursor style last sent to the outer terminal.
    cursor_style: Option<CursorStyle>,
    saving: Option<Saving>,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    quit: bool,
}

impl App {
    pub fn new(screen: Rect, open: Option<OpenSession>) -> Result<App> {
        let (tx, rx) = channel();
        let (sidebar_area, main_area) = ui::screen_areas(screen, true);
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            panes: HashMap::new(),
            next_pane_id: 0,
            tabs_created: 0,
            mode: Mode::Normal,
            sidebar_sel: 0,
            sidebar_area,
            sidebar_hidden: false,
            main_area,
            computed: Computed::default(),
            mouse_grab: None,
            cursor_style: None,
            saving: None,
            tx,
            rx,
            quit: false,
        };
        if let Some(open) = open {
            if let Some(saved) = open.saved {
                app.restore(saved);
            }
            app.saving = Some(Saving {
                path: session::path(&open.name),
                name: open.name,
                _lock: open.lock,
                dirty: false,
                last_save: Instant::now(),
                failed: false,
            });
        }
        if app.tabs.is_empty() {
            app.new_tab()?;
        }
        Ok(app)
    }

    pub fn run(mut self, terminal: &mut Terminal) -> Result<()> {
        spawn_input_thread(self.tx.clone());
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            self.sync_cursor_style(terminal);

            // Wait for one event, then drain whatever else is queued so a
            // burst of output causes one redraw, not hundreds. The timeout
            // lets a pending autosave happen even when nothing else does.
            match self.rx.recv_timeout(SAVE_INTERVAL) {
                Ok(ev) => {
                    self.handle(ev);
                    while let Ok(ev) = self.rx.try_recv() {
                        self.handle(ev);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.autosave();
        }
        self.finish_session();
        Ok(())
    }

    // ---- sessions -----------------------------------------------------

    /// Recreate the tabs of a saved session: same layout, names and pane
    /// text, with a fresh shell in each pane's old directory.
    fn restore(&mut self, saved: Session) {
        for tab in saved.tabs {
            let mut panes: HashMap<PaneId, SavedPane> =
                tab.panes.into_iter().map(|p| (p.id, p)).collect();
            let mut layout = Some(tab.layout.clone());
            for (id, rect) in tab.layout.compute(self.main_area).panes {
                let saved = panes.remove(&id);
                let (cwd, title, history) = match saved {
                    Some(p) => (p.cwd, p.title, Some(p.history)),
                    None => (None, None, None),
                };
                let size = TermSize {
                    cols: rect.width,
                    rows: rect.height,
                };
                match Pane::spawn(id, size, self.tx.clone(), SpawnOptions { cwd, history }) {
                    Ok(mut pane) => {
                        pane.title = title;
                        self.panes.insert(id, pane);
                    }
                    Err(_) => layout = layout.and_then(|l| l.remove(id)),
                }
                self.next_pane_id = self.next_pane_id.max(id + 1);
            }
            let Some(layout) = layout else { continue };
            let ids = layout.panes();
            let focused = if ids.contains(&tab.focused) {
                tab.focused
            } else {
                ids[0]
            };
            self.tabs.push(Tab {
                name: tab.name,
                layout,
                focused,
                collapsed: tab.collapsed,
            });
        }
        self.tabs_created = self.tabs.len();
        self.active = saved.active.min(self.tabs.len().saturating_sub(1));
    }

    fn snapshot_session(&self) -> Session {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| SavedTab {
                name: tab.name.clone(),
                collapsed: tab.collapsed,
                focused: tab.focused,
                layout: tab.layout.clone(),
                panes: tab
                    .layout
                    .panes()
                    .into_iter()
                    .filter_map(|id| {
                        let pane = self.panes.get(&id)?;
                        Some(SavedPane {
                            id,
                            cwd: pane.cwd(),
                            title: pane.title.clone(),
                            history: pane.history(),
                        })
                    })
                    .collect(),
            })
            .collect();
        Session::new(self.active, tabs)
    }

    fn save_session(&mut self) {
        if self.saving.is_none() {
            return;
        }
        let snapshot = self.snapshot_session();
        let Some(saving) = &mut self.saving else {
            return;
        };
        saving.failed = session::save(&saving.path, &snapshot).is_err();
        saving.dirty = false;
        saving.last_save = Instant::now();
    }

    fn autosave(&mut self) {
        if let Some(s) = &self.saving
            && s.dirty
            && s.last_save.elapsed() >= SAVE_INTERVAL
        {
            self.save_session();
        }
    }

    /// On exit: quitting saves the session so it can be resumed; if every
    /// shell exited instead, the session is over and its file is removed.
    fn finish_session(&mut self) {
        if self.tabs.is_empty() {
            if let Some(s) = &self.saving {
                session::delete(&s.path);
            }
        } else {
            self.save_session();
        }
    }

    // ---- drawing ------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        let (sidebar_area, main_area) = ui::screen_areas(f.area(), !self.sidebar_hidden);
        self.sidebar_area = sidebar_area;
        self.main_area = main_area;

        let tab = &self.tabs[self.active];
        self.computed = tab.layout.compute(main_area);
        let mut pane_cursor = None;
        for &(id, rect) in &self.computed.panes {
            let Some(pane) = self.panes.get_mut(&id) else {
                continue;
            };
            // Keeps the PTY and emulator in sync with the on-screen size.
            pane.resize(TermSize {
                cols: rect.width,
                rows: rect.height,
            });
            let term = pane.term.lock().unwrap();
            let cursor = ui::pane_view::render(&term, rect, f.buffer_mut());
            if id == tab.focused {
                pane_cursor = cursor;
            }
        }
        let focused_rect = self.computed.rect_of(tab.focused);
        let multiple = self.computed.panes.len() > 1;
        ui::render_dividers(
            &self.computed.dividers,
            focused_rect.filter(|_| multiple),
            f.buffer_mut(),
        );

        let rename_cursor = self.draw_sidebar(f);
        let cursor = match self.mode {
            Mode::Normal => pane_cursor,
            Mode::Rename(_) => rename_cursor,
            Mode::Sidebar | Mode::Help => None,
        };
        if matches!(self.mode, Mode::Help) {
            ui::help::render(f);
        }
        if let Some(pos) = cursor {
            f.set_cursor_position(pos);
        }
    }

    fn draw_sidebar(&self, f: &mut Frame) -> Option<Position> {
        let entries = self.sidebar_entries();
        let rows = sidebar::rows(&entries);
        let (status, highlight) = match (&self.mode, &self.saving) {
            (Mode::Sidebar, _) => (" SIDEBAR ⏎ go ␣ fold".to_string(), true),
            (Mode::Rename(_), _) => (" RENAME ⏎ ok esc".to_string(), true),
            (_, Some(s)) if s.failed => (" ⚠ saving session failed".to_string(), true),
            (_, Some(s)) => (format!(" [{}] ctrl+shift+h: help", s.name), false),
            (_, None) => (" ctrl+shift+h: help".to_string(), false),
        };
        let view = sidebar::View {
            tabs: &entries,
            selected: matches!(self.mode, Mode::Sidebar).then_some(self.sidebar_sel),
            anchor: self.sidebar_anchor(&rows),
            rename: match &self.mode {
                Mode::Rename(text) => Some((self.active, text.as_str())),
                _ => None,
            },
            status: sidebar::Status {
                text: &status,
                highlight,
            },
        };
        sidebar::render(&view, self.sidebar_area, f.buffer_mut())
    }

    fn sidebar_entries(&self) -> Vec<sidebar::TabEntry<'_>> {
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| sidebar::TabEntry {
                name: &tab.name,
                collapsed: tab.collapsed,
                active: i == self.active,
                panes: tab
                    .layout
                    .panes()
                    .into_iter()
                    .map(|id| sidebar::PaneEntry {
                        id,
                        title: self
                            .panes
                            .get(&id)
                            .and_then(|p| p.title.as_deref())
                            .unwrap_or("shell"),
                        focused: id == tab.focused,
                    })
                    .collect(),
            })
            .collect()
    }

    /// The row the sidebar keeps scrolled into view: the selection while the
    /// sidebar has focus, otherwise the active tab's focused pane.
    fn sidebar_anchor(&self, rows: &[sidebar::Target]) -> usize {
        if matches!(self.mode, Mode::Sidebar) {
            return self.sidebar_sel;
        }
        let focused = self.tabs[self.active].focused;
        rows.iter()
            .position(|&t| t == sidebar::Target::Pane(self.active, focused))
            .or_else(|| {
                rows.iter()
                    .position(|&t| t == sidebar::Target::Tab(self.active))
            })
            .unwrap_or(0)
    }

    /// Mirror the focused program's cursor shape (e.g. vim's insert-mode
    /// beam) on the outer terminal.
    fn sync_cursor_style(&mut self, terminal: &mut Terminal) {
        let style = self
            .focused_pane()
            .map(|p| p.term.lock().unwrap().cursor_style());
        if style == self.cursor_style {
            return;
        }
        self.cursor_style = style;
        let Some(style) = style else { return };
        let cmd = match (style.shape, style.blinking) {
            (CursorShape::Underline, true) => SetCursorStyle::BlinkingUnderScore,
            (CursorShape::Underline, false) => SetCursorStyle::SteadyUnderScore,
            (CursorShape::Beam, true) => SetCursorStyle::BlinkingBar,
            (CursorShape::Beam, false) => SetCursorStyle::SteadyBar,
            (_, true) => SetCursorStyle::BlinkingBlock,
            (_, false) => SetCursorStyle::SteadyBlock,
        };
        let _ = execute!(terminal.backend_mut(), cmd);
    }

    // ---- events -------------------------------------------------------

    fn handle(&mut self, ev: AppEvent) {
        // Anything but pointer movement may change what's worth saving.
        let moved =
            matches!(&ev, AppEvent::Input(Event::Mouse(m)) if m.kind == MouseEventKind::Moved);
        if let Some(s) = &mut self.saving
            && !moved
        {
            s.dirty = true;
        }
        match ev {
            AppEvent::Input(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                self.on_key(key)
            }
            AppEvent::Input(Event::Paste(text)) => self.on_paste(&text),
            AppEvent::Input(Event::Mouse(m)) => self.on_mouse(m),
            AppEvent::Input(_) | AppEvent::PaneOutput => {}
            AppEvent::PaneExited(id) => self.close_pane(id),
            AppEvent::Title(id, title) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.title = title;
                }
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if matches!(self.mode, Mode::Help) {
            // Swallow the key: it was meant to dismiss the popup.
            self.mode = Mode::Normal;
            return;
        }
        // Shortcuts work everywhere except while typing a tab name.
        if !matches!(self.mode, Mode::Rename(_))
            && let Some(action) = input::shortcut(&key)
        {
            self.run_action(action);
            return;
        }
        match &mut self.mode {
            Mode::Normal => {
                if let Some(pane) = self.focused_pane() {
                    let mode = *pane.term.lock().unwrap().mode();
                    if let Some(bytes) = input::encode_key(&key, mode) {
                        // Typing jumps back to the live screen.
                        pane.scroll(Scroll::Bottom);
                        pane.write(&bytes);
                    }
                }
            }
            Mode::Rename(text) => match key.code {
                KeyCode::Enter => {
                    let name = text.trim().to_string();
                    if !name.is_empty() {
                        self.tabs[self.active].name = name;
                    }
                    self.mode = Mode::Normal;
                }
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    text.push(c)
                }
                _ => {}
            },
            Mode::Sidebar => self.on_sidebar_key(key),
            Mode::Help => {}
        }
    }

    fn on_sidebar_key(&mut self, key: KeyEvent) {
        let rows = sidebar::rows(&self.sidebar_entries());
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.sidebar_sel = sidebar::move_selection(self.sidebar_sel, 1, rows.len());
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sidebar_sel = sidebar::move_selection(self.sidebar_sel, -1, rows.len());
            }
            KeyCode::Enter => {
                if let Some(&target) = rows.get(self.sidebar_sel) {
                    self.activate(target);
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Char(' ') => {
                if let Some(&target) = rows.get(self.sidebar_sel) {
                    self.toggle_collapsed(target);
                }
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => self.mode = Mode::Normal,
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let pressed = matches!(m.kind, MouseEventKind::Down(_));
        if matches!(self.mode, Mode::Help) {
            if pressed {
                self.mode = Mode::Normal;
            }
            return;
        }

        if self.mouse_grab.is_none() && layout::contains(self.sidebar_area, x, y) {
            if m.kind == MouseEventKind::Down(MouseButton::Left) {
                self.on_sidebar_click(x, y);
            }
            return;
        }

        // Drags/releases follow the pane that got the press; everything else
        // goes to the pane under the pointer.
        let target = match (m.kind, self.mouse_grab) {
            (MouseEventKind::Drag(_) | MouseEventKind::Up(_), Some(id)) => {
                self.computed.rect_of(id).map(|r| (id, r))
            }
            _ => self.computed.pane_at(x, y),
        };
        if matches!(m.kind, MouseEventKind::Up(_)) {
            self.mouse_grab = None;
        }
        let Some((id, rect)) = target else { return };
        if pressed {
            self.mouse_grab = Some(id);
            if matches!(self.mode, Mode::Sidebar | Mode::Rename(_)) {
                self.mode = Mode::Normal;
            }
            self.tabs[self.active].focused = id;
        }

        let Some(pane) = self.panes.get(&id) else {
            return;
        };
        let mode = *pane.term.lock().unwrap().mode();
        // Pane-relative coordinates, clamped for drags outside the pane.
        let col = x.clamp(rect.x, rect.right().saturating_sub(1)) - rect.x;
        let row = y.clamp(rect.y, rect.bottom().saturating_sub(1)) - rect.y;

        if let Some(bytes) = input::encode_mouse(m.kind, m.modifiers, col, row, mode) {
            // The program asked for mouse events (vim with mouse=a, htop, lazygit...).
            pane.write(&bytes);
            return;
        }
        let up = match m.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen programs without mouse support (less, man) have
            // no scrollback; send arrow keys instead, as most terminals do.
            let arrow = match (up, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            pane.write(arrow.repeat(SCROLL_LINES as usize).as_bytes());
        } else if !mode.contains(TermMode::ALT_SCREEN) {
            pane.scroll(Scroll::Delta(if up { SCROLL_LINES } else { -SCROLL_LINES }));
        }
    }

    fn on_sidebar_click(&mut self, x: u16, y: u16) {
        let rows = sidebar::rows(&self.sidebar_entries());
        let anchor = self.sidebar_anchor(&rows);
        let Some(n) = sidebar::row_at(self.sidebar_area, y, anchor, rows.len()) else {
            return;
        };
        let target = rows[n];
        // Clicking the ▸/▾ arrow folds; clicking anywhere else switches.
        let on_arrow = matches!(target, sidebar::Target::Tab(_)) && x <= self.sidebar_area.x + 2;
        if on_arrow {
            self.toggle_collapsed(target);
        } else {
            self.activate(target);
            self.mode = Mode::Normal;
        }
    }

    fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Rename(buf) => buf.push_str(text),
            _ => {
                let Some(pane) = self.focused_pane() else {
                    return;
                };
                let bracketed = pane
                    .term
                    .lock()
                    .unwrap()
                    .mode()
                    .contains(TermMode::BRACKETED_PASTE);
                if bracketed {
                    pane.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
                } else {
                    pane.write(text.as_bytes());
                }
            }
        }
    }

    fn run_action(&mut self, action: Action) {
        if matches!(self.mode, Mode::Sidebar) {
            // Any command leaves the sidebar; Ctrl+Shift+S there toggles it off.
            self.mode = Mode::Normal;
            if action == Action::FocusSidebar {
                return;
            }
        }
        let result = match action {
            Action::NewTab => self.new_tab(),
            Action::ClosePane => {
                self.close_pane(self.tabs[self.active].focused);
                Ok(())
            }
            Action::SplitVertical => self.split(Split::Vertical),
            Action::SplitHorizontal => self.split(Split::Horizontal),
            Action::Help => {
                self.mode = Mode::Help;
                Ok(())
            }
            Action::RenameTab => {
                // Renaming happens in the sidebar, so make sure it's visible.
                self.sidebar_hidden = false;
                self.mode = Mode::Rename(self.tabs[self.active].name.clone());
                Ok(())
            }
            Action::FocusSidebar => {
                self.sidebar_hidden = false;
                let rows = sidebar::rows(&self.sidebar_entries());
                self.sidebar_sel = self.sidebar_anchor(&rows);
                self.mode = Mode::Sidebar;
                Ok(())
            }
            Action::ToggleSidebar => {
                self.sidebar_hidden = !self.sidebar_hidden;
                Ok(())
            }
            Action::PrevTab | Action::NextTab => {
                let delta = if action == Action::NextTab { 1 } else { -1 };
                self.active = cycle(self.active, delta, self.tabs.len());
                Ok(())
            }
            Action::PrevPane | Action::NextPane => {
                let delta = if action == Action::NextPane { 1 } else { -1 };
                let tab = &mut self.tabs[self.active];
                let panes = tab.layout.panes();
                let current = panes.iter().position(|&p| p == tab.focused).unwrap_or(0);
                tab.focused = panes[cycle(current, delta, panes.len())];
                Ok(())
            }
            Action::Quit => {
                self.quit = true;
                Ok(())
            }
        };
        // There is no message area; a failed spawn simply does nothing.
        let _ = result;
    }

    // ---- tabs & panes -------------------------------------------------

    fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.tabs.get(self.active)?.focused)
    }

    fn spawn_pane(&mut self, rect: Rect) -> Result<PaneId> {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let size = TermSize {
            cols: rect.width,
            rows: rect.height,
        };
        let pane = Pane::spawn(id, size, self.tx.clone(), SpawnOptions::default())?;
        self.panes.insert(id, pane);
        Ok(id)
    }

    fn new_tab(&mut self) -> Result<()> {
        let id = self.spawn_pane(self.main_area)?;
        self.tabs_created += 1;
        self.tabs.push(Tab {
            name: format!("tab {}", self.tabs_created),
            layout: Node::Pane(id),
            focused: id,
            collapsed: false,
        });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Split the focused pane; the new pane goes right/below and gets focus.
    fn split(&mut self, split: Split) -> Result<()> {
        let tab = &mut self.tabs[self.active];
        let target = tab.focused;
        // Insert the new id first so we know the rect it will occupy, and
        // can spawn its PTY at the right size from the start.
        let id = self.next_pane_id;
        tab.layout.split(target, split, id);
        let rect = tab
            .layout
            .compute(self.main_area)
            .rect_of(id)
            .unwrap_or_default();
        match self.spawn_pane(rect) {
            Ok(_) => {
                self.tabs[self.active].focused = id;
                Ok(())
            }
            Err(e) => {
                let tab = &mut self.tabs[self.active];
                let layout = std::mem::replace(&mut tab.layout, Node::Pane(target));
                tab.layout = layout.remove(id).unwrap_or(Node::Pane(target));
                Err(e)
            }
        }
    }

    /// Remove a pane from its tab (killing its process). Closes the tab if
    /// it was the last pane, and quits if it was the last tab.
    fn close_pane(&mut self, id: PaneId) {
        // Also called when a pane's process exits; it may already be gone.
        let Some(pane) = self.panes.remove(&id) else {
            return;
        };
        drop(pane);
        let Some(ti) = self
            .tabs
            .iter()
            .position(|t| t.layout.panes().contains(&id))
        else {
            return;
        };

        let tab = &mut self.tabs[ti];
        let order = tab.layout.panes();
        let layout = std::mem::replace(&mut tab.layout, Node::Pane(id));
        match layout.remove(id) {
            Some(rest) => {
                if tab.focused == id {
                    // Focus the pane that came before it in tree order.
                    let idx = order.iter().position(|&p| p == id).unwrap_or(0);
                    let remaining = rest.panes();
                    tab.focused = remaining[idx.saturating_sub(1).min(remaining.len() - 1)];
                }
                tab.layout = rest;
            }
            None => {
                self.tabs.remove(ti);
                if self.tabs.is_empty() {
                    self.quit = true;
                    return;
                }
                if ti < self.active || self.active >= self.tabs.len() {
                    self.active -= 1;
                }
            }
        }
        let rows = sidebar::rows(&self.sidebar_entries());
        self.sidebar_sel = self.sidebar_sel.min(rows.len().saturating_sub(1));
    }

    /// Switch to what a sidebar row points at.
    fn activate(&mut self, target: sidebar::Target) {
        match target {
            sidebar::Target::Tab(i) => self.active = i,
            sidebar::Target::Pane(i, id) => {
                self.active = i;
                self.tabs[i].focused = id;
            }
        }
    }

    /// Fold/unfold the tab a sidebar row belongs to. On a pane row this
    /// folds its tab and moves the selection up to the tab row.
    fn toggle_collapsed(&mut self, target: sidebar::Target) {
        let i = match target {
            sidebar::Target::Tab(i) | sidebar::Target::Pane(i, _) => i,
        };
        self.tabs[i].collapsed = !self.tabs[i].collapsed;
        let rows = sidebar::rows(&self.sidebar_entries());
        if matches!(target, sidebar::Target::Pane(..))
            && let Some(row) = rows.iter().position(|&t| t == sidebar::Target::Tab(i))
        {
            self.sidebar_sel = row;
        }
        self.sidebar_sel = self.sidebar_sel.min(rows.len().saturating_sub(1));
    }
}

fn spawn_input_thread(tx: Sender<AppEvent>) {
    thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if tx.send(AppEvent::Input(ev)).is_err() {
                break;
            }
        }
    });
}

/// Step `index` by `delta` within `0..len`, wrapping at both ends.
fn cycle(index: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (index as i64 + delta as i64).rem_euclid(len as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::cycle;

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(0, 1, 3), 1);
        assert_eq!(cycle(2, 1, 3), 0);
        assert_eq!(cycle(0, -1, 3), 2);
        assert_eq!(cycle(0, 1, 1), 0);
        assert_eq!(cycle(0, -1, 0), 0);
    }
}
