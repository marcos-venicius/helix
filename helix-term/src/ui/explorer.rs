//! A file tree in a side panel on the left of the editor, toggled with `space E`. Files ignored by
//! git are dimmed (or hidden with `editor.explorer.hide-gitignored`), and the panel can create,
//! rename, move (cut and paste) and delete files.
//!
//! The panel lives in [`EditorView`], which shrinks the editor area to make room for it and hands
//! it the keys while it is focused. Keys the panel doesn't use go to the editor keymap only when
//! they lead to the space menu or the command line, and the panel gives the focus back as soon as
//! the editor shows another view or buffer (a picker or `:open` opened a file, ...).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::bail;
use helix_view::document::Mode;
use helix_view::editor::Action;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::{current_ref, doc, DocumentId, Editor, ViewId};
use ignore::WalkBuilder;
use tui::buffer::Buffer as Surface;

use crate::commands;
use crate::compositor;
use crate::job::Callback;
use crate::keymap::{KeyTrie, KeyTrieNode, Keymaps};
use crate::ui::{self, EditorView, PromptEvent};
use crate::{ctrl, key};

mod icons;

const HELP: &str = "a: new (end with / for a directory)  r: rename  x: cut  p: paste  d: delete  \
                    R: refresh  H: show/hide git ignored  q: close  esc: back to the editor";

#[derive(Clone)]
struct Node {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    /// Ignored by git, directly or because a parent directory is.
    ignored: bool,
}

pub struct Explorer {
    root: PathBuf,
    /// The visible tree, flattened in display order.
    nodes: Vec<Node>,
    /// Listed directories, so expanding or collapsing one doesn't read the others again.
    listings: Listings,
    expanded: HashSet<PathBuf>,
    cursor: usize,
    scroll: usize,
    /// Screen area of the panel and of its list of nodes in the last render, for mouse events.
    area: Rect,
    body: Rect,
    focused: bool,
    /// Path marked with `x`, moved by `p`.
    cut: Option<PathBuf>,
    /// `H` flips `editor.explorer.hide-gitignored` for this session.
    flip_hidden: bool,
    /// Whether `nodes` was listed with ignored files hidden.
    hiding: bool,
    /// Path waiting for the `y` that confirms its deletion.
    confirm_delete: Option<PathBuf>,
    /// The view and buffer the editor showed while the panel had the focus. When they change, the
    /// user went back to editing, so the panel gives the focus back.
    focus_target: Option<(ViewId, DocumentId)>,
}

/// What the panel did with a key.
pub enum KeyResult {
    Handled,
    Close,
}

impl Explorer {
    pub fn new(editor: &Editor) -> Self {
        let mut explorer = Self::empty(helix_stdx::env::current_working_dir());
        explorer.sync_focus_target(editor);
        explorer.refresh(editor);
        if let Some(path) = doc!(editor).path().map(Path::to_path_buf) {
            explorer.reveal(&path);
        }
        explorer
    }

    fn empty(root: PathBuf) -> Self {
        Self {
            root,
            nodes: Vec::new(),
            listings: HashMap::new(),
            expanded: HashSet::new(),
            cursor: 0,
            scroll: 0,
            area: Rect::default(),
            body: Rect::default(),
            focused: true,
            cut: None,
            flip_hidden: false,
            hiding: false,
            confirm_delete: None,
            focus_target: None,
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus(&mut self, editor: &Editor) {
        self.focused = true;
        self.sync_focus_target(editor);
        // Files may have changed on disk while the focus was elsewhere.
        self.refresh(editor);
    }

    pub fn unfocus(&mut self) {
        self.focused = false;
        self.confirm_delete = None;
    }

    /// Width of the panel, leaving at least half of `available` to the editor.
    pub fn width(&self, editor: &Editor, available: u16) -> u16 {
        editor.config().explorer.width.min(available / 2)
    }

    fn sync_focus_target(&mut self, editor: &Editor) {
        let (view, doc) = current_ref!(editor);
        self.focus_target = Some((view.id, doc.id()));
    }

    /// Gives the focus back to the editor when it shows another view or buffer than when the panel
    /// last handled an event.
    fn check_focus_target(&mut self, editor: &Editor) {
        let (view, doc) = current_ref!(editor);
        if self.focused && self.focus_target != Some((view.id, doc.id())) {
            self.unfocus();
        }
    }

    fn hide_ignored(&self, editor: &Editor) -> bool {
        editor.config().explorer.hide_gitignored != self.flip_hidden
    }

    fn selected(&self) -> Option<&Node> {
        self.nodes.get(self.cursor)
    }

    /// Updates the tree from disk, keeping the expanded directories and the selection. Only the
    /// directories that changed since they were listed are read again.
    pub fn refresh(&mut self, editor: &Editor) {
        let cwd = helix_stdx::env::current_working_dir();
        if cwd != self.root {
            self.root = cwd;
            self.expanded.clear();
            self.listings.clear();
            self.cursor = 0;
            self.scroll = 0;
        }
        drop_changed_listings(&mut self.listings);
        self.hiding = self.hide_ignored(editor);
        self.rebuild();
    }

    /// Reads every directory again, for the changes `refresh` can't see (global git excludes,
    /// coarse modification times).
    fn reload(&mut self, editor: &Editor) {
        self.listings.clear();
        self.refresh(editor);
    }

    fn rebuild(&mut self) {
        let selected = self.selected().map(|node| node.path.clone());
        self.expanded.retain(|dir| dir.is_dir());
        self.nodes = build_tree(&self.root, &self.expanded, self.hiding, &mut self.listings);
        self.cursor = self.cursor.min(self.nodes.len().saturating_sub(1));
        if let Some(path) = selected {
            self.select(&path);
        }
    }

    fn select(&mut self, path: &Path) {
        if let Some(index) = self.nodes.iter().position(|node| node.path == path) {
            self.cursor = index;
        }
    }

    /// Expands the parent directories of `path` and selects it.
    pub fn reveal(&mut self, path: &Path) {
        let Some(parent) = path.parent() else {
            return;
        };
        let Ok(relative) = parent.strip_prefix(&self.root) else {
            return;
        };
        let mut dir = self.root.clone();
        let mut changed = false;
        for component in relative.components() {
            dir.push(component);
            changed |= self.expanded.insert(dir.clone());
        }
        if changed {
            self.rebuild();
        }
        self.select(path);
    }

    /// Directory that new and pasted files go to: the selected directory, or the parent of the
    /// selected file.
    fn target_dir(&self) -> PathBuf {
        match self.selected() {
            Some(node) if node.is_dir => node.path.clone(),
            Some(node) => node.path.parent().unwrap_or(&self.root).to_path_buf(),
            None => self.root.clone(),
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.nodes.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Opens the selected file, or expands/collapses the selected directory.
    fn activate(&mut self, editor: &mut Editor) {
        let Some(node) = self.selected() else {
            return;
        };
        let path = node.path.clone();
        if node.is_dir {
            if !self.expanded.remove(&path) {
                self.expanded.insert(path);
            }
            self.rebuild();
        } else {
            match editor.open(&path, Action::Replace) {
                Ok(_) => self.unfocus(),
                Err(err) => editor.set_error(format!("Failed to open {}: {err}", path.display())),
            }
        }
    }

    fn expand(&mut self, editor: &mut Editor) {
        match self.selected() {
            Some(node) if node.is_dir && self.expanded.contains(&node.path) => self.move_cursor(1),
            Some(_) => self.activate(editor),
            None => {}
        }
    }

    /// Collapses the selected directory, or goes to the parent directory.
    fn collapse(&mut self) {
        let Some(node) = self.selected() else {
            return;
        };
        let path = node.path.clone();
        if node.is_dir && self.expanded.remove(&path) {
            self.rebuild();
        } else if let Some(parent) = path.parent() {
            self.select(parent);
        }
    }

    /// Handles a key when the panel is focused. Returns `None` when the key is for the editor: the
    /// panel isn't focused, or the key leads to the space menu or the command line, so `space E`,
    /// the pickers and commands keep working from the panel. While a deletion waits for its
    /// confirmation the panel takes every key, so that only the very next one can confirm it.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        mode: Mode,
        keymaps: &Keymaps,
        cx: &mut commands::Context,
    ) -> Option<KeyResult> {
        if !self.focused {
            return None;
        }
        let result = self.handle_focused_key(key, mode, keymaps, cx);
        if matches!(result, Some(KeyResult::Handled)) {
            // The panel's own actions may change the buffer behind it (deleting closes it).
            self.sync_focus_target(cx.editor);
        }
        result
    }

    fn handle_focused_key(
        &mut self,
        key: KeyEvent,
        mode: Mode,
        keymaps: &Keymaps,
        cx: &mut commands::Context,
    ) -> Option<KeyResult> {
        if let Some(path) = self.confirm_delete.take() {
            if key == key!('y') {
                if let Err(err) = self.delete(&path, cx.editor) {
                    cx.editor.set_error(format!("Failed to delete: {err}"));
                }
            } else {
                cx.editor.set_status("Delete cancelled");
            }
            return Some(KeyResult::Handled);
        }

        let half_page = (self.body.height as isize / 2).max(1);
        match key {
            key!('j') | key!(Down) => self.move_cursor(1),
            key!('k') | key!(Up) => self.move_cursor(-1),
            ctrl!('d') | key!(PageDown) => self.move_cursor(half_page),
            ctrl!('u') | key!(PageUp) => self.move_cursor(-half_page),
            key!('g') | key!(Home) => self.cursor = 0,
            key!('G') | key!(End) => self.move_cursor(isize::MAX),
            key!('l') | key!(Right) => self.expand(cx.editor),
            key!('h') | key!(Left) => self.collapse(),
            key!(Enter) => self.activate(cx.editor),
            key!('a') => self.prompt_create(cx),
            key!('r') => self.prompt_rename(cx),
            key!('x') => {
                if let Some(node) = self.selected() {
                    cx.editor.set_status(format!(
                        "Cut {}, press p on the destination to move it",
                        node.name
                    ));
                    self.cut = Some(node.path.clone());
                }
            }
            key!('p') => {
                if let Err(err) = self.paste(cx.editor) {
                    cx.editor.set_error(format!("Failed to move: {err}"));
                }
            }
            key!('d') => {
                if let Some(node) = self.selected() {
                    cx.editor
                        .set_status(format!("Delete {}? (y/n)", self.relative(&node.path)));
                    self.confirm_delete = Some(node.path.clone());
                }
            }
            key!('R') => {
                self.reload(cx.editor);
                cx.editor.set_status("Explorer refreshed");
            }
            key!('H') => {
                self.flip_hidden = !self.flip_hidden;
                self.refresh(cx.editor);
            }
            key!('?') => cx.editor.set_status(HELP),
            key!(Esc) => self.unfocus(),
            key!('q') => return Some(KeyResult::Close),
            _ if mode != Mode::Insert && leads_to_keymap(keymaps, mode, key) => return None,
            _ => {}
        }
        Some(KeyResult::Handled)
    }

    fn relative<'a>(&self, path: &'a Path) -> std::borrow::Cow<'a, str> {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
    }

    fn prompt_create(&self, cx: &mut commands::Context) {
        let dir = self.target_dir();
        let shown = match self.relative(&dir) {
            relative if relative.is_empty() => String::new(),
            relative => format!("{relative}/"),
        };
        ui::prompt(
            cx,
            format!("new: {shown}").into(),
            None,
            ui::completers::none,
            move |cx, input, event| {
                if event != PromptEvent::Validate || input.trim().is_empty() {
                    return;
                }
                let is_dir = input.ends_with('/');
                let path = helix_stdx::path::normalize(dir.join(input.trim_end_matches('/')));
                if path.exists() {
                    cx.editor
                        .set_error(format!("{} already exists", path.display()));
                    return;
                }
                match cx.editor.create_path(&path, is_dir) {
                    Ok(()) => refresh_later(cx, path),
                    Err(err) => cx.editor.set_error(format!("Failed to create: {err}")),
                }
            },
        );
    }

    fn prompt_rename(&self, cx: &mut commands::Context) {
        let Some(node) = self.selected() else {
            return;
        };
        let old = node.path.clone();
        ui::prompt_with_input(
            cx,
            "rename: ".into(),
            node.name.clone(),
            None,
            ui::completers::none,
            move |cx, input, event| {
                if event != PromptEvent::Validate || input.trim().is_empty() {
                    return;
                }
                let new = old.parent().unwrap_or(Path::new("")).join(input);
                match move_entry(cx.editor, &old, &new) {
                    Ok(new) => refresh_later(cx, new),
                    Err(err) => cx.editor.set_error(format!("Failed to rename: {err}")),
                }
            },
        );
    }

    fn paste(&mut self, editor: &mut Editor) -> anyhow::Result<()> {
        let Some(cut) = self.cut.clone() else {
            bail!("nothing to paste, mark a file with x first");
        };
        let Some(name) = cut.file_name() else {
            bail!("can't move {}", cut.display());
        };
        let new = match move_entry(editor, &cut, &self.target_dir().join(name)) {
            Ok(new) => new,
            Err(err) => {
                if cut.symlink_metadata().is_err() {
                    self.cut = None;
                }
                return Err(err);
            }
        };
        self.cut = None;
        self.refresh(editor);
        self.reveal(&new);
        Ok(())
    }

    fn delete(&mut self, path: &Path, editor: &mut Editor) -> anyhow::Result<()> {
        let documents: Vec<DocumentId> = editor
            .documents()
            .filter(|doc| {
                doc.path()
                    .is_some_and(|doc_path| doc_path.starts_with(path))
            })
            .map(|doc| doc.id())
            .collect();
        if let Some(doc) = documents
            .iter()
            .filter_map(|id| editor.document(*id))
            .find(|doc| doc.is_modified())
        {
            bail!("{} has unsaved changes", doc.display_name());
        }
        // Delete first: if that fails, the buffers stay open.
        editor.delete_path(path, true)?;
        if self.cut.as_ref().is_some_and(|cut| cut.starts_with(path)) {
            self.cut = None;
        }
        self.refresh(editor);
        let closed = documents.into_iter().fold(true, |closed, id| {
            editor.close_document(id, true).is_ok() && closed
        });
        if !closed {
            bail!("deleted, but couldn't close its buffers");
        }
        editor.set_status(format!("Deleted {}", self.relative(path)));
        Ok(())
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        (self.area.left()..self.area.right()).contains(&column)
            && (self.area.top()..self.area.bottom()).contains(&row)
    }

    pub fn handle_mouse(&mut self, event: &MouseEvent, editor: &mut Editor) {
        if self.confirm_delete.take().is_some() {
            editor.set_status("Delete cancelled");
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.move_cursor(3),
            MouseEventKind::ScrollUp => self.move_cursor(-3),
            MouseEventKind::Down(MouseButton::Left) => {
                // Find the clicked entry in what is on screen, before `focus` reloads the tree and
                // possibly shifts the rows.
                let clicked = (self.body.top()..self.body.bottom())
                    .contains(&event.row)
                    .then(|| self.scroll + (event.row - self.body.top()) as usize)
                    .and_then(|index| self.nodes.get(index))
                    .map(|node| node.path.clone());
                if !self.focused {
                    self.focus(editor);
                }
                let Some(path) = clicked else {
                    return;
                };
                if let Some(index) = self.nodes.iter().position(|node| node.path == path) {
                    self.cursor = index;
                    self.activate(editor);
                }
            }
            _ => {}
        }
        if self.focused {
            self.sync_focus_target(editor);
        }
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, editor: &Editor) {
        // Runs after every event, wherever it went (a picker, a command, a job).
        self.check_focus_target(editor);
        if self.hiding != self.hide_ignored(editor) {
            self.refresh(editor);
        }
        self.area = area;
        let theme = &editor.theme;
        surface.clear_with(
            area,
            theme
                .try_get("ui.explorer")
                .unwrap_or_else(|| theme.get("ui.background")),
        );
        if area.width < 4 || area.height < 3 {
            self.body = Rect::default();
            return;
        }

        let separator = theme.get("ui.window");
        for y in area.top()..area.bottom() {
            surface.set_string(area.right() - 1, y, "│", separator);
        }
        let content = area.clip_right(1);

        let directory = theme.get("ui.text.directory");
        let text = theme.get("ui.text");
        let ignored = theme
            .try_get("ui.explorer.ignored")
            .or_else(|| theme.try_get("ui.text.inactive"))
            .unwrap_or_else(|| Style::default().add_modifier(Modifier::DIM));
        let cursor = if self.focused {
            theme.get("ui.selection")
        } else {
            theme.try_get("ui.cursorline.primary").unwrap_or_default()
        };

        let title = self
            .root
            .file_name()
            .map(|name| format!(" {}/", name.to_string_lossy()))
            .unwrap_or_else(|| format!(" {}", self.root.display()));
        surface.set_stringn(
            content.x,
            content.y,
            &title,
            content.width as usize,
            directory.add_modifier(Modifier::BOLD),
        );

        let mut body = content.clip_top(1);
        if let Some(cut) = &self.cut {
            body = body.clip_bottom(1);
            let name = cut.file_name().unwrap_or_default().to_string_lossy();
            surface.set_stringn(
                content.x,
                body.bottom(),
                &format!(" cut: {name} (p to paste)"),
                content.width as usize,
                ignored,
            );
        }
        self.body = body;

        let height = body.height as usize;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if height > 0 && self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }

        if self.nodes.is_empty() {
            surface.set_stringn(body.x, body.y, " (empty)", body.width as usize, ignored);
            return;
        }

        let current = doc!(editor).path();
        let show_icons = editor.config().explorer.icons;
        for (index, node) in self.nodes.iter().enumerate().skip(self.scroll).take(height) {
            let y = body.y + (index - self.scroll) as u16;
            if index == self.cursor {
                surface.set_style(Rect::new(body.x, y, body.width, 1), cursor);
            }
            let mut style = if node.is_dir { directory } else { text };
            if node.ignored {
                style = style.patch(ignored);
            }
            if current == Some(node.path.as_path()) {
                style = style.add_modifier(Modifier::BOLD);
            }
            if self.cut.as_ref() == Some(&node.path) {
                style = style.add_modifier(Modifier::ITALIC);
            }
            let expanded = self.expanded.contains(&node.path);
            let chevron = match (node.is_dir, expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let indent = format!(" {}{chevron}", "  ".repeat(node.depth));
            let (mut x, _) = surface.set_stringn(body.x, y, &indent, body.width as usize, style);
            if show_icons {
                let (glyph, color) = if node.is_dir {
                    icons::directory(expanded)
                } else {
                    icons::file(&node.name)
                };
                let mut icon_style = Style::default().fg(color);
                if node.ignored {
                    icon_style = icon_style.patch(ignored);
                }
                let width = body.right().saturating_sub(x) as usize;
                (x, _) = surface.set_stringn(x, y, &format!("{glyph} "), width, icon_style);
            }
            let slash = if node.is_dir { "/" } else { "" };
            let width = body.right().saturating_sub(x) as usize;
            surface.set_stringn(x, y, &format!("{}{slash}", node.name), width, style);
        }
    }
}

/// `space E`: opens and focuses the panel, focuses it when open but not focused, or closes it.
pub fn toggle(editor_view: &mut EditorView, editor: &Editor) {
    match &mut editor_view.explorer {
        Some(explorer) if explorer.is_focused() => editor_view.explorer = None,
        Some(explorer) => explorer.focus(editor),
        None => editor_view.explorer = Some(Explorer::new(editor)),
    }
}

/// Refreshes the panel and selects `path` once the prompt that changed the files has closed.
fn refresh_later(cx: &mut compositor::Context, path: PathBuf) {
    cx.jobs.callback(async move {
        Ok(Callback::EditorCompositor(Box::new(
            move |editor, compositor| {
                let explorer = compositor
                    .find::<EditorView>()
                    .and_then(|view| view.explorer.as_mut());
                if let Some(explorer) = explorer {
                    explorer.refresh(editor);
                    explorer.reveal(&path);
                    if explorer.focused {
                        explorer.sync_focus_target(editor);
                    }
                }
            },
        )))
    });
}

/// Moves or renames `old` to `new`, creating the missing parent directories, and returns where it
/// went: `new`, normalized. Open buffers follow the file, including the ones inside a moved
/// directory.
fn move_entry(editor: &mut Editor, old: &Path, new: &Path) -> anyhow::Result<PathBuf> {
    // A rename to `../name` would otherwise leave `..` in the paths of the buffers below.
    let new = helix_stdx::path::normalize(new);
    if old == new {
        return Ok(new);
    }
    // `Editor::move_path` quietly does nothing when `old` is gone.
    if old.symlink_metadata().is_err() {
        bail!("{} no longer exists", old.display());
    }
    if new.symlink_metadata().is_ok() {
        bail!("{} already exists", new.display());
    }
    if new.starts_with(old) {
        bail!("can't move a directory into itself");
    }
    // Missing parent directories, deepest first, removed again if the move fails.
    let created: Vec<PathBuf> = new
        .ancestors()
        .skip(1)
        .take_while(|dir| !dir.exists())
        .map(Path::to_path_buf)
        .collect();
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // `Editor::move_path` only updates the buffer of `old` itself.
    let inside: Vec<(DocumentId, PathBuf)> = if old.is_dir() {
        editor
            .documents()
            .filter_map(|doc| {
                let relative = doc.path()?.strip_prefix(old).ok()?;
                Some((doc.id(), new.join(relative)))
            })
            .collect()
    } else {
        Vec::new()
    };
    if let Err(err) = editor.move_path(old, &new) {
        for dir in created {
            let _ = std::fs::remove_dir(dir);
        }
        return Err(err.into());
    }
    for (id, path) in inside {
        editor.set_doc_path(id, &path);
    }
    Ok(new)
}

/// Whether `key` leads out of the panel: to the menu holding `toggle_explorer` or `file_picker`
/// (the space menu, wherever it is mapped), to the command line, or to `toggle_explorer` itself.
fn leads_to_keymap(keymaps: &Keymaps, mode: Mode, key: KeyEvent) -> bool {
    let keymap = keymaps.map();
    match keymap.get(&mode).and_then(|trie| trie.search(&[key])) {
        Some(KeyTrie::Node(node)) => {
            holds_command(node, "toggle_explorer") || holds_command(node, "file_picker")
        }
        Some(KeyTrie::MappableCommand(command)) => {
            matches!(command.name(), "command_mode" | "toggle_explorer")
        }
        _ => false,
    }
}

fn holds_command(node: &KeyTrieNode, name: &str) -> bool {
    node.values().any(|trie| match trie {
        KeyTrie::MappableCommand(command) => command.name() == name,
        KeyTrie::Sequence(commands) => commands.iter().any(|command| command.name() == name),
        KeyTrie::Node(node) => holds_command(node, name),
    })
}

type Listings = HashMap<PathBuf, Listing>;

/// The entries of a directory, as listed at `stamp`.
struct Listing {
    stamp: Stamp,
    listed_at: SystemTime,
    /// Whether the directory itself was ignored, which makes all its entries ignored.
    dir_ignored: bool,
    entries: Vec<Node>,
}

/// Modification times of a directory and of its `.gitignore`. The first changes when entries are
/// added, removed or renamed, the second when the ignore rules of the directory change.
type Stamp = (Option<SystemTime>, Option<SystemTime>);

fn stamp(dir: &Path) -> Stamp {
    let modified = |path: &Path| path.metadata().and_then(|meta| meta.modified()).ok();
    (modified(dir), modified(&dir.join(".gitignore")))
}

/// Modification times are coarse (a kernel tick on Linux), so a change made right after a listing
/// can keep the same time. Like git's "racy" entries, a listing made this soon after a change is
/// never trusted.
const RACY: Duration = Duration::from_secs(2);

impl Listing {
    fn is_current(&self, dir: &Path) -> bool {
        let (dir_time, ignore_time) = self.stamp;
        let racy = [dir_time, ignore_time]
            .into_iter()
            .flatten()
            .any(|time| time + RACY >= self.listed_at);
        !racy && stamp(dir) == self.stamp
    }
}

/// Drops the listings of the directories that changed on disk, with the ones below them: a
/// `.gitignore` applies to the whole subtree.
fn drop_changed_listings(listings: &mut Listings) {
    let changed: Vec<PathBuf> = listings
        .iter()
        .filter(|(dir, listing)| !listing.is_current(dir))
        .map(|(dir, _)| dir.clone())
        .collect();
    if !changed.is_empty() {
        listings.retain(|dir, _| !changed.iter().any(|changed| dir.starts_with(changed)));
    }
}

fn build_tree(
    root: &Path,
    expanded: &HashSet<PathBuf>,
    hide_ignored: bool,
    listings: &mut Listings,
) -> Vec<Node> {
    let mut nodes = Vec::new();
    push_children(root, 0, false, expanded, hide_ignored, listings, &mut nodes);
    nodes
}

fn push_children(
    dir: &Path,
    depth: usize,
    dir_ignored: bool,
    expanded: &HashSet<PathBuf>,
    hide_ignored: bool,
    listings: &mut Listings,
    nodes: &mut Vec<Node>,
) {
    let listing = match listings.get(dir) {
        Some(listing) if listing.dir_ignored == dir_ignored => listing,
        _ => {
            let listing = Listing {
                stamp: stamp(dir),
                listed_at: SystemTime::now(),
                dir_ignored,
                entries: list_dir(dir, dir_ignored),
            };
            listings.insert(dir.to_path_buf(), listing);
            &listings[dir]
        }
    };
    let entries: Vec<Node> = listing
        .entries
        .iter()
        .filter(|entry| !(hide_ignored && entry.ignored))
        .cloned()
        .collect();
    for mut node in entries {
        node.depth = depth;
        let path = node.path.clone();
        let ignored = node.ignored;
        let expand = node.is_dir && expanded.contains(&path);
        nodes.push(node);
        if expand {
            push_children(
                &path,
                depth + 1,
                ignored,
                expanded,
                hide_ignored,
                listings,
                nodes,
            );
        }
    }
}

/// Entries of `dir`, directories first. Everything inside an ignored directory is ignored too.
fn list_dir(dir: &Path, dir_ignored: bool) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    // Listing the directory again with git's ignore rules applied tells which entries git ignores.
    // This follows nested `.gitignore` files, the ones of parent directories, `.git/info/exclude`
    // and the global excludes file.
    let not_ignored: HashSet<PathBuf> = if dir_ignored {
        HashSet::new()
    } else {
        WalkBuilder::new(dir)
            .max_depth(Some(1))
            .hidden(false)
            .ignore(false)
            .parents(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.depth() == 1)
            .map(|entry| entry.into_path())
            .collect()
    };
    let mut nodes: Vec<Node> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != ".git")
        .map(|entry| {
            let path = entry.path();
            // Only symlinks need another stat to know where they lead.
            let is_dir = match entry.file_type() {
                Ok(file_type) if !file_type.is_symlink() => file_type.is_dir(),
                _ => path.is_dir(),
            };
            Node {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir,
                ignored: dir_ignored || !not_ignored.contains(&path),
                depth: 0,
                path,
            }
        })
        .collect();
    nodes.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[Node]) -> Vec<(String, bool)> {
        nodes
            .iter()
            .map(|node| {
                let indent = "  ".repeat(node.depth);
                (format!("{indent}{}", node.name), node.ignored)
            })
            .collect()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/nested/.gitignore"), "secret.txt\n").unwrap();
        std::fs::write(root.join("src/nested/secret.txt"), "").unwrap();
        std::fs::write(root.join("src/nested/public.txt"), "").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "").unwrap();
        std::fs::write(root.join("build.log"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        dir
    }

    #[test]
    fn lists_directories_first_and_dims_ignored_files() {
        let dir = repo();
        let root = dir.path();
        let expanded: HashSet<PathBuf> = ["src", "src/nested", "target"]
            .iter()
            .map(|path| root.join(path))
            .collect();
        let tree = build_tree(root, &expanded, false, &mut HashMap::new());
        let expected = [
            ("src", false),
            ("  nested", false),
            ("    .gitignore", false),
            ("    public.txt", false),
            ("    secret.txt", true),
            ("  main.rs", false),
            ("target", true),
            ("  debug", true),
            (".gitignore", false),
            ("build.log", true),
            ("README.md", false),
        ];
        let expected: Vec<(String, bool)> = expected
            .iter()
            .map(|(name, ignored)| (name.to_string(), *ignored))
            .collect();
        assert_eq!(names(&tree), expected);
    }

    #[test]
    fn space_menu_and_command_line_lead_to_the_keymap() {
        let keymaps = Keymaps::default();
        for key in [key!(' '), key!(':')] {
            assert!(leads_to_keymap(&keymaps, Mode::Normal, key));
        }
        // Other menus and commands would act on the buffer behind the panel.
        for key in [key!('m'), key!('z'), key!('i'), key!('u'), ctrl!('w')] {
            assert!(!leads_to_keymap(&keymaps, Mode::Normal, key));
        }
    }

    #[test]
    fn remapped_space_menu_leads_to_the_keymap() {
        let mut keymap = crate::keymap::default::default();
        let normal = keymap.get_mut(&Mode::Normal).unwrap().node_mut().unwrap();
        let space = normal.shift_remove(&key!(' ')).unwrap();
        normal.insert(key!(','), space);
        let keymaps = Keymaps::new(Box::new(arc_swap::ArcSwap::from_pointee(keymap)));
        assert!(leads_to_keymap(&keymaps, Mode::Normal, key!(',')));
        assert!(!leads_to_keymap(&keymaps, Mode::Normal, key!(' ')));
    }

    #[test]
    fn unfocus_cancels_a_pending_delete() {
        let mut explorer = Explorer::empty(PathBuf::from("/project"));
        explorer.focused = true;
        explorer.confirm_delete = Some(PathBuf::from("/project/src"));
        explorer.unfocus();
        assert!(explorer.confirm_delete.is_none());
    }

    #[test]
    fn hides_ignored_files() {
        let dir = repo();
        let root = dir.path();
        let expanded: HashSet<PathBuf> = [root.join("src"), root.join("src/nested")].into();
        let tree = build_tree(root, &expanded, true, &mut HashMap::new());
        let names: Vec<String> = names(&tree).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                "src",
                "  nested",
                "    .gitignore",
                "    public.txt",
                "  main.rs",
                ".gitignore",
                "README.md"
            ]
        );
    }

    #[test]
    fn lists_again_only_what_changed() {
        let dir = repo();
        let root = dir.path();
        let expanded: HashSet<PathBuf> = [root.join("src")].into();
        let mut listings = HashMap::new();
        let names_of = |listings: &mut Listings| -> Vec<(String, bool)> {
            names(&build_tree(root, &expanded, false, listings))
        };
        names_of(&mut listings);

        // Unchanged directories come from the listings.
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        assert!(!names_of(&mut listings).contains(&("  lib.rs".into(), false)));
        // The directory's modification time changed, so it is listed again.
        drop_changed_listings(&mut listings);
        assert!(names_of(&mut listings).contains(&("  lib.rs".into(), false)));

        // A changed `.gitignore` lists its directory and everything below it again.
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n*.rs\n").unwrap();
        drop_changed_listings(&mut listings);
        let tree = names_of(&mut listings);
        assert!(tree.contains(&("  lib.rs".into(), true)));
        assert!(tree.contains(&("  main.rs".into(), true)));
    }

    #[test]
    fn listing_is_current_until_the_directory_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::open(root)
            .unwrap()
            .set_modified(an_hour_ago)
            .unwrap();
        let mut listings = HashMap::new();
        build_tree(root, &HashSet::new(), false, &mut listings);
        assert!(listings[root].is_current(root));

        std::fs::write(root.join("new.txt"), "").unwrap();
        assert!(!listings[root].is_current(root));
    }
}
