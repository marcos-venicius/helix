use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use helix_core::syntax::Loader;
use helix_view::Theme;

use helix_vcs::{CommitInfo, FileChange};
use helix_view::Editor;

use crate::compositor::Compositor;
use crate::job::{self, Callback};
use crate::ui::diff_view::{DiffView, OnOpen, Side, ID};
use crate::ui::picker;

/// One version of the file, read but not highlighted yet.
struct SideInput {
    title: String,
    text: String,
    /// Path used to pick the language.
    path: PathBuf,
}

/// Both versions of a file, ready to be compared.
struct DiffInputs {
    left: SideInput,
    right: SideInput,
    /// File to open with Enter, if it still exists.
    path: Option<PathBuf>,
    tab_width: usize,
    wrap: bool,
    /// Path shown when there are no changes.
    label: String,
}

/// Tells the requests apart, so a placeholder only takes the diff it was shown for.
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

impl DiffInputs {
    /// Highlights both versions, in parallel, and aligns them.
    fn build(self, loader: &Loader, theme: &Theme) -> DiffView {
        let side = |input: SideInput| Side::new(input.title, input.text, &input.path, loader, theme);
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| side(self.left));
            let right = side(self.right);
            let left = left
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            (left, right)
        });
        DiffView::new(left, right, self.path, self.tab_width).with_soft_wrap(self.wrap)
    }

    /// Shows a placeholder at once and computes the diff in the background: parsing a large
    /// file for syntax highlighting takes long enough to freeze the editor. The diff replaces
    /// the placeholder, unless it was closed in the meantime.
    fn show(self, editor: &mut Editor, compositor: &mut Compositor, on_open: Option<OnOpen>) {
        if self.left.text == self.right.text {
            editor.set_status(format!("No changes in {}", self.label));
            return;
        }
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        let label = self.label.clone();
        compositor.push(Box::new(DiffView::loading(token, label.clone())));

        let loader = editor.syn_loader.load_full();
        let theme = editor.theme.clone();
        tokio::task::spawn_blocking(move || {
            let built = std::panic::catch_unwind(AssertUnwindSafe(|| self.build(&loader, &theme)));
            job::dispatch_blocking(move |editor, compositor| {
                let waiting = |compositor: &mut Compositor| {
                    compositor
                        .find_id::<DiffView>(ID)
                        .is_some_and(|view| view.loading_token() == Some(token))
                };
                if !waiting(compositor) {
                    return;
                }
                let view = match built {
                    Ok(view) if view.has_changes() => view,
                    Ok(_) => {
                        compositor.remove(ID);
                        editor.set_status(format!("No changes in {label}"));
                        return;
                    }
                    Err(_) => {
                        compositor.remove(ID);
                        editor.set_error(format!("Failed to compute the diff of {label}"));
                        return;
                    }
                };
                let view = match on_open {
                    Some(on_open) => view.with_on_open(on_open),
                    None => view,
                };
                if let Some(placeholder) = compositor.find_id::<DiffView>(ID) {
                    *placeholder = view;
                }
            });
        });
    }
}

fn relative(path: &Path) -> String {
    helix_stdx::path::get_relative_path(path)
        .display()
        .to_string()
}

/// The HEAD version of `base_path` and the current contents of `path` (they only differ for
/// renamed files).
fn head_inputs(editor: &Editor, base_path: &Path, path: &Path) -> anyhow::Result<DiffInputs> {
    let workspace = helix_loader::find_workspace_in(path.parent().unwrap_or(path)).0;
    let trust_full = editor
        .workspace_trust
        .query(&workspace, helix_loader::workspace_trust::TrustQuery::Git)
        .is_trusted();
    let base = editor.diff_providers.get_diff_base(base_path, trust_full);
    let left_title = match &base {
        Some(_) => format!("HEAD: {}", relative(base_path)),
        None => "HEAD: (new file)".to_string(),
    };
    let base = String::from_utf8(base.unwrap_or_default())
        .map_err(|_| anyhow::anyhow!("{} is a binary file", relative(base_path)))?;

    // an open buffer may have unsaved changes: show those
    let (current, right_title, tab_width, exists) = match editor.document_by_path(path) {
        Some(doc) => {
            let modified = if doc.is_modified() { " [modified]" } else { "" };
            (
                doc.text().to_string(),
                format!("Buffer: {}{modified}", relative(path)),
                doc.tab_width(),
                true,
            )
        }
        None => match std::fs::read(path) {
            Ok(bytes) => (
                String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("{} is a binary file", relative(path)))?,
                format!("Working tree: {}", relative(path)),
                4,
                true,
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (
                String::new(),
                format!("Working tree: {} (deleted)", relative(path)),
                4,
                false,
            ),
            Err(err) => anyhow::bail!("failed to read {}: {err}", relative(path)),
        },
    };

    Ok(DiffInputs {
        left: SideInput {
            title: left_title,
            text: base,
            path: base_path.to_path_buf(),
        },
        right: SideInput {
            title: right_title,
            text: current,
            path: path.to_path_buf(),
        },
        path: exists.then(|| path.to_path_buf()),
        tab_width,
        wrap: soft_wrap(editor, path),
        label: relative(path),
    })
}

/// The HEAD path and the current path to diff for an entry of the changed files picker.
pub(crate) fn paths_of(change: &FileChange) -> (PathBuf, PathBuf) {
    match change {
        FileChange::Renamed { from_path, to_path } => (from_path.clone(), to_path.clone()),
        change => (change.path().to_path_buf(), change.path().to_path_buf()),
    }
}

/// Opens the diff for an entry of the changed files picker, on top of the picker. Opening the
/// file from the diff closes the picker too.
pub(crate) fn open_from_picker(cx: &mut crate::compositor::Context, change: &FileChange) {
    let (base_path, path) = paths_of(change);
    cx.jobs.callback(async move {
        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            match head_inputs(editor, &base_path, &path) {
                Ok(inputs) => inputs.show(
                    editor,
                    compositor,
                    Some(Box::new(|compositor: &mut Compositor| {
                        compositor.remove(picker::ID);
                    })),
                ),
                Err(err) => editor.set_error(err.to_string()),
            }
        };
        Ok(Callback::EditorCompositor(Box::new(call)))
    });
}

/// Opens the diff for the file of the current buffer.
pub(crate) fn open_current(editor: &mut Editor, compositor: &mut Compositor) {
    let Some(path) = helix_view::doc!(editor).path().map(Path::to_path_buf) else {
        editor.set_error("The current buffer has no file");
        return;
    };
    match head_inputs(editor, &path, &path) {
        Ok(inputs) => inputs.show(editor, compositor, None),
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// Opens the diff of `change` in `commit` against the commit's first parent, on top of the
/// commit pickers. Opening the file from the diff closes those pickers too.
pub(crate) fn open_commit(
    editor: &mut Editor,
    compositor: &mut Compositor,
    cwd: &Path,
    commit: &CommitInfo,
    change: &FileChange,
) -> anyhow::Result<()> {
    let trust_full = editor
        .workspace_trust
        .query(
            &helix_loader::find_workspace_in(cwd).0,
            helix_loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted();
    let read = |id: &str, path: &Path| -> anyhow::Result<Option<String>> {
        editor
            .diff_providers
            .file_at_commit(cwd, trust_full, id, path)?
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|_| anyhow::anyhow!("{} is a binary file", relative(path)))
            })
            .transpose()
    };

    let (base_path, path) = paths_of(change);
    let short = &commit.short_id;
    let (base, left_title) = match read(&format!("{}^", commit.id), &base_path)? {
        Some(text) => (text, format!("{short}^: {}", relative(&base_path))),
        None => (String::new(), format!("{short}^: (new file)")),
    };
    let (current, right_title) = match read(&commit.id, &path)? {
        Some(text) => (text, format!("{short}: {}", relative(&path))),
        None => (
            String::new(),
            format!("{short}: {} (deleted)", relative(&path)),
        ),
    };
    let tab_width = editor
        .document_by_path(&path)
        .map_or(4, |doc| doc.tab_width());

    let inputs = DiffInputs {
        left: SideInput {
            title: left_title,
            text: base,
            path: base_path,
        },
        right: SideInput {
            title: right_title,
            text: current,
            path: path.clone(),
        },
        path: path.exists().then_some(path.clone()),
        tab_width,
        wrap: soft_wrap(editor, &path),
        label: relative(&path),
    };
    inputs.show(
        editor,
        compositor,
        // the commit's files picker and the commit picker below it
        Some(Box::new(|compositor: &mut Compositor| {
            compositor.remove(picker::ID);
            compositor.remove(picker::ID);
        })),
    );
    Ok(())
}

/// Whether lines of `path` wrap in the editor: the setting of its buffer when it is open, else
/// `soft-wrap.enable` of its language, else of the editor.
fn soft_wrap(editor: &Editor, path: &Path) -> bool {
    if let Some(doc) = editor.document_by_path(path) {
        // any width wide enough to wrap
        return doc.text_format(1000, None).soft_wrap;
    }
    let loader = editor.syn_loader.load();
    loader
        .language_for_filename(path)
        .and_then(|language| loader.language(language).config().soft_wrap.as_ref()?.enable)
        .or(editor.config().soft_wrap.enable)
        .unwrap_or(false)
}
