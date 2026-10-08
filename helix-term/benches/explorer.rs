//! Benchmarks for the file explorer panel: the directory reads it does on the UI thread.
//!
//! cargo bench -p helix-term --features bench --bench explorer

use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use helix_term::ui::explorer::bench;

/// Backdates the modification time of `dir`, so the panel trusts its listings: it never trusts
/// one made within 2 seconds of a change.
fn backdate(dir: &Path) {
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
    open_dir(dir).set_modified(an_hour_ago).unwrap();
}

/// Opens `dir` so that its modification time can be changed. Windows only opens a directory
/// with `FILE_FLAG_BACKUP_SEMANTICS`, and needs write access to change its times.
fn open_dir(dir: &Path) -> std::fs::File {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options.write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    #[cfg(not(windows))]
    options.read(true);
    options.open(dir).unwrap()
}

/// A git repository, with a `.gitignore` at the root.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".gitignore"), "target/\n*.log\n").unwrap();
    dir
}

fn files(dir: &Path, count: usize) {
    fs::create_dir_all(dir).unwrap();
    for i in 0..count {
        let name = if i % 10 == 0 { "log" } else { "rs" };
        fs::write(dir.join(format!("file{i}.{name}")), "").unwrap();
    }
}

/// A directory `depth` levels deep, each level with its own `.gitignore` and a few files.
fn deep(root: &Path, depth: usize) -> PathBuf {
    let mut dir = root.to_path_buf();
    for level in 0..depth {
        dir.push(format!("level{level}"));
        files(&dir, 10);
        fs::write(dir.join(".gitignore"), format!("ignored{level}\n")).unwrap();
    }
    dir
}

fn list_dir(c: &mut Criterion) {
    let mut group = c.benchmark_group("list_dir");
    for count in [100, 1_000, 10_000] {
        let repo = repo();
        let dir = repo.path().join("flat");
        files(&dir, count);
        let mut tree = bench::Tree::new(repo.path(), &[]);
        group.bench_with_input(BenchmarkId::new("flat", count), &dir, |b, dir| {
            b.iter(|| tree.list(black_box(dir)))
        });
    }
    // Every level above has its own `.gitignore`.
    let repo = repo();
    let dir = deep(repo.path(), 10);
    let parents: Vec<PathBuf> = dir
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != repo.path())
        .map(Path::to_path_buf)
        .collect();
    let mut tree = bench::Tree::new(repo.path(), &parents);
    group.bench_function("depth_10", |b| b.iter(|| tree.list(black_box(&dir))));
    group.finish();
}

/// A tree of `count` expanded directories with 20 files each.
fn wide(root: &Path, count: usize) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = (0..count)
        .map(|i| root.join(format!("dir{i:03}")))
        .collect();
    for dir in &dirs {
        files(dir, 20);
        backdate(dir);
    }
    backdate(root);
    dirs
}

fn refresh(c: &mut Criterion) {
    let mut group = c.benchmark_group("refresh");
    for count in [10, 100] {
        let repo = repo();
        let expanded = wide(repo.path(), count);
        let mut tree = bench::Tree::new(repo.path(), &expanded);
        // Nothing changed on disk: what focusing the panel costs.
        group.bench_function(BenchmarkId::new("unchanged", count), |b| {
            b.iter(|| tree.refresh())
        });
        // Everything read again: `R`, or opening the panel with these directories expanded.
        group.bench_function(BenchmarkId::new("reload", count), |b| {
            b.iter(|| tree.reload())
        });
    }
    group.finish();
}

fn reveal(c: &mut Criterion) {
    let repo = repo();
    let dir = deep(repo.path(), 10);
    let file = dir.join("file1.rs");
    // Auto-reveal of a file 10 levels deep, from a collapsed tree.
    c.bench_function("reveal/depth_10", |b| {
        b.iter(|| bench::reveal(repo.path(), black_box(&file)))
    });
}

criterion_group!(benches, list_dir, refresh, reveal);
criterion_main!(benches);
