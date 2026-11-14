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
| Git commit log | `helix-term/src/commands/git_log.rs`, `open_commit` in `helix-term/src/commands/diff_view.rs`, `for_each_commit` / `commit_changes` / `file_at_commit` in `helix-vcs/src/git.rs` and `helix-vcs/src/lib.rs`, `CommitInfo` in `helix-vcs/src/commit.rs` |
| Staged files in the changed files picker | `status()` and its helpers in `helix-vcs/src/git.rs`, `FileChange::Added` in `helix-vcs/src/status.rs` |
| File explorer side panel | `helix-term/src/ui/explorer.rs` and `helix-term/src/ui/explorer/` (icons, git ignore rules, git status markers); `changed_files` in `helix-vcs/src/lib.rs`; hooks in `EditorView` (`helix-term/src/ui/editor.rs`), `ExplorerConfig` in `helix-view/src/editor.rs`; benchmarks in `helix-term/benches/explorer.rs` |
| Markdown table formatting | `helix-term/src/commands/markdown_table.rs` |
| Binary files are not opened | `helix-view/src/binary.rs`; the check in `Editor::open` (`helix-view/src/editor.rs`), `DocumentOpenError::BinaryFile` in `helix-view/src/document.rs`, the startup file loop in `helix-term/src/application.rs` |
| Blacksite theme | `runtime/themes/blacksite.toml` |

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
  and the code table in [Fork features](#fork-features). A new feature, or a change to its main
  keys, also updates its card in [site/index.html](site/index.html). A small difference that has no
  screenshot (a behaviour change, a theme) gets a short text entry in the page's "Smaller changes"
  section instead of a card.

## Site

`site/index.html` is the fork's page, published to GitHub Pages
(`https://marcos-venicius.github.io/helix/`) by `.github/workflows/fork-site.yml` on pushes to
`marcos` that touch `site/`. The Pages source is "GitHub Actions": upstream's `gh-pages.yml` still
pushes the Helix book to the `gh-pages` branch on `master`, but that branch isn't served. The page
is a single HTML file with no build step, styled with the editor's `github_light` (light) and
`blacksite` (dark) palettes and JetBrains Mono from Google Fonts. It follows the system color scheme,
and a button in the header cycles system, light and dark (saved in `localStorage`, applied by an
inline script in the `<head>` so the page doesn't flash). It summarizes the features and links to
FORK.md for the details. The features with screenshots are cards in "What the fork adds"; above
them, "Smaller changes" lists the differences without a screenshot, as short text entries. Its
"Read this first" section says what the fork is (a personal build,
no stability or release guarantees, AI-assisted). Each feature has two screenshots in `site/screenshots/`,
`<name>-light.webp` (`github_light`) and `<name>-dark.webp` (`blacksite`), WebP 1600x1020, and the
page shows the pair matching its theme. They are `hx` in a 130x34 tmux pane (with `mouse` and
`focus-events` on, so Claude Code shows no tmux hints), the pane's `capture-pane -e` output turned
into HTML (JetBrainsMono Nerd Font Mono 20px, 28px rows, a window bar on top; the 16 ANSI colors
taken from the Alacritty palettes of the same themes) and photographed with headless Chrome. Scenes
that need git state (explorer markers, changed files) run in a scratch clone named `helix` on a
`marcos` branch. When a feature's look changes, retake both of its screenshots the same way.

Link previews and SEO live in the `<head>` of `site/index.html`: description, canonical URL, Open
Graph and Twitter Card tags and JSON-LD. They use absolute URLs under
`https://marcos-venicius.github.io/helix/`, so update them if the address changes. `site/og.jpg`
(1200x630, under 300 KB so WhatsApp shows it) is the preview image: logo, title, tagline, the feature
names and part of the commit diff screenshot, rendered from HTML with headless Chrome. When a
feature is added or renamed, update the description in the tags and re-render the image.

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

- `jump_view_left` in `helix-term/src/commands.rs` first calls `ui::explorer::jump_left`, which
  focuses the explorer from the leftmost view. Keep upstream's version and re-add that check.
- `helix-term/src/keymap/default.rs`, the `static_commands!` list in `helix-term/src/commands.rs`,
  `TYPABLE_COMMAND_LIST` in `helix-term/src/commands/typed.rs`: usually keep both sides.
- `helix-term/src/ui/picker.rs`: we add generic key handlers (a `key_handlers` field, the
  `with_key_handler` and `with_stacked_key_handler` builders, and a check at the top of the key
  match in `handle_event` that closes the picker or not). Keep upstream's version and re-add those
  pieces; `changed_file_picker` in `helix-term/src/commands.rs` uses the stacked one for `C-g`,
  the commit pickers in `commands/git_log.rs` for `Enter` and `C-g`.
- `changed_file_picker` in `helix-term/src/commands.rs`: its columns and `FileChangeData` are moved
  out into `file_change_columns`, shared with the commit files picker. If upstream changes the
  columns, port the change into `file_change_columns`.
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
  `cargo test -p helix-vcs --features git`, which covers the staged cases. `get_diff_base`'s
  checkout filtering is factored into `to_worktree`, shared with `file_at_commit`.
- `helix-vcs/src/lib.rs`, `helix-vcs/src/status.rs`: we add the commit log methods and
  `changed_files` (for the explorer) to `DiffProviderRegistry` and `DiffProvider`, and `#[derive(Clone)]` to `FileChange`. Keep both
  sides.
- `Editor::open` in `helix-view/src/editor.rs` calls `binary::is_binary_file` before
  `Document::open`, and `DocumentOpenError` has a `BinaryFile` variant. The startup loop in
  `Application::new` (`helix-term/src/application.rs`) skips binary files like irregular ones and
  reports them after the loop. Keep upstream's version and re-add those pieces.
- `helix-term/Cargo.toml`: we add a `bench` feature, `criterion` to the dev-dependencies and the
  `explorer` `[[bench]]`. Keep both sides.
- `Cargo.lock`: take upstream's version, then run `cargo build` to add our dependencies back
  (`alacritty_terminal`, `imara-diff` in `helix-term`).
- `book/src/generated/*.md`: don't resolve by hand; take either side and run `cargo xtask docgen`.

When adding a feature, follow the same pattern: put the code in new modules and keep changes to
upstream files to the minimum needed to wire it up. This keeps upstream merges cheap.

## Known issues

- The `helix-term` integration suite (`cargo test -p helix-term --features integration`) aborts with
  SIGABRT on this machine. Clean upstream `master` aborts the same way, so it is not caused by the
  fork's changes.
