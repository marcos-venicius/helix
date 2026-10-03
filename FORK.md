# Fork features

This fork ([marcos-venicius/helix](https://github.com/marcos-venicius/helix)) is upstream
[Helix](https://github.com/helix-editor/helix) plus the features below. They are not part of
upstream, so please report issues with them here. Everything else works and is documented exactly
as in the [Helix documentation](https://docs.helix-editor.com/).

- [Claude Code popup](#claude-code-popup)
- [Side by side git diff](#side-by-side-git-diff)
- [Staged files in the changed files picker](#staged-files-in-the-changed-files-picker)

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
| `C-g` in the changed files picker (`space g`) | Open the diff of the selected file. `Enter` still opens the file. |

Inside the diff:

| Keys | Action |
| --- | --- |
| `n` / `N`, `]c` / `[c` | Next / previous hunk |
| `j` / `k`, `C-d` / `C-u`, `C-f` / `C-b`, `PageDown` / `PageUp`, mouse wheel | Scroll |
| `g` / `G` | Top / bottom |
| `h` / `l`, `0` | Scroll horizontally, back to the start of the line |
| `Enter` | Close the diff and open the file at the current hunk |
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

## Staged files in the changed files picker

Upstream's changed files picker (`space g`) only lists unstaged changes, so files disappear from it
as soon as they are `git add`ed. In this fork it lists every change compared to HEAD, like
`git status`, staged or not:

- new files that are staged are listed as `+ added` (unstaged ones stay `+ untracked`);
- staged renames (`git mv`) are listed as renamed;
- a file that is staged and then changed again is listed once;
- a file that is added and then deleted from disk is not listed, since it matches HEAD again.
