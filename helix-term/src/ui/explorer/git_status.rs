//! Git status of the entries of the panel, like the markers of VS Code's explorer: changed files
//! show a letter and a color, and directories holding changes show a dot in the color of their
//! most important change. `git status` runs in the background, the panel picks up its result on
//! the next render.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use helix_loader::workspace_trust::TrustQuery;
use helix_vcs::FileChange;
use helix_view::graphics::Style;
use helix_view::{DocumentId, Editor, Theme};

/// The change of a file, or the most important change below a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    Untracked,
    Added,
    Renamed,
    /// Only on directories, deleted files aren't in the tree.
    Deleted,
    Modified,
    Conflict,
}

impl Mark {
    fn of(change: &FileChange) -> Self {
        match change {
            FileChange::Untracked { .. } => Self::Untracked,
            FileChange::Added { .. } => Self::Added,
            FileChange::Modified { .. } => Self::Modified,
            FileChange::Conflict { .. } => Self::Conflict,
            FileChange::Deleted { .. } => Self::Deleted,
            FileChange::Renamed { .. } => Self::Renamed,
        }
    }

    /// The letter shown next to a file.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Untracked => "U",
            Self::Added => "A",
            Self::Renamed => "R",
            Self::Deleted => "D",
            Self::Modified => "M",
            Self::Conflict => "!",
        }
    }

    /// The same scopes as the changed files picker (`space g`).
    pub fn style(self, theme: &Theme) -> Style {
        theme.get(match self {
            Self::Untracked | Self::Added => "diff.plus",
            Self::Renamed => "diff.delta.moved",
            Self::Deleted => "diff.minus",
            Self::Modified => "diff.delta",
            Self::Conflict => "diff.delta.conflict",
        })
    }
}

type Marks = HashMap<PathBuf, Mark>;

/// Result of a request, tagged with its number so that a late answer to an older request doesn't
/// replace a newer one.
type Slot = Arc<Mutex<Option<(u64, Marks)>>>;

#[derive(Default)]
pub struct GitStatus {
    marks: Marks,
    slot: Slot,
    /// Number of the last request.
    requested: u64,
    /// Buffers with unsaved changes at the last check: one that leaves the set was saved (or its
    /// changes undone), which may change the status.
    modified: HashSet<DocumentId>,
}

impl GitStatus {
    pub fn get(&self, path: &Path) -> Option<Mark> {
        self.marks.get(path).copied()
    }

    /// Runs `git status` on `root` in the background. Without the `git-status` option, or outside
    /// a runtime (tests), the marks are only cleared.
    pub fn request(&mut self, root: &Path, editor: &Editor) {
        self.requested += 1;
        let handle = tokio::runtime::Handle::try_current();
        let (true, Ok(handle)) = (editor.config().explorer.git_status, handle) else {
            self.marks.clear();
            return;
        };
        let trust_full = editor
            .workspace_trust
            .query(&helix_loader::find_workspace_in(root).0, TrustQuery::Git)
            .is_trusted();
        let providers = editor.diff_providers.clone();
        let root = root.to_path_buf();
        let slot = self.slot.clone();
        let id = self.requested;
        handle.spawn_blocking(move || {
            let marks = match providers.changed_files(&root, trust_full) {
                Ok(changes) => marks(&root, &changes),
                // Not a repository.
                Err(err) => {
                    log::debug!("explorer git status: {err:#?}");
                    Marks::new()
                }
            };
            let mut slot = slot.lock().unwrap();
            if slot.as_ref().is_none_or(|(stored, _)| *stored < id) {
                *slot = Some((id, marks));
                helix_event::request_redraw();
            }
        });
    }

    /// Picks up the result of the last request, and requests again when a buffer was saved.
    pub fn update(&mut self, root: &Path, editor: &Editor) {
        if let Some((id, marks)) = self.slot.lock().unwrap().take() {
            if id == self.requested {
                self.marks = marks;
            }
        }
        let modified: HashSet<DocumentId> = editor
            .documents()
            .filter(|doc| doc.is_modified())
            .map(|doc| doc.id())
            .collect();
        let saved = self.modified.iter().any(|id| !modified.contains(id));
        self.modified = modified;
        if saved {
            self.request(root, editor);
        }
    }
}

/// The mark of each changed file, and of each directory between them and `root`.
fn marks(root: &Path, changes: &[FileChange]) -> Marks {
    let mut marks = Marks::new();
    for change in changes {
        let mark = Mark::of(change);
        let path = change.path();
        if !path.starts_with(root) {
            continue;
        }
        if mark != Mark::Deleted {
            marks.insert(path.to_path_buf(), mark);
        }
        for dir in path.ancestors().skip(1) {
            if !dir.starts_with(root) || dir == root {
                break;
            }
            let dir_mark = marks.entry(dir.to_path_buf()).or_insert(mark);
            *dir_mark = (*dir_mark).max(mark);
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_take_their_most_important_change() {
        let root = Path::new("/repo");
        let path = |relative: &str| root.join(relative);
        let changes = [
            FileChange::Untracked {
                path: path("src/new.rs"),
            },
            FileChange::Modified {
                path: path("src/ui/view.rs"),
            },
            FileChange::Deleted {
                path: path("docs/old.md"),
            },
            FileChange::Renamed {
                from_path: path("a.txt"),
                to_path: path("b.txt"),
            },
            FileChange::Modified {
                path: PathBuf::from("/elsewhere/file.rs"),
            },
        ];
        let marks = marks(root, &changes);
        let expected: Marks = [
            ("src/new.rs", Mark::Untracked),
            ("src/ui/view.rs", Mark::Modified),
            ("src/ui", Mark::Modified),
            ("src", Mark::Modified),
            ("docs", Mark::Deleted),
            ("b.txt", Mark::Renamed),
        ]
        .into_iter()
        .map(|(relative, mark)| (path(relative), mark))
        .collect();
        assert_eq!(marks, expected);
    }
}
