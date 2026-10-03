use std::path::{Path, PathBuf};

use helix_vcs::FileChange;
use helix_view::Editor;

use crate::compositor::Compositor;
use crate::ui::diff_view::{DiffView, Side};

/// Opens a side by side diff between the HEAD version of `base_path` and the current contents
/// of `path` (they only differ for renamed files).
pub(crate) fn open(
    editor: &mut Editor,
    compositor: &mut Compositor,
    base_path: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    let relative = |path: &Path| {
        helix_stdx::path::get_relative_path(path)
            .display()
            .to_string()
    };

    let workspace = helix_loader::find_workspace_in(path.parent().unwrap_or(path)).0;
    let trust_full = editor
        .workspace_trust
        .query(&workspace, helix_loader::workspace_trust::TrustQuery::Git)
        .is_trusted();
    let base = editor.diff_providers.get_diff_base(base_path, trust_full);
    let left_title = match &base {
        Some(_) => format!("HEAD: {}", relative(base_path)),
        None => "HEAD: (not tracked)".to_string(),
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

    let left = Side::new(left_title, base, base_path, editor);
    let right = Side::new(right_title, current, path, editor);
    let view = DiffView::new(left, right, exists.then(|| path.to_path_buf()), tab_width);
    if !view.has_changes() {
        editor.set_status(format!("No changes in {}", relative(path)));
        return Ok(());
    }
    compositor.push(Box::new(view));
    Ok(())
}

/// The HEAD path and the current path to diff for an entry of the changed files picker.
pub(crate) fn paths_of(change: &FileChange) -> (PathBuf, PathBuf) {
    match change {
        FileChange::Renamed { from_path, to_path } => (from_path.clone(), to_path.clone()),
        change => (change.path().to_path_buf(), change.path().to_path_buf()),
    }
}

/// Opens the diff for an entry of the changed files picker, once the picker has closed.
pub(crate) fn open_from_picker(cx: &mut crate::compositor::Context, change: &FileChange) {
    let (base_path, path) = paths_of(change);
    cx.jobs.callback(async move {
        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            if let Err(err) = open(editor, compositor, &base_path, &path) {
                editor.set_error(err.to_string());
            }
        };
        Ok(crate::job::Callback::EditorCompositor(Box::new(call)))
    });
}

/// Opens the diff for the file of the current buffer.
pub(crate) fn open_current(editor: &mut Editor, compositor: &mut Compositor) {
    let Some(path) = helix_view::doc!(editor).path().map(Path::to_path_buf) else {
        editor.set_error("The current buffer has no file");
        return;
    };
    if let Err(err) = open(editor, compositor, &path, &path) {
        editor.set_error(err.to_string());
    }
}
