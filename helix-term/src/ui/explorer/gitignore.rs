//! Git's ignore rules for the explorer. Each directory's rules are read once and kept with its
//! listing, instead of listing the directory again through `ignore::WalkBuilder`, which also reads
//! the `.gitignore` files of every parent directory and the global excludes file each time.
//!
//! Like git (and the `ignore` crate): the `.gitignore` of the deepest directory that has a say
//! wins, up to the root of the repository, then `.git/info/exclude`, then the global excludes file.
//! Outside of a repository nothing is ignored.

use std::path::Path;
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The ignore rules of one directory.
#[derive(Default)]
pub struct DirRules {
    gitignore: Option<Gitignore>,
    /// `.git/info/exclude`, when the directory is the root of a repository.
    exclude: Option<Gitignore>,
    /// Whether the directory is the root of a repository, where the parents' rules stop applying.
    repo: bool,
}

impl DirRules {
    pub fn read(dir: &Path) -> Arc<Self> {
        let git = dir.join(".git");
        let repo = git.exists();
        Arc::new(Self {
            gitignore: parse(dir, &dir.join(".gitignore")),
            exclude: repo
                .then(|| parse(dir, &git.join("info/exclude")))
                .flatten(),
            repo,
        })
    }
}

/// Rules from outside the tree: the directories above its root, up to the root of the repository,
/// and the global excludes file. Read when the panel opens, on `R` and when the root changes.
pub struct Outer {
    /// Deepest first.
    above: Vec<Arc<DirRules>>,
    global: Option<Gitignore>,
}

impl Outer {
    pub fn read(root: &Path) -> Self {
        let mut above = Vec::new();
        let mut repo = root.join(".git").exists().then_some(root);
        if repo.is_none() {
            for dir in root.ancestors().skip(1) {
                let rules = DirRules::read(dir);
                let is_repo = rules.repo;
                above.push(rules);
                if is_repo {
                    repo = Some(dir);
                    break;
                }
            }
        }
        let global = repo.and_then(|repo| {
            let (global, _) = GitignoreBuilder::new(repo).build_global();
            (!global.is_empty()).then_some(global)
        });
        Self { above, global }
    }
}

fn parse(root: &Path, file: &Path) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    // A missing file is an error here and leaves the rules empty, without another stat.
    builder.add(file);
    builder.build().ok().filter(|rules| !rules.is_empty())
}

/// Tells which entries of a directory git ignores.
pub struct Matcher<'a> {
    /// Most important first. Empty outside of a repository.
    rules: Vec<&'a Gitignore>,
}

impl<'a> Matcher<'a> {
    /// `chain` holds the rules of the directory and of its parents up to the root of the tree,
    /// deepest first.
    pub fn new(chain: &'a [Arc<DirRules>], outer: &'a Outer) -> Self {
        let mut rules = Vec::new();
        let mut repo = None;
        for dir in chain.iter().chain(&outer.above) {
            rules.extend(&dir.gitignore);
            if dir.repo {
                repo = Some(dir);
                break;
            }
        }
        let Some(repo) = repo else {
            return Self { rules: Vec::new() };
        };
        rules.extend(&repo.exclude);
        rules.extend(&outer.global);
        Self { rules }
    }

    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        self.rules
            .iter()
            .map(|rules| rules.matched(path, is_dir))
            .find(|matched| !matched.is_none())
            .is_some_and(|matched| matched.is_ignore())
    }
}
