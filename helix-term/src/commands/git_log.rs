use std::path::{Path, PathBuf};
use std::sync::Arc;

use helix_event::status;
use helix_vcs::{CommitInfo, FileChange};
use helix_view::Editor;
use tui::text::Span;

use super::{diff_view, file_change_columns};
use crate::compositor::{self, Compositor};
use crate::job::Callback;
use crate::ui::{overlay::overlaid, picker, Picker, PickerColumn};
use crate::{ctrl, key};

fn trust_full(editor: &Editor, cwd: &Path) -> bool {
    editor
        .workspace_trust
        .query(
            &helix_loader::find_workspace_in(cwd).0,
            helix_loader::workspace_trust::TrustQuery::Git,
        )
        .is_trusted()
}

/// Lists the commits reachable from HEAD. Enter lists the files changed by the selected commit.
pub(crate) fn commit_picker(editor: &mut Editor) -> Option<Picker<CommitInfo, CommitStyle>> {
    let cwd = helix_stdx::env::current_working_dir();
    if !cwd.exists() {
        editor.set_error("Current working directory does not exist");
        return None;
    }

    let columns = [
        PickerColumn::new("commit", |commit: &CommitInfo, style: &CommitStyle| {
            Span::styled(commit.short_id.clone(), style.id).into()
        }),
        PickerColumn::new("date", |commit: &CommitInfo, style: &CommitStyle| {
            Span::styled(commit.date.clone(), style.date).into()
        }),
        PickerColumn::new("author", |commit: &CommitInfo, style: &CommitStyle| {
            Span::styled(commit.author.clone(), style.author).into()
        }),
        PickerColumn::new("summary", |commit: &CommitInfo, _: &CommitStyle| {
            commit.summary.as_str().into()
        }),
    ];
    let style = CommitStyle {
        id: editor.theme.get("constant"),
        date: editor.theme.get("comment"),
        author: editor.theme.get("variable"),
    };

    let open_cwd = cwd.clone();
    let picker = Picker::new(columns, 3, [], style, move |cx, commit, _action| {
        open_commit_files(cx, open_cwd.clone(), commit)
    })
    .with_stacked_key_handler(key!(Enter), {
        let cwd = cwd.clone();
        move |cx, commit| open_commit_files(cx, cwd.clone(), commit)
    });
    let injector = picker.injector();

    let trust_full = trust_full(editor, &cwd);
    editor
        .diff_providers
        .clone()
        .for_each_commit(cwd, trust_full, move |commit| match commit {
            Ok(commit) => injector.push(commit).is_ok(),
            Err(err) => {
                status::report_blocking(err);
                false
            }
        });
    Some(picker)
}

pub(crate) struct CommitStyle {
    id: helix_view::graphics::Style,
    date: helix_view::graphics::Style,
    author: helix_view::graphics::Style,
}

/// Pushes the picker of the files changed by `commit` on top of the commit picker.
fn open_commit_files(cx: &mut compositor::Context, cwd: PathBuf, commit: &CommitInfo) {
    let commit = Arc::new(commit.clone());
    cx.jobs.callback(async move {
        let call = move |editor: &mut Editor, compositor: &mut Compositor| {
            let trust_full = trust_full(editor, &cwd);
            let changes = match editor
                .diff_providers
                .commit_changes(&cwd, trust_full, &commit.id)
            {
                Ok(changes) => changes,
                Err(err) => {
                    editor.set_error(format!("{err}"));
                    return;
                }
            };
            if changes.is_empty() {
                editor.set_status(format!("{} changes no files", commit.short_id));
                return;
            }
            editor.set_status(format!("{} {}", commit.short_id, commit.summary));
            let picker = commit_files_picker(editor, cwd, commit, changes);
            compositor.push(Box::new(overlaid(picker)));
        };
        Ok(Callback::EditorCompositor(Box::new(call)))
    });
}

/// The files changed by `commit`. Enter and `C-g` open the diff of the selected file on top.
fn commit_files_picker(
    editor: &Editor,
    cwd: PathBuf,
    commit: Arc<CommitInfo>,
    changes: Vec<FileChange>,
) -> Picker<FileChange, super::FileChangeData> {
    let (columns, data) = file_change_columns(editor, cwd.clone());
    let show_diff = move |cx: &mut compositor::Context, change: &FileChange| {
        let cwd = cwd.clone();
        let commit = commit.clone();
        let change = change.clone();
        cx.jobs.callback(async move {
            let call = move |editor: &mut Editor, compositor: &mut Compositor| {
                if let Err(err) = diff_view::open_commit(editor, compositor, &cwd, &commit, &change)
                {
                    editor.set_error(err.to_string());
                }
            };
            Ok(Callback::EditorCompositor(Box::new(call)))
        });
    };

    Picker::new(
        columns,
        1,
        changes,
        data,
        |cx, change: &FileChange, action| {
            // the file as it is now, closing the commit picker below too
            let path = change.path().to_path_buf();
            if let Err(err) = cx.editor.open(&path, action) {
                cx.editor
                    .set_error(format!("unable to open \"{}\": {err}", path.display()));
            }
            cx.jobs.callback(async move {
                let call = |_: &mut Editor, compositor: &mut Compositor| {
                    compositor.remove(picker::ID);
                };
                Ok(Callback::EditorCompositor(Box::new(call)))
            });
        },
    )
    .with_stacked_key_handler(key!(Enter), show_diff.clone())
    .with_stacked_key_handler(ctrl!('g'), show_diff)
}
