//! An embedded terminal running Claude Code inside a popup.
//!
//! Sessions keep running in the background when the popup is hidden and live until the
//! `claude` process exits or the editor is closed.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};
use helix_view::graphics::{Color, CursorKind, Modifier, Rect, Style, UnderlineStyle};
use helix_view::input::{Event, KeyEvent, MouseEventKind};
use helix_view::keyboard::{KeyCode, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Compositor, Context, EventResult};
use crate::job;

pub const ID: &str = "claude-terminal";

/// All running sessions, in creation order.
static SESSIONS: Mutex<Vec<Arc<Session>>> = Mutex::new(Vec::new());
/// The session that was shown most recently.
static LAST_SESSION: AtomicUsize = AtomicUsize::new(0);
static NEXT_SESSION_ID: AtomicUsize = AtomicUsize::new(1);

pub struct Session {
    pub id: usize,
    /// Where the session was started from, e.g. `src/main.rs:12`.
    pub label: String,
    shared: Arc<Shared>,
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: Mutex<(u16, u16)>,
    /// The last file reference pasted into the prompt, to avoid pasting it twice.
    last_reference: Mutex<Option<String>>,
}

/// State shared with the PTY thread.
#[derive(Default)]
struct Shared {
    sender: OnceLock<EventLoopSender>,
    title: Mutex<String>,
    exited: AtomicBool,
}

#[derive(Clone)]
struct Listener {
    session_id: usize,
    shared: Arc<Shared>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Wakeup => helix_event::request_redraw(),
            TermEvent::PtyWrite(text) => {
                if let Some(sender) = self.shared.sender.get() {
                    let _ = sender.send(Msg::Input(text.into_bytes().into()));
                }
            }
            TermEvent::Title(title) => {
                *self.shared.title.lock().unwrap() = title;
                helix_event::request_redraw();
            }
            TermEvent::ResetTitle => self.shared.title.lock().unwrap().clear(),
            TermEvent::ChildExit(_) | TermEvent::Exit => {
                if self.shared.exited.swap(true, Ordering::SeqCst) {
                    return;
                }
                let id = self.session_id;
                job::dispatch_blocking(move |editor, compositor| {
                    session_exited(editor, compositor, id)
                });
            }
            _ => {}
        }
    }
}

struct TermSize {
    cols: u16,
    rows: u16,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.cols as usize
    }
}

fn window_size(cols: u16, rows: u16) -> WindowSize {
    WindowSize {
        num_lines: rows,
        num_cols: cols,
        cell_width: 1,
        cell_height: 1,
    }
}

impl Session {
    /// Spawns `claude` with `args` in a new PTY sized for a popup over `viewport`.
    pub fn spawn(
        label: String,
        args: Vec<String>,
        cwd: PathBuf,
        reference: Option<String>,
        viewport: Rect,
    ) -> anyhow::Result<Arc<Session>> {
        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::SeqCst);
        let inner = inner_area(popup_area(viewport));
        let (cols, rows) = (inner.width.max(2), inner.height.max(2));

        let options = tty::Options {
            shell: Some(tty::Shell::new("claude".to_string(), args)),
            working_directory: Some(cwd),
            drain_on_exit: false,
            env: HashMap::from([
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
            ]),
        };
        let pty = tty::new(&options, window_size(cols, rows), id as u64)
            .map_err(|err| anyhow::anyhow!("Failed to start claude: {err}"))?;

        let shared = Arc::new(Shared::default());
        let listener = Listener {
            session_id: id,
            shared: shared.clone(),
        };
        let term = Arc::new(FairMutex::new(Term::new(
            Config::default(),
            &TermSize { cols, rows },
            listener.clone(),
        )));
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let sender = event_loop.channel();
        let _ = shared.sender.set(sender.clone());
        event_loop.spawn();

        let session = Arc::new(Session {
            id,
            label,
            shared,
            term,
            sender,
            size: Mutex::new((cols, rows)),
            last_reference: Mutex::new(reference),
        });
        SESSIONS.lock().unwrap().push(session.clone());
        Ok(session)
    }

    pub fn title(&self) -> String {
        self.shared.title.lock().unwrap().clone()
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let _ = self.sender.send(Msg::Input(bytes.into()));
    }

    /// Writes `text` as if it was pasted into the terminal.
    fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.write(text.into_bytes());
    }

    /// Pastes `reference` into the prompt, unless it was the last one pasted.
    pub fn paste_reference(&self, reference: String) {
        let mut last = self.last_reference.lock().unwrap();
        if last.as_ref() != Some(&reference) {
            self.paste(&format!("{reference} "));
            *last = Some(reference);
        }
    }

    fn resize(&self, cols: u16, rows: u16) {
        let mut size = self.size.lock().unwrap();
        if *size == (cols, rows) {
            return;
        }
        *size = (cols, rows);
        self.term.lock().resize(TermSize { cols, rows });
        let _ = self.sender.send(Msg::Resize(window_size(cols, rows)));
    }

    fn scroll(&self, scroll: Scroll) {
        self.term.lock().scroll_display(scroll);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.sender.send(Msg::Shutdown);
    }
}

pub fn sessions() -> Vec<Arc<Session>> {
    SESSIONS.lock().unwrap().clone()
}

/// The most recently shown session that is still running.
pub fn last_session() -> Option<Arc<Session>> {
    let last = LAST_SESSION.load(Ordering::SeqCst);
    let sessions = SESSIONS.lock().unwrap();
    sessions
        .iter()
        .find(|session| session.id == last)
        .or(sessions.last())
        .cloned()
}

/// Shows `session` in the popup, replacing any other session being shown.
pub fn show(compositor: &mut Compositor, session: Arc<Session>) {
    LAST_SESSION.store(session.id, Ordering::SeqCst);
    compositor.replace_or_push(
        ID,
        ClaudeTerminal {
            session,
            cursor: None,
        },
    );
}

fn session_exited(editor: &mut Editor, compositor: &mut Compositor, id: usize) {
    SESSIONS.lock().unwrap().retain(|session| session.id != id);
    if compositor
        .find_id::<ClaudeTerminal>(ID)
        .is_some_and(|popup| popup.session.id == id)
    {
        compositor.remove(ID);
    }
    crate::commands::reload_documents(editor, true);
    editor.set_status(format!("Claude session #{id} ended"));
}

fn popup_area(viewport: Rect) -> Rect {
    let width = (viewport.width * 9 / 10).max(viewport.width.min(20));
    let height = (viewport.height * 9 / 10).max(viewport.height.min(6));
    Rect::new(
        viewport.x + (viewport.width - width) / 2,
        viewport.y + (viewport.height - height) / 2,
        width,
        height,
    )
}

fn inner_area(area: Rect) -> Rect {
    Block::bordered().inner(area)
}

pub struct ClaudeTerminal {
    session: Arc<Session>,
    cursor: Option<(helix_core::Position, CursorKind)>,
}

impl Component for ClaudeTerminal {
    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        match event {
            // Without the kitty keyboard protocol, C-\ (0x1c) is decoded as C-4.
            Event::Key(KeyEvent {
                code: KeyCode::Char('\\' | '4'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                EventResult::Consumed(Some(Box::new(|compositor: &mut Compositor, cx| {
                    compositor.remove(ID);
                    crate::commands::reload_documents(cx.editor, true);
                })))
            }
            Event::Key(key) => {
                let app_cursor = self
                    .session
                    .term
                    .lock()
                    .mode()
                    .contains(TermMode::APP_CURSOR);
                if let Some(bytes) = encode_key(key, app_cursor) {
                    self.session.scroll(Scroll::Bottom);
                    self.session.write(bytes);
                }
                EventResult::Consumed(None)
            }
            Event::Paste(text) => {
                self.session.scroll(Scroll::Bottom);
                self.session.paste(text);
                EventResult::Consumed(None)
            }
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.session.scroll(Scroll::Delta(3)),
                    MouseEventKind::ScrollDown => self.session.scroll(Scroll::Delta(-3)),
                    _ => {}
                }
                EventResult::Consumed(None)
            }
            Event::Resize(..) | Event::IdleTimeout | Event::FocusGained | Event::FocusLost => {
                EventResult::Ignored(None)
            }
        }
    }

    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let area = popup_area(viewport);
        let theme = &cx.editor.theme;
        let background = theme.get("ui.popup");
        surface.clear_with(area, background);

        let title = self.session.title();
        let title = if title.is_empty() {
            format!(" Claude #{} ", self.session.id)
        } else {
            format!(" Claude #{}: {title} ", self.session.id)
        };
        let block = Block::bordered()
            .title(title)
            .border_style(theme.get("ui.popup.info"));
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.width < 2 || inner.height < 2 {
            return;
        }
        let hint = " C-\\ to hide ";
        if inner.width as usize > hint.len() + 2 {
            surface.set_string(
                area.right() - hint.len() as u16 - 2,
                area.bottom() - 1,
                hint,
                theme.get("ui.text.inactive"),
            );
        }

        self.session.resize(inner.width, inner.height);

        let term = self.session.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let row = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            if row < 0 || row >= inner.height as i32 || col >= inner.width as usize {
                continue;
            }
            let Some(target) = surface.get_mut(inner.x + col as u16, inner.y + row as u16) else {
                continue;
            };
            let mut symbol = String::from(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(zerowidth) = cell.zerowidth() {
                symbol.extend(zerowidth);
            }
            target.set_symbol(&symbol);
            target.set_style(cell_style(cell.fg, cell.bg, cell.flags, background));
        }

        let cursor = content.cursor;
        let row = cursor.point.line.0 + offset;
        self.cursor = (term.mode().contains(TermMode::SHOW_CURSOR)
            && cursor.shape != CursorShape::Hidden
            && row >= 0
            && row < inner.height as i32)
            .then(|| {
                let kind = match cursor.shape {
                    CursorShape::Beam => CursorKind::Bar,
                    CursorShape::Underline => CursorKind::Underline,
                    _ => CursorKind::Block,
                };
                (
                    helix_core::Position::new(
                        inner.y as usize + row as usize,
                        inner.x as usize + cursor.point.column.0,
                    ),
                    kind,
                )
            });
    }

    fn cursor(&self, _area: Rect, _editor: &Editor) -> (Option<helix_core::Position>, CursorKind) {
        match self.cursor {
            Some((pos, kind)) => (Some(pos), kind),
            None => (None, CursorKind::Hidden),
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(ID)
    }
}

fn convert_color(color: TermColor) -> (Color, bool) {
    match color {
        TermColor::Spec(rgb) => (Color::Rgb(rgb.r, rgb.g, rgb.b), false),
        TermColor::Indexed(index) => (Color::Indexed(index), false),
        TermColor::Named(named) => match named {
            NamedColor::DimBlack
            | NamedColor::DimRed
            | NamedColor::DimGreen
            | NamedColor::DimYellow
            | NamedColor::DimBlue
            | NamedColor::DimMagenta
            | NamedColor::DimCyan
            | NamedColor::DimWhite => (
                Color::Indexed(named as usize as u8 - NamedColor::DimBlack as usize as u8),
                true,
            ),
            NamedColor::DimForeground => (Color::Reset, true),
            named if (named as usize) < 16 => (Color::Indexed(named as usize as u8), false),
            _ => (Color::Reset, false),
        },
    }
}

fn cell_style(fg: TermColor, bg: TermColor, flags: Flags, background: Style) -> Style {
    let (fg, dim) = convert_color(fg);
    let (bg, _) = convert_color(bg);
    let mut style = Style::default().fg(fg);
    // Default background blends with the popup instead of the terminal's own background.
    style = match bg {
        Color::Reset => style.bg(background.bg.unwrap_or(Color::Reset)),
        bg => style.bg(bg),
    };

    let mut modifier = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if dim || flags.contains(Flags::DIM) {
        modifier |= Modifier::DIM;
    }
    if flags.contains(Flags::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    if flags.contains(Flags::INVERSE) {
        modifier |= Modifier::REVERSED;
    }
    if flags.contains(Flags::HIDDEN) {
        modifier |= Modifier::HIDDEN;
    }
    if flags.contains(Flags::STRIKEOUT) {
        modifier |= Modifier::CROSSED_OUT;
    }
    style = style.add_modifier(modifier);
    if flags.intersects(Flags::ALL_UNDERLINES) {
        style = style.underline_style(UnderlineStyle::Line);
    }
    style
}

/// Encodes a key press as the bytes a terminal would send (xterm legacy encoding).
fn encode_key(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let param = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;

    let csi = |final_byte: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{code};{param}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let with_alt = |bytes: Vec<u8>| -> Vec<u8> {
        if alt {
            let mut prefixed = vec![0x1b];
            prefixed.extend(bytes);
            prefixed
        } else {
            bytes
        }
    };

    let bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let byte = match c {
                'a'..='z' | 'A'..='Z' => c.to_ascii_lowercase() as u8 & 0x1f,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '/' | '7' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return None,
            };
            with_alt(vec![byte])
        }
        KeyCode::Char(c) => with_alt(c.to_string().into_bytes()),
        // Claude Code inserts a newline on ESC + CR.
        KeyCode::Enter if shift || alt => b"\x1b\r".to_vec(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab if shift => b"\x1b[Z".to_vec(),
        KeyCode::Tab => with_alt(b"\t".to_vec()),
        KeyCode::Backspace if ctrl => with_alt(vec![0x08]),
        KeyCode::Backspace => with_alt(vec![0x7f]),
        KeyCode::Esc => with_alt(vec![0x1b]),
        KeyCode::Up => csi('A'),
        KeyCode::Down => csi('B'),
        KeyCode::Right => csi('C'),
        KeyCode::Left => csi('D'),
        KeyCode::Home => csi('H'),
        KeyCode::End => csi('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let final_byte = (b'P' + n - 1) as char;
            if param > 1 {
                format!("\x1b[1;{param}{final_byte}").into_bytes()
            } else {
                format!("\x1bO{final_byte}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        _ => return None,
    };
    Some(bytes)
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    fn encode(key: &str) -> Vec<u8> {
        encode_key(&KeyEvent::from_str(key).unwrap(), false).unwrap()
    }

    #[test]
    fn encodes_keys() {
        assert_eq!(encode("a"), b"a");
        assert_eq!(encode("C-c"), [0x03]);
        assert_eq!(encode("A-b"), b"\x1bb");
        assert_eq!(encode("ret"), b"\r");
        assert_eq!(encode("S-ret"), b"\x1b\r");
        assert_eq!(encode("esc"), [0x1b]);
        assert_eq!(encode("backspace"), [0x7f]);
        assert_eq!(encode("up"), b"\x1b[A");
        assert_eq!(encode("C-left"), b"\x1b[1;5D");
        assert_eq!(encode("del"), b"\x1b[3~");
        assert_eq!(encode("F5"), b"\x1b[15~");
        assert_eq!(
            encode_key(&KeyEvent::from_str("up").unwrap(), true).unwrap(),
            b"\x1bOA"
        );
    }
}
