# Helix fork (marcos-venicius/helix)

This repository is a personal fork of [helix-editor/helix](https://github.com/helix-editor/helix).
It carries features that won't be accepted upstream, while staying in sync with upstream fixes and
improvements.

## Fork features

What each feature does and how to use it is documented for users in [FORK.md](FORK.md). Where the
code lives:

| Feature | Code |
| --- | --- |
| Claude Code popup | `helix-term/src/ui/claude.rs`, `helix-term/src/commands/claude.rs` |
| Side by side git diff | `helix-term/src/ui/diff_view.rs`, `helix-term/src/commands/diff_view.rs` |
| Staged files in the changed files picker | `status()` and its helpers in `helix-vcs/src/git.rs`, `FileChange::Added` in `helix-vcs/src/status.rs` |
| File explorer side panel | `helix-term/src/ui/explorer.rs`; hooks in `EditorView` (`helix-term/src/ui/editor.rs`), `ExplorerConfig` in `helix-view/src/editor.rs` |

## Branches

| Branch | Role |
| --- | --- |
| `master` | Exact mirror of `upstream/master`. **Never commit here.** It only receives fast-forwards from upstream. |
| `marcos` | The fork's own version: upstream plus our features. Default branch of the fork on GitHub, and what gets built and used. |
| `feature/*`, `fix/*` | Short-lived branches created from `marcos`, merged into it through PRs. |

Remotes:

- `origin`: `git@github.com:marcos-venicius/helix.git` (the fork)
- `upstream`: `https://github.com/helix-editor/helix.git` (official, read-only for us)

## Rules

- Create new work from an up-to-date `marcos`: `git checkout marcos && git pull && git checkout -b feature/<name>`.
- PRs always target `marcos` on the fork, never `master` and never upstream. `gh` is configured with
  `gh repo set-default marcos-venicius/helix`, and `marcos` is the fork's default branch. Still, be
  explicit: `gh pr create --repo marcos-venicius/helix --base marcos`.
- To contribute a fix upstream, branch from `master` (not `marcos`) so none of the fork's features
  leak into it.
- Keep `marcos` in sync with upstream (below) before starting new features and whenever upstream has
  news.
- Every PR that adds or changes a fork feature updates [FORK.md](FORK.md) (usage, limitations, ideas)
  and the code table in [Fork features](#fork-features).

## Syncing with upstream

```sh
git fetch upstream
git checkout master
git merge --ff-only upstream/master   # must fast-forward; if it doesn't, something was committed to master
git push origin master
git checkout marcos
git merge master                      # merge, never rebase: marcos is published and shared by PRs
git push origin marcos
```

`git rerere` is enabled (`git config rerere.enabled true`), so a conflict resolution is recorded and
reapplied automatically if the same conflict shows up again.

After merging, verify the result:

```sh
cargo build
cargo clippy --all-targets
cargo test -p helix-term --lib
cargo xtask docgen                    # regenerates book/src/generated/*.md
```

### Expected conflicts

Our features keep upstream files mostly untouched: the logic lives in new files (see
[Fork features](#fork-features)), and upstream files only get small hooks. When upstream touches
the same spots:

- `helix-term/src/keymap/default.rs`, the `static_commands!` list in `helix-term/src/commands.rs`,
  `TYPABLE_COMMAND_LIST` in `helix-term/src/commands/typed.rs`: usually keep both sides.
- `helix-term/src/ui/picker.rs`: we add a generic `with_key_handler` (a `key_handlers` field, its
  builder and a check at the top of the key match in `handle_event`). Keep upstream's version and
  re-add those pieces; `changed_file_picker` in `helix-term/src/commands.rs` uses it for `C-g`.
- `helix-term/src/ui/editor.rs`: `EditorView` has an `explorer` field. `render` clips the
  explorer's width off the editor area before `cx.editor.resize`, calls `explorer.update` before
  drawing the views (unfocusing them while the explorer has the focus) and renders it after them.
  `handle_event` drops pastes while it's focused, offers it each key first (`explorer_key`, before
  `on_next_key`, with `&mut self.keymaps` so it can cancel pending keys) and hands it the mouse
  events over its area (after `handle_non_key_input`). `cursor` hides the cursor while it's
  focused. Keep upstream's version and re-add those pieces.
- `helix-vcs/src/git.rs`: `status()` is rewritten to also list staged changes (`into_iter` instead of
  `into_index_worktree_iter`, merging both kinds of change per file), and `FileChange` has an extra
  `Added` variant. If upstream changes `status()`, port their change onto our version and run
  `cargo test -p helix-vcs --features git`, which covers the staged cases.
- `Cargo.lock`: take upstream's version, then run `cargo build` to add our dependencies back
  (`alacritty_terminal`, `imara-diff` in `helix-term`).
- `book/src/generated/*.md`: don't resolve by hand; take either side and run `cargo xtask docgen`.

When adding a feature, follow the same pattern: put the code in new modules and keep changes to
upstream files to the minimum needed to wire it up. This keeps upstream merges cheap.

## Known issues

- The `helix-term` integration suite (`cargo test -p helix-term --features integration`) aborts with
  SIGABRT on this machine. Clean upstream `master` aborts the same way, so it is not caused by the
  fork's changes.
