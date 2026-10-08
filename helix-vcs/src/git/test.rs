use std::{fs::File, io::Write, path::Path, process::Command};

use tempfile::TempDir;

use crate::git;

fn exec_git_cmd(args: &str, git_dir: &Path) {
    let res = Command::new("git")
        .arg("-C")
        .arg(git_dir) // execute the git command in this directory
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_AUTHOR_DATE", "2000-01-01 00:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_NAME", "author")
        .env("GIT_COMMITTER_DATE", "2000-01-02 00:00:00 +0000")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_NAME", "committer")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap_or_else(|_| panic!("`git {args}` failed"));
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args}` failed (see output above)")
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    if add_modified {
        exec_git_cmd("add -A", repo);
    }
    exec_git_cmd("commit -m message", repo);
}

fn empty_git_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir for git testing");
    exec_git_cmd("init", tmp.path());
    exec_git_cmd("config user.email test@helix.org", tmp.path());
    exec_git_cmd("config user.name helix-test", tmp.path());
    tmp
}

#[test]
fn missing_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"foo").unwrap();

    assert!(git::get_diff_base(&file, true).is_err());
}

#[test]
fn unmodified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

#[test]
fn modified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"bar").unwrap();

    assert_eq!(
        git::get_diff_base(&file, true).unwrap(),
        Vec::from(contents)
    );
}

/// Test that `get_file_head` does not return content for a directory.
/// This is important to correctly cover cases where a directory is removed and replaced by a file.
/// If the contents of the directory object were returned a diff between a path and the directory children would be produced.
#[test]
fn directory() {
    let temp_git = empty_git_repo();
    let dir = temp_git.path().join("file.txt");
    std::fs::create_dir(&dir).expect("");
    let file = dir.join("file.txt");
    let contents = b"foo".as_slice();
    File::create(file).unwrap().write_all(contents).unwrap();

    create_commit(temp_git.path(), true);

    std::fs::remove_dir_all(&dir).unwrap();
    File::create(&dir).unwrap().write_all(b"bar").unwrap();
    assert!(git::get_diff_base(&dir, true).is_err());
}

/// Test that `get_diff_base` resolves symlinks so that the same diff base is
/// used as the target file.
///
/// This is important to correctly cover cases where a symlink is removed and
/// replaced by a file. If the contents of the symlink object were returned
/// a diff between a literal file path and the actual file content would be
/// produced (bad ui).
#[cfg(any(unix, windows))]
#[test]
fn symlink() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    let file_link = temp_git.path().join("file_link.txt");

    symlink("file.txt", &file_link).unwrap();
    create_commit(temp_git.path(), true);

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

/// Test that `get_diff_base` returns content when the file is a symlink to
/// another file that is in a git repo, but the symlink itself is not.
#[cfg(any(unix, windows))]
#[test]
fn symlink_to_git_repo() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let temp_git = empty_git_repo();

    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    create_commit(temp_git.path(), true);

    let file_link = temp_dir.path().join("file_link.txt");
    symlink(&file, &file_link).unwrap();

    assert_eq!(git::get_diff_base(&file_link, true).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file, true).unwrap(), contents);
}

/// The changed files of `repo`, as `(kind, path relative to the repo)`, sorted by path.
fn changed_files(repo: &Path) -> Vec<(&'static str, String)> {
    use crate::FileChange;
    use std::sync::Mutex;

    let changes = Mutex::new(Vec::new());
    git::for_each_changed_file(repo, true, |change| {
        let relative = |path: &Path| {
            let repo = repo.canonicalize().unwrap();
            path.canonicalize()
                .unwrap_or_else(|_| path.to_path_buf())
                .strip_prefix(&repo)
                .unwrap_or(path)
                .display()
                .to_string()
        };
        let entry = match change.unwrap() {
            FileChange::Untracked { path } => ("untracked", relative(&path)),
            FileChange::Added { path } => ("added", relative(&path)),
            FileChange::Modified { path } => ("modified", relative(&path)),
            FileChange::Conflict { path } => ("conflict", relative(&path)),
            FileChange::Deleted { path } => (
                "deleted",
                path.file_name().unwrap().to_string_lossy().into(),
            ),
            FileChange::Renamed { from_path, to_path } => (
                "renamed",
                format!(
                    "{} -> {}",
                    from_path.file_name().unwrap().to_string_lossy(),
                    relative(&to_path)
                ),
            ),
        };
        changes.lock().unwrap().push(entry);
        true
    })
    .unwrap();
    let mut changes = changes.into_inner().unwrap();
    changes.sort_by(|a, b| a.1.cmp(&b.1));
    changes
}

fn write(repo: &Path, file: &str, contents: &str) {
    File::create(repo.join(file))
        .unwrap()
        .write_all(contents.as_bytes())
        .unwrap();
}

/// A repository with `a.txt` and `b.txt` committed.
fn committed_repo() -> TempDir {
    let temp_git = empty_git_repo();
    write(temp_git.path(), "a.txt", "a\n");
    write(temp_git.path(), "b.txt", "b\nb\nb\nb\n");
    create_commit(temp_git.path(), true);
    temp_git
}

#[test]
fn status_lists_unstaged_changes() {
    let temp_git = committed_repo();
    let repo = temp_git.path();
    write(repo, "a.txt", "changed\n");
    write(repo, "new.txt", "new\n");
    assert_eq!(
        changed_files(repo),
        [
            ("modified", "a.txt".into()),
            ("untracked", "new.txt".into())
        ]
    );
}

#[test]
fn status_lists_staged_changes() {
    let temp_git = committed_repo();
    let repo = temp_git.path();
    write(repo, "a.txt", "changed\n");
    write(repo, "new.txt", "new\n");
    exec_git_cmd("add -A", repo);
    exec_git_cmd("rm -q b.txt", repo);
    assert_eq!(
        changed_files(repo),
        [
            ("modified", "a.txt".into()),
            ("deleted", "b.txt".into()),
            ("added", "new.txt".into()),
        ]
    );
}

#[test]
fn status_merges_staged_and_unstaged_changes() {
    let temp_git = committed_repo();
    let repo = temp_git.path();
    // staged, then changed again: listed once
    write(repo, "a.txt", "staged\n");
    exec_git_cmd("add a.txt", repo);
    write(repo, "a.txt", "staged and changed again\n");
    // added, then changed again: still new compared to HEAD
    write(repo, "new.txt", "new\n");
    exec_git_cmd("add new.txt", repo);
    write(repo, "new.txt", "new, changed\n");
    // added, then deleted: not in HEAD either
    write(repo, "gone.txt", "gone\n");
    exec_git_cmd("add gone.txt", repo);
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    assert_eq!(
        changed_files(repo),
        [("modified", "a.txt".into()), ("added", "new.txt".into())]
    );
}

#[test]
fn status_lists_staged_renames() {
    let temp_git = committed_repo();
    let repo = temp_git.path();
    exec_git_cmd("mv b.txt c.txt", repo);
    assert_eq!(changed_files(repo), [("renamed", "b.txt -> c.txt".into())]);
}

fn head_id(repo: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit_changes(repo: &Path, id: &str) -> Vec<(&'static str, String)> {
    use crate::FileChange;

    let name = |path: &Path| path.file_name().unwrap().to_string_lossy().to_string();
    git::commit_changes(repo, true, id)
        .unwrap()
        .into_iter()
        .map(|change| match change {
            FileChange::Added { path } => ("added", name(&path)),
            FileChange::Modified { path } => ("modified", name(&path)),
            FileChange::Deleted { path } => ("deleted", name(&path)),
            FileChange::Renamed { from_path, to_path } => (
                "renamed",
                format!("{} -> {}", name(&from_path), name(&to_path)),
            ),
            _ => unreachable!(),
        })
        .collect()
}

#[test]
fn commit_log_lists_changes_and_contents() {
    let temp_git = committed_repo();
    let repo = temp_git.path();
    let root = head_id(repo);

    write(repo, "a.txt", "changed\n");
    exec_git_cmd("mv b.txt c.txt", repo);
    write(repo, "new.txt", "new\n");
    exec_git_cmd("add -A", repo);
    exec_git_cmd("commit -m second", repo);
    let second = head_id(repo);

    exec_git_cmd("rm -q new.txt", repo);
    exec_git_cmd("commit -m third", repo);
    let third = head_id(repo);

    assert_eq!(
        commit_changes(repo, &root),
        [("added", "a.txt".into()), ("added", "b.txt".into())]
    );
    assert_eq!(
        commit_changes(repo, &second),
        [
            ("modified", "a.txt".into()),
            ("renamed", "b.txt -> c.txt".into()),
            ("added", "new.txt".into()),
        ]
    );
    assert_eq!(
        commit_changes(repo, &third),
        [("deleted", "new.txt".into())]
    );

    let a = repo.join("a.txt");
    let at = |id: &str, file: &Path| git::file_at_commit(repo, true, id, file).unwrap();
    assert_eq!(at(&second, &a), Some(b"changed\n".to_vec()));
    assert_eq!(at(&format!("{second}^"), &a), Some(b"a\n".to_vec()));
    assert_eq!(at(&format!("{root}^"), &a), None);
    assert_eq!(at(&third, &repo.join("new.txt")), None);

    let summaries = std::sync::Mutex::new(Vec::new());
    git::for_each_commit(repo, true, |commit| {
        let commit = commit.unwrap();
        assert_eq!(commit.date, "2000-01-01");
        summaries.lock().unwrap().push(commit.summary);
        true
    })
    .unwrap();
    let mut summaries = summaries.into_inner().unwrap();
    summaries.sort();
    assert_eq!(summaries, ["message", "second", "third"]);
}
