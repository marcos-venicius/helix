# Claude Code integration: future improvements

Fork-only feature: `space i` / `:claude` (alias `:ai`) suspends the UI and runs `claude` in the
terminal with the current file, cursor line and selection as context. When `claude` exits, every
unmodified buffer is reloaded from disk.

## File watcher for files modified by Claude

Today buffers are only refreshed when `claude` exits (`reload_documents(editor, true)` in
`helix-term/src/commands/typed.rs`). A file watcher would make this automatic and would also
cover `claude` running in another terminal, a tmux popup or a split.

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
