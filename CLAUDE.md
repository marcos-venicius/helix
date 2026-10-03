# Helix fork (marcos-venicius/helix)

This repository is a personal fork of [helix-editor/helix](https://github.com/helix-editor/helix).
It carries features that won't be accepted upstream (for example the Claude Code popup, see
`docs/claude-code-improvements.md`), while staying in sync with upstream fixes and improvements.

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

Our features keep upstream files mostly untouched: the logic lives in new files
(`helix-term/src/ui/claude.rs`, `helix-term/src/commands/claude.rs`), and upstream files only get
small hooks. When upstream touches the same spots:

- `helix-term/src/keymap/default.rs`, the `static_commands!` list in `helix-term/src/commands.rs`,
  `TYPABLE_COMMAND_LIST` in `helix-term/src/commands/typed.rs`: usually keep both sides.
- `Cargo.lock`: take upstream's version, then run `cargo build` to add our dependencies back
  (`alacritty_terminal`).
- `book/src/generated/*.md`: don't resolve by hand; take either side and run `cargo xtask docgen`.

When adding a feature, follow the same pattern: put the code in new modules and keep changes to
upstream files to the minimum needed to wire it up. This keeps upstream merges cheap.

## Known issues

- The `helix-term` integration suite (`cargo test -p helix-term --features integration`) aborts with
  SIGABRT on this machine. Clean upstream `master` aborts the same way, so it is not caused by the
  fork's changes.
