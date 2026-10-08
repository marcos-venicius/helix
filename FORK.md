# Fork features

This fork ([marcos-venicius/helix](https://github.com/marcos-venicius/helix)) is upstream
[Helix](https://github.com/helix-editor/helix) plus the features below. They are not part of
upstream, so please report issues with them here. Everything else works and is documented exactly
as in the [Helix documentation](https://docs.helix-editor.com/).

- [Claude Code popup](#claude-code-popup)
- [Side by side git diff](#side-by-side-git-diff)
- [Git commit log](#git-commit-log)
- [Staged files in the changed files picker](#staged-files-in-the-changed-files-picker)
- [File explorer side panel](#file-explorer-side-panel)

To build and install the fork, from the `marcos` branch:

```sh
cargo install --path helix-term --locked
```

## Claude Code popup

Runs [Claude Code](https://code.claude.com/) in a terminal embedded in a popup. Claude gets the
current file, line and selection as context. Sessions keep running in the background when the popup
is hidden, so you can switch between Claude and the code without losing the conversation. The
`claude` CLI must be installed and in `PATH`.

### Usage

| Keys / command | Action |
| --- | --- |
| `space i`, `:claude` (alias `:ai`) | Toggle the popup. The first time, start a session with the current file, line and selection as context. |
| `space I` | Pick one of the running sessions, or start a new one. |
| `:claude <prompt>` | Start a new session and send `<prompt>` as the first message. |
| `C-\` (inside the popup) | Hide the popup. The session keeps running. |

When you reopen a session from another file or line, a reference such as `@src/main.rs#L10-20` is
pasted into Claude's prompt, without sending it, so you can ask about that spot. Reopening from the
same spot pastes nothing.

Every other key, the mouse and pastes go to Claude, so its own shortcuts work in the popup.
Examples: `PageUp`/`PageDown` or the mouse wheel to scroll, `C-o` for the transcript, `/exit` to end
the session.

Buffers without unsaved changes are reloaded from disk when the popup is hidden or a session ends,
so Claude's edits show up in the editor. A new session can't be started from a buffer with unsaved
changes, to avoid conflicting with Claude's edits on disk.

Sessions live until Claude exits or the editor closes. Closing and reopening Helix always starts
fresh.

### Limitations

- Keys are sent with the legacy terminal encoding, which can't tell `C-S-c` from `C-c`. So
  `C-S-<key>` shortcuts reach Claude as `C-<key>`. `S-Enter` and `A-Enter` are sent as `ESC CR`,
  which Claude treats as a newline.
- Claude keeps running while the popup is hidden, so it can change a file you are editing. A buffer
  with unsaved changes is not reloaded, and `:w` will warn that the file changed on disk.
- Claude tracks the mouse, so text can't be selected with the mouse in the popup.

### Ideas

- A file watcher that reloads buffers as soon as Claude changes them, rather than only when the
  popup hides. It could use the `notify` crate to watch open documents, send change events to the
  application event loop, reload buffers without unsaved changes (same logic as
  `reload_documents`) and warn for the others. Helix's own writes would be ignored by comparing the
  mtime with `Document::last_saved_time`. This would be opt-in, e.g. `editor.auto-reload = true`.
- The kitty keyboard protocol in the embedded terminal, so `C-S-<key>` and `S-Enter` are sent
  natively.
- Answers to OSC 10/11 color queries with the theme colors, so Claude picks a matching light or
  dark theme.
- Running sessions shown in the statusline.

## Side by side git diff

Shows a file's HEAD version (left) next to its current contents (right), read-only. Changed lines
are aligned, with `╱` filler where one side has no line. Both sides get syntax highlighting, and
within a changed line, only the words that changed are highlighted. When the file is open with
unsaved changes, the right side shows the buffer and its title says `[modified]`.

### Usage

| Keys | Action |
| --- | --- |
| `space =` | Open the diff of the current file. |
| `C-g` in the changed files picker (`space g`) | Open the diff of the selected file on top of the picker: closing the diff goes back to the picker. `Enter` still opens the file. |

Inside the diff:

| Keys | Action |
| --- | --- |
| `n` / `N`, `]c` / `[c` | Next / previous hunk |
| `j` / `k`, `C-d` / `C-u`, `C-f` / `C-b`, `PageDown` / `PageUp`, mouse wheel | Scroll |
| `g` / `G` | Top / bottom |
| `h` / `l`, `0` | Scroll horizontally, back to the start of the line |
| `Enter` | Close the diff (and the picker it was opened from) and open the file at the current hunk |
| `q`, `Esc` | Close |

The footer shows the current hunk (e.g. `hunk 2/5`), and its line numbers are bold. New files
(untracked or staged) only have a right side, and deleted files only have a left side. Renamed files
are compared with their old path in HEAD.

### Limitations

- Binary files can't be shown.
- A line ending change (CRLF ↔ LF) marks every line as changed.
- The background tints need a theme with true colors. With other themes, only the changed words are
  marked (reversed).

### Ideas

- Search inside the diff.
- Fold long runs of unchanged lines.
- Stage or revert a hunk from the diff.

## Git commit log

A picker of the commits reachable from HEAD, newest first, with their short id, date, author and
summary. Choosing a commit lists the files it changed, and choosing a file opens the
[side by side diff](#side-by-side-git-diff) of that file in the commit: its version in the
commit's parent on the left, in the commit on the right.

### Usage

| Keys | Action |
| --- | --- |
| `space l`, `:git-log` | Open the commit picker. Typing filters by summary; `%commit`, `%author`, `%date` filter by the other columns. |
| `Enter` in the commit picker | List the files changed by the commit, on top of the commit picker. The status line shows the commit. |
| `Enter`, `C-g` in the files picker | Open the diff of the file, on top of the files picker. |
| `C-s` / `C-v` in the files picker | Open the file as it is now in a split, closing both pickers. |
| `q`, `Esc` in the diff | Back to the files picker. |
| `Esc` in the files picker | Back to the commit picker. |
| `Enter` in the diff | Open the file as it is now, at the current hunk, closing both pickers. |

The diff keys are the same as in the [side by side diff](#side-by-side-git-diff). Renames are
detected, so a renamed file is compared with its old path.

### Limitations

- Merge commits are compared with their first parent only.
- `Enter` in the diff opens the file as it is now in the working tree, so the line it jumps to may
  have moved since the commit. Files deleted since then can't be opened.
- Only the history of HEAD is listed, sorted by commit time; there's no graph.
- In a shallow clone, the oldest commit can't be shown: its parent isn't in the repository.

### Ideas

- List the commits that changed the current file.
- Show the commit message and stats in a preview.
- Pick another branch or a range.

## Staged files in the changed files picker

Upstream's changed files picker (`space g`) only lists unstaged changes, so files disappear from it
as soon as they are `git add`ed. In this fork it lists every change compared to HEAD, like
`git status`, staged or not:

- new files that are staged are listed as `+ added` (unstaged ones stay `+ untracked`);
- staged renames (`git mv`) are listed as renamed;
- a file that is staged and then changed again is listed once;
- a file that is added and then deleted from disk is not listed, since it matches HEAD again.

## File explorer side panel

A file tree in a panel on the left of the editor, rooted at the working directory. The editor area
shrinks to make room for it. Files and directories ignored by git (`.gitignore` files, including
nested ones and the ones of parent directories, `.git/info/exclude` and the global excludes file)
are shown dimmed instead of hidden. The `.git` directory is never listed.

Entries have file type icons (Nerd Font glyphs, colored by type), and the file of the current buffer
is shown in bold. While the panel isn't focused, it follows the current buffer: switching to another
file (a picker, `:open`, `gd`, ...) expands the tree down to it and selects it, unless the file is
inside a directory ignored by git (`target`, `node_modules`).

Not to be confused with upstream's `space e`, which opens the file explorer picker.

### Usage

| Keys | Action |
| --- | --- |
| `space E` | Open and focus the panel. With the panel open, focus it, or close it when it is already focused. |
| `j`/`k`, arrows | Move. `C-d`/`C-u` move half a page, `g`/`G` go to the top/bottom. |
| `l`, `Right` | Expand a directory (or move into an expanded one), open a file. |
| `h`, `Left` | Collapse a directory, or go to the parent directory. |
| `Enter`, mouse click | Expand/collapse a directory, open a file. |
| `a` | Create a file in the selected directory (or next to the selected file). Missing parent directories are created, and a name ending in `/` creates a directory. |
| `r` | Rename. The new name may contain `/` to move the entry to a subdirectory. Changing only the case of a name works on case-insensitive filesystems too. |
| `x`, then `p` | Cut the selected entry, then move it into the selected directory (or next to the selected file). |
| `d`, then `y` | Delete the selected file or directory (directories are deleted recursively). Any other key or a click cancels. |
| `R` | Read the whole tree from disk again. |
| `H` | Show/hide git ignored files for this session. |
| `?` | Show the keys in the status line. |
| `Esc`, `C-w l` | Give the focus back to the editor. From the leftmost view, `C-w h` focuses the panel again. |
| `q` | Close the panel. |

Keys that lead to a picker or a prompt keep working while the panel is focused: `:`, and in the
space menu (wherever it is mapped) the pickers, `space E`, global search, the command palette, the
Claude Code popup and the git diff view. `jump_view_right` (`C-w l`, `space w l`) goes back to the
editor. Other keys the panel doesn't use are ignored, and other
space menu entries (`space p`, `space c`, `space w`, ...) cancel the menu, so they can't edit the
buffer behind the panel. Terminal pastes are ignored too. A sticky menu entered before focusing the
panel keeps its keys until `Esc`. Clicking the panel in insert mode goes back to normal mode.

The panel gives the focus back to the editor as soon as the editor moves to another view, buffer or
selection: opening a file from the panel, a picker or `:open`, a jump within the same file (symbol
picker, `:42`), or clicking in the editor. Opened files show in the current view.

Renames, moves, creations and deletions go through the same code as `:move`, so language servers
are notified (`willRename`, `didCreate`, ...). Open buffers follow renamed and moved files,
including files inside a moved directory. Deleting a file closes its buffers, and deleting is
refused when one of them has unsaved changes. Moves and renames never overwrite an existing file.

The tree is updated from disk when the panel gets the focus and after each operation. Directory
listings of the expanded directories are kept: only the directories whose modification time changed
(or their `.gitignore`'s, with everything below it) are read again. `R` reads everything again. When the working directory
changes (`:cd`), the tree follows it on the next update.

### Configuration

```toml
[editor.explorer]
width = 30               # columns, at most half of the screen
hide-gitignored = false  # hide git ignored files instead of dimming them
icons = true             # file type icons; needs a Nerd Font, turn off otherwise
auto-reveal = true       # follow the current buffer while the panel isn't focused
```

Theme scopes: `ui.explorer` (background, defaults to `ui.background`), `ui.explorer.ignored`
(ignored entries, defaults to `ui.text.inactive`, or dim text), `ui.text.directory` (directories),
`ui.selection` (selected entry while focused), `ui.cursorline.primary` (selected entry while not
focused) and `ui.window` (separator).

### Limitations

- The tree doesn't watch the disk: files created outside the panel show up on the next update
  (focusing the panel or `R`).
- Changes to the global git excludes file or `.git/info/exclude`, and to a `.gitignore` of a
  directory above the working directory, are only picked up by `R`.
- One entry at a time: there is no multi-selection for moving or deleting several files.
- Deleting is permanent, there is no trash.
- Directories are read on the UI thread. That takes about 1 ms per 1,000 entries (see
  `cargo bench -p helix-term --features bench --bench explorer`), so only a directory with tens of
  thousands of entries, or a slow network filesystem, pauses the editor noticeably.
- Copying files isn't supported, only moving.
- Icons need a terminal font patched with [Nerd Fonts](https://www.nerdfonts.com/); without one
  they show as boxes, so set `icons = false`. Their colors are fixed, not taken from the theme, and
  file types without a known icon get a generic file icon.

### Ideas

- Copy and paste (`c`/`p`), and a multi-selection for batch moves and deletions.
- Git status markers (modified, added, untracked) next to the entries.
