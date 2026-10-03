# Claude Code integration: future improvements

Fork-only feature: Claude Code runs in an embedded terminal popup (`helix-term/src/ui/claude.rs`,
built on `alacritty_terminal`).

- `space i` / `:claude` (alias `:ai`) toggles the popup. The first time, it starts a session with the
  current file, line and selection as context. On later opens, it pastes `@path#Lx-y` into the prompt.
- `:claude <prompt>` starts a new session and sends the prompt as the first message.
- `space I` opens a picker with the running sessions and a "new session" entry.
- `C-\` hides the popup; the session keeps running in the background.

Every unmodified buffer is reloaded from disk when the popup is hidden or a session ends.

## File watcher for files modified by Claude

Today buffers are only refreshed when the popup is hidden or a session ends
(`reload_documents(editor, true)` in `helix-term/src/commands/typed.rs`). Sessions now keep running
in the background, so Claude can change files while you edit them. A file watcher would refresh
buffers as soon as that happens. It would also cover `claude` running in another terminal.

Implementation idea:

- Use the `notify` crate (with a debouncer) to watch the paths of the open documents. Add watches
  on document open and remove them on close. Alternatively, watch the workspace root and filter by
  open paths.
- Forward the events to the application event loop through a channel. Add a new branch in
  `Application::event_loop_until_idle`, or an `EditorEvent`.
- On a change event for a document:
  - if it has no unsaved changes, reload it with the same logic as `reload_documents` (sync
    views, notify the LSP through `file_event_handler.file_changed`, keep the cursor in view);
  - if it has unsaved changes, do not reload; show a warning ("file changed on disk, use :reload
    or :w!").
- Ignore events caused by Helix's own writes. Compare the mtime with `Document::last_saved_time`
  or skip events for a short window after a save.
- Make it opt-in through a config option, e.g. `editor.auto-reload = true`.

## Other ideas

- Kitty keyboard protocol in the embedded terminal, so keys like Shift-Enter are sent natively.
  Today Shift/Alt-Enter are mapped to `ESC CR`.
- Answer OSC 10/11 color queries with the theme colors, so Claude picks a matching light or dark
  theme.
- Text selection and copy with the mouse when the app does not track the mouse itself. Claude Code
  tracks it, so clicks and the wheel are forwarded to it.
- Show running sessions in the statusline.
