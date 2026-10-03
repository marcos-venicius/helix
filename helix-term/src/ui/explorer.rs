//! A file tree in a side panel on the left of the editor, toggled with `space E`. Files ignored by
//! git are dimmed (or hidden with `editor.explorer.hide-gitignored`), and the panel can create,
//! rename, move (cut and paste) and delete files.
//!
//! The panel lives in [`EditorView`], which shrinks the editor area to make room for it and hands
//! it the keys while it is focused.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::bail;
use helix_view::document::Mode;
use helix_view::editor::Action;
use helix_view::graphics::{Modifier, Rect, Style};
use helix_view::input::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use helix_view::{doc, DocumentId, Editor};
use ignore::WalkBuilder;
use tui::buffer::Buffer as Surface;

use crate::commands;
use crate::compositor;
use crate::job::Callback;
use crate::ui::{self, EditorView, PromptEvent};
use crate::{ctrl, key};

const HELP: &str = "a: new (end with / for a directory)  r: rename  x: cut  p: paste  d: delete  \
                    R: refresh  H: show/hide git ignored  q: close  esc: back to the editor";

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
}

impl Explorer {
    pub fn new(editor: &Editor) -> Self {
        let mut explorer = Self::empty(helix_stdx::env::current_working_dir());
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
        }
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn focus(&mut self, editor: &Editor) {
        self.focused = true;
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

    /// Whether the panel wants `key` instead of the editor keymap. Space and `:` still reach the
    /// keymap, so `space E`, the pickers and commands keep working with the panel focused. While a
    /// deletion waits for its confirmation every key comes here, so that only the very next key
    /// can confirm it.
    pub fn takes_key(&self, key: KeyEvent, mode: Mode) -> bool {
        self.focused
            && (self.confirm_delete.is_some()
                || mode == Mode::Insert
                || (key != key!(' ') && key != key!(':')))
    }

    fn hide_ignored(&self, editor: &Editor) -> bool {
        editor.config().explorer.hide_gitignored != self.flip_hidden
    }

    fn selected(&self) -> Option<&Node> {
        self.nodes.get(self.cursor)
    }

    /// Lists the tree again from disk, keeping the expanded directories and the selection.
    pub fn refresh(&mut self, editor: &Editor) {
        let cwd = helix_stdx::env::current_working_dir();
        if cwd != self.root {
            self.root = cwd;
            self.expanded.clear();
            self.cursor = 0;
            self.scroll = 0;
        }
        self.hiding = self.hide_ignored(editor);
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let selected = self.selected().map(|node| node.path.clone());
        self.expanded.retain(|dir| dir.is_dir());
        self.nodes = build_tree(&self.root, &self.expanded, self.hiding);
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

    /// Handles a key while the panel is focused. Returns false when the panel should close.
    pub fn handle_key(&mut self, key: KeyEvent, cx: &mut commands::Context) -> bool {
        if let Some(path) = self.confirm_delete.take() {
            if key == key!('y') {
                if let Err(err) = self.delete(&path, cx.editor) {
                    cx.editor.set_error(format!("Failed to delete: {err}"));
                }
            } else {
                cx.editor.set_status("Delete cancelled");
            }
            return true;
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
                self.refresh(cx.editor);
                cx.editor.set_status("Explorer refreshed");
            }
            key!('H') => {
                self.flip_hidden = !self.flip_hidden;
                self.refresh(cx.editor);
            }
            key!('?') => cx.editor.set_status(HELP),
            key!(Esc) => self.unfocus(),
            key!('q') => return false,
            _ => {}
        }
        true
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
        let new = move_entry(editor, &cut, &self.target_dir().join(name))?;
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
                if !self.focused {
                    self.focus(editor);
                }
                if event.row < self.body.top() || event.row >= self.body.bottom() {
                    return;
                }
                let index = self.scroll + (event.row - self.body.top()) as usize;
                if index < self.nodes.len() {
                    self.cursor = index;
                    self.activate(editor);
                }
            }
            _ => {}
        }
    }

    pub fn render(&mut self, area: Rect, surface: &mut Surface, editor: &Editor) {
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
            let icon = match (node.is_dir, self.expanded.contains(&node.path)) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let slash = if node.is_dir { "/" } else { "" };
            let line = format!(" {}{icon}{}{slash}", "  ".repeat(node.depth), node.name);
            surface.set_stringn(body.x, y, &line, body.width as usize, style);
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
    if new.exists() {
        bail!("{} already exists", new.display());
    }
    if new.starts_with(old) {
        bail!("can't move a directory into itself");
    }
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
    editor.move_path(old, &new)?;
    for (id, path) in inside {
        editor.set_doc_path(id, &path);
    }
    Ok(new)
}

fn build_tree(root: &Path, expanded: &HashSet<PathBuf>, hide_ignored: bool) -> Vec<Node> {
    let mut nodes = Vec::new();
    push_children(root, 0, false, expanded, hide_ignored, &mut nodes);
    nodes
}

fn push_children(
    dir: &Path,
    depth: usize,
    dir_ignored: bool,
    expanded: &HashSet<PathBuf>,
    hide_ignored: bool,
    nodes: &mut Vec<Node>,
) {
    for node in list_dir(dir, depth, dir_ignored) {
        if hide_ignored && node.ignored {
            continue;
        }
        let path = node.path.clone();
        let ignored = node.ignored;
        let expand = node.is_dir && expanded.contains(&path);
        nodes.push(node);
        if expand {
            push_children(&path, depth + 1, ignored, expanded, hide_ignored, nodes);
        }
    }
}

/// Entries of `dir`, directories first. Everything inside an ignored directory is ignored too.
fn list_dir(dir: &Path, depth: usize, dir_ignored: bool) -> Vec<Node> {
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
            Node {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: path.is_dir(),
                ignored: dir_ignored || !not_ignored.contains(&path),
                depth,
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
        let tree = build_tree(root, &expanded, false);
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
    fn pending_delete_takes_every_key() {
        let mut explorer = Explorer::empty(PathBuf::from("/project"));
        assert!(!explorer.takes_key(key!(' '), Mode::Normal));
        assert!(!explorer.takes_key(key!(':'), Mode::Normal));
        explorer.confirm_delete = Some(PathBuf::from("/project/src"));
        assert!(explorer.takes_key(key!(' '), Mode::Normal));
        assert!(explorer.takes_key(key!(':'), Mode::Normal));
        explorer.unfocus();
        assert!(explorer.confirm_delete.is_none());
    }

    #[test]
    fn hides_ignored_files() {
        let dir = repo();
        let root = dir.path();
        let expanded: HashSet<PathBuf> = [root.join("src"), root.join("src/nested")].into();
        let tree = build_tree(root, &expanded, true);
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
}
