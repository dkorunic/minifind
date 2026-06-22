// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Custom parallel filesystem walker: a `crossbeam-deque` work-stealing
//! engine over a `cfg`-split leaf (`unix`/`fallback`).

use crate::args::Args;
use crate::filetype::EntryType;
use crate::meta::Meta;
use crate::ratelimit::Limiter;
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use crossbeam_utils::Backoff;
use globset::GlobSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[cfg(unix)]
#[path = "walk/unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "walk/fallback.rs"]
mod platform;

/// Whether traversal should continue or stop entirely.
pub enum WalkState {
    Continue,
    Quit,
}

/// A matched filesystem entry handed to the visitor.
pub struct Entry {
    pub path: PathBuf,
    pub file_type: EntryType,
    /// Distance from the starting path: 0 for a command-line root, 1 for its
    /// children, and so on (used by the `--min-depth` gate in the visitor).
    pub depth: usize,
}

impl Entry {
    /// Final path component (used for `--name` glob matching); falls back to
    /// the whole path for roots like `/`.
    #[inline]
    pub fn file_name(&self) -> &OsStr {
        self.path.file_name().unwrap_or_else(|| self.path.as_os_str())
    }
}

/// A directory-descent unit. The queue carries *only* directories (and
/// followed symlink-dirs); non-directories are emitted inline by their
/// parent's read loop and never become tasks.
struct Task {
    path: PathBuf,
    // Parent directory fd, shared as the `openat`/`statat` anchor for this
    // child; `None` only for command-line roots (opened by absolute path).
    parent: Option<Arc<platform::DirFd>>,
    // Whether to resolve a final symlink when opening (roots + followed
    // symlink-dirs); real subdirectories use `O_NOFOLLOW`.
    follow: bool,
    depth: usize,
    root_dev: u64,
    // (dev, ino) of every ancestor directory; Some only when following
    // symlinks, so the common path stays allocation-free.
    ancestors: Option<Arc<Vec<(u64, u64)>>>,
}

/// A lazy, leaf-relative metadata fetch handed to the visitor with each entry.
/// [`fetch`](StatAt::fetch) runs only when a predicate is active, so the
/// `statx` is paid after the cheaper filters cull — relative to the parent fd.
#[derive(Clone, Copy)]
pub struct StatAt<'a> {
    src: StatSrc<'a>,
    follow: bool,
}

#[derive(Clone, Copy)]
enum StatSrc<'a> {
    Child { dir: &'a Arc<platform::DirFd>, name: &'a OsStr },
    Root { path: &'a Path },
}

impl<'a> StatAt<'a> {
    fn child(
        dir: &'a Arc<platform::DirFd>,
        name: &'a OsStr,
        follow: bool,
    ) -> Self {
        StatAt { src: StatSrc::Child { dir, name }, follow }
    }

    fn root(path: &'a Path, follow: bool) -> Self {
        StatAt { src: StatSrc::Root { path }, follow }
    }

    /// Fetches the metadata fields selected by `mask` (see [`crate::meta`]).
    pub fn fetch(&self, mask: u32) -> std::io::Result<Meta> {
        match self.src {
            StatSrc::Child { dir, name } => {
                platform::stat_at(dir, name, self.follow, mask)
            }
            StatSrc::Root { path } => {
                platform::stat_root(path, self.follow, mask)
            }
        }
    }

    /// `faccessat` for the `meta::access` mode bits (`-readable`/…).
    pub fn access(&self, mode: u8) -> bool {
        match self.src {
            StatSrc::Child { dir, name } => {
                platform::access_at(dir, name, mode)
            }
            StatSrc::Root { path } => platform::access_root(path, mode),
        }
    }

    /// The symlink target (for `-lname`); `None` if not a symlink / unreadable.
    pub fn readlink(&self) -> Option<std::ffi::OsString> {
        match self.src {
            StatSrc::Child { dir, name } => platform::readlink_at(dir, name),
            StatSrc::Root { path } => platform::readlink_root(path),
        }
    }
}

/// Immutable shared state for one `walk_parallel` run, bundled so the
/// recursive worker functions keep small signatures.
struct WalkCtx<'a> {
    args: &'a Args,
    pending: &'a AtomicUsize,
    quit: &'a AtomicBool,
    limiter: Option<&'a Limiter>,
    // matched against each child's file name; a matched dir is pruned
    exclude: Option<&'a GlobSet>,
}

/// Walks `roots` in parallel, invoking a fresh per-thread visitor (from
/// `make_visitor`) for every entry. Directory read/open errors are skipped.
pub fn walk_parallel<F, V>(
    args: &Args,
    roots: &[&Path],
    limiter: Option<&Limiter>,
    exclude: Option<&GlobSet>,
    make_visitor: F,
) where
    F: Fn() -> V + Sync,
    V: FnMut(Entry, &StatAt) -> WalkState + Send,
{
    let n_workers = (args.threads - 1).max(1);
    let injector = Injector::new();
    // outstanding tasks (pushed but not yet fully processed); reaching 0 is
    // the termination signal. `quit` is the early-stop signal (Quit visitor).
    let pending = AtomicUsize::new(0);
    let quit = AtomicBool::new(false);

    for root in roots {
        let Ok((dev, _ino)) = platform::path_id(root) else {
            continue;
        };
        // Ancestors hold the parent chain only; `descend` appends each
        // directory's own id before recursing, so a root starts empty.
        let ancestors = args.follow_symlinks.then(|| Arc::new(Vec::new()));
        pending.fetch_add(1, Ordering::SeqCst);
        injector.push(Task {
            path: root.to_path_buf(),
            parent: None,
            // a root that is a symlink-to-dir is followed, like find(1).
            follow: true,
            depth: 0,
            root_dev: dev,
            ancestors,
        });
    }

    let workers: Vec<Worker<Task>> =
        (0..n_workers).map(|_| Worker::new_lifo()).collect();
    let stealers: Vec<Stealer<Task>> =
        workers.iter().map(Worker::stealer).collect();

    let ctx =
        WalkCtx { args, pending: &pending, quit: &quit, limiter, exclude };

    thread::scope(|scope| {
        for worker in workers {
            let ctx = &ctx;
            let injector = &injector;
            let stealers = &stealers;
            let make_visitor = &make_visitor;
            scope.spawn(move || {
                let mut visitor = make_visitor();
                run_worker(ctx, &worker, injector, stealers, &mut visitor);
            });
        }
    });
}

fn run_worker<V: FnMut(Entry, &StatAt) -> WalkState>(
    ctx: &WalkCtx,
    local: &Worker<Task>,
    injector: &Injector<Task>,
    stealers: &[Stealer<Task>],
    visitor: &mut V,
) {
    let backoff = Backoff::new();
    loop {
        if ctx.quit.load(Ordering::Relaxed) {
            return;
        }
        match find_task(local, injector, stealers) {
            Some(task) => {
                backoff.reset();
                process(ctx, task, local, visitor);
                ctx.pending.fetch_sub(1, Ordering::SeqCst);
            }
            None => {
                if ctx.pending.load(Ordering::SeqCst) == 0 {
                    return;
                }
                backoff.snooze();
            }
        }
    }
}

fn find_task(
    local: &Worker<Task>,
    injector: &Injector<Task>,
    stealers: &[Stealer<Task>],
) -> Option<Task> {
    local.pop().or_else(|| {
        std::iter::repeat_with(|| {
            injector
                .steal_batch_and_pop(local)
                .or_else(|| stealers.iter().map(Stealer::steal).collect())
        })
        .find(|s| !s.is_retry())
        .and_then(Steal::success)
    })
}

/// Whether an entry of type `ty` is descended into (and so enqueued as a
/// task): real directories always, symlinks only under `--follow`.
fn descends_into(ty: EntryType, args: &Args) -> bool {
    match ty {
        EntryType::Dir => true,
        EntryType::Symlink => args.follow_symlinks,
        _ => false,
    }
}

fn process<V: FnMut(Entry, &StatAt) -> WalkState>(
    ctx: &WalkCtx,
    task: Task,
    local: &Worker<Task>,
    visitor: &mut V,
) {
    // children are emitted by their parent's read loop; a root has none, so it
    // self-emits here (as Dir — a non-dir root just fails to open below)
    if task.parent.is_none() {
        let stat = StatAt::root(&task.path, ctx.args.follow_symlinks);
        if let WalkState::Quit = visitor(
            Entry {
                path: task.path.clone(),
                file_type: EntryType::Dir,
                depth: task.depth,
            },
            &stat,
        ) {
            ctx.quit.store(true, Ordering::Relaxed);
            return;
        }
    }
    descend(ctx, &task, local, visitor);
}

fn descend<V: FnMut(Entry, &StatAt) -> WalkState>(
    ctx: &WalkCtx,
    task: &Task,
    local: &Worker<Task>,
    visitor: &mut V,
) {
    // At depth == max we still emit (the parent did) but never read children.
    if let Some(max) = ctx.args.max_depth {
        if task.depth >= max {
            return;
        }
    }

    // throttle one token per directory visited; abort on shutdown
    if let Some(limiter) = ctx.limiter {
        if !limiter.acquire(ctx.quit) {
            return;
        }
    }

    // Roots open by absolute path; children open relative to the parent fd.
    let opened = match &task.parent {
        None => platform::open_root(&task.path, task.follow),
        Some(parent) => {
            let leaf =
                task.path.file_name().unwrap_or_else(|| task.path.as_os_str());
            platform::open_child(parent, leaf, &task.path, task.follow)
        }
    };
    let Ok(fd) = opened else {
        return;
    };
    // anchors this dir's children; refcounting frees the fd once its last
    // still-queued subdir is opened
    let dir = Arc::new(fd);
    let Ok((dev, ino)) = platform::dir_id(&dir) else {
        return;
    };
    if ctx.args.one_filesystem && dev != task.root_dev {
        return;
    }
    if let Some(anc) = &task.ancestors {
        if anc.contains(&(dev, ino)) {
            return; // symlink cycle
        }
    }
    let child_ancestors = task.ancestors.as_ref().map(|a| {
        let mut v = (**a).clone();
        v.push((dev, ino));
        Arc::new(v)
    });
    let child_depth = task.depth + 1;
    // skip subdir tasks that can't read (depth >= max); they'd only pin the fd
    let enqueue_children =
        ctx.args.max_depth.is_none_or(|max| child_depth < max);

    // emit every entry inline; enqueue a descend task only for dirs / followed
    // symlink-dirs
    let follow = ctx.args.follow_symlinks;
    let _ = platform::for_each_entry(&dir, &task.path, |path, leaf, ty| {
        // --exclude: skip the entry; a matched dir prunes the subtree (no
        // task → no opendir). Roots never reach here, so are always kept.
        if let Some(ex) = ctx.exclude {
            if ex.is_match(leaf) {
                return true;
            }
        }
        // compute before `path` moves into Entry; only descenders clone it
        let descend_path = (enqueue_children && descends_into(ty, ctx.args))
            .then(|| path.clone());
        // `leaf` anchors the lazy statx on the parent fd
        let stat = StatAt::child(&dir, leaf, follow);
        if let WalkState::Quit =
            visitor(Entry { path, file_type: ty, depth: child_depth }, &stat)
        {
            ctx.quit.store(true, Ordering::Relaxed);
            return false;
        }
        if let Some(child_path) = descend_path {
            ctx.pending.fetch_add(1, Ordering::SeqCst);
            local.push(Task {
                path: child_path,
                parent: Some(Arc::clone(&dir)),
                follow: ty == EntryType::Symlink,
                depth: child_depth,
                root_dev: task.root_dev,
                ancestors: child_ancestors.clone(),
            });
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serializes tests that touch the process-global absolute-fallback seam
    // (a force toggle + counter) so cargo's parallel test threads don't race.
    #[cfg(unix)]
    static SEAM_LOCK: Mutex<()> = Mutex::new(());

    fn base_args(threads: usize) -> Args {
        Args {
            threads,
            path: vec![],
            follow_symlinks: false,
            one_filesystem: true,
            max_depth: None,
            min_depth: None,
            max_scan_rate: None,
            max_results: None,
            name: None,
            regex: None,
            case_insensitive: false,
            file_type: vec![],
            meta: crate::meta::Predicates::default(),
            path_glob: None,
            lname: None,
            access: 0,
            exclude: None,
            null: false,
        }
    }

    fn collect(args: &Args, roots: &[&Path]) -> Vec<PathBuf> {
        let sink = Mutex::new(Vec::new());
        walk_parallel(args, roots, None, None, || {
            |e: Entry, _: &StatAt| {
                sink.lock().unwrap().push(e.path);
                WalkState::Continue
            }
        });
        sink.into_inner().unwrap()
    }

    // Captures each emitted entry's path together with the depth the walker
    // stamped on it (0 = root, 1 = its children, …).
    fn collect_depths(args: &Args, roots: &[&Path]) -> Vec<(PathBuf, usize)> {
        let sink = Mutex::new(Vec::new());
        walk_parallel(args, roots, None, None, || {
            |e: Entry, _: &StatAt| {
                sink.lock().unwrap().push((e.path, e.depth));
                WalkState::Continue
            }
        });
        sink.into_inner().unwrap()
    }

    #[test]
    fn entry_depth_reflects_distance_from_root() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::write(tmp.path().join("a/b/deep.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("top.txt"), b"x").unwrap();
        let got = collect_depths(&base_args(4), &[tmp.path()]);
        let depth = |name: &str| {
            got.iter()
                .find(|(p, _)| p.ends_with(name))
                .unwrap_or_else(|| panic!("{name} missing"))
                .1
        };
        assert_eq!(got.iter().find(|(p, _)| p == tmp.path()).unwrap().1, 0);
        assert_eq!(depth("top.txt"), 1);
        assert_eq!(depth("a"), 1);
        assert_eq!(depth("b"), 2);
        assert_eq!(depth("deep.txt"), 3);
    }

    #[test]
    fn emits_root_and_all_descendants() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::write(tmp.path().join("a/f.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("g.txt"), b"x").unwrap();
        let root = tmp.path();
        let got = collect(&base_args(4), &[root]);
        assert!(got.iter().any(|p| p == root));
        assert!(got.iter().any(|p| p.ends_with("a")));
        assert!(got.iter().any(|p| p.ends_with("a/f.txt")));
        assert!(got.iter().any(|p| p.ends_with("g.txt")));
        assert_eq!(got.len(), 4);
    }

    #[test]
    fn terminates_on_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let got = collect(&base_args(4), &[tmp.path()]);
        assert_eq!(got, vec![tmp.path().to_path_buf()]);
    }

    #[test]
    fn terminates_on_deep_chain() {
        let tmp = TempDir::new().unwrap();
        let mut p = tmp.path().to_path_buf();
        for i in 0..50 {
            p = p.join(format!("d{i}"));
            std::fs::create_dir(&p).unwrap();
        }
        let got = collect(&base_args(4), &[tmp.path()]);
        assert_eq!(got.len(), 51);
    }

    #[test]
    fn max_depth_limits_descent() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("l1/l2")).unwrap();
        std::fs::write(tmp.path().join("l1/l2/deep.txt"), b"x").unwrap();
        let mut args = base_args(4);
        args.max_depth = Some(1);
        let got = collect(&args, &[tmp.path()]);
        assert!(got.iter().any(|p| p.ends_with("l1")));
        assert!(!got.iter().any(|p| p.ends_with("deep.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_not_followed_by_default() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();
        let got = collect(&base_args(4), &[tmp.path()]);
        assert!(got.iter().any(|p| p.ends_with("link")));
        assert!(!got.iter().any(|p| p.starts_with(tmp.path().join("link"))
            && p.ends_with("inside.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_traversed() {
        // A command-line root that is a symlink to a directory must be walked
        // (find(1) behavior), even with follow_symlinks=false.
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("inside.txt"), b"x").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let got = collect(&base_args(4), &[link.as_path()]);
        assert!(
            got.iter().any(|p| p.ends_with("inside.txt")),
            "symlinked root must be descended"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_does_not_hang() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        std::fs::create_dir(&a).unwrap();
        std::os::unix::fs::symlink(&a, a.join("loop")).unwrap();
        let mut args = base_args(4);
        args.follow_symlinks = true;
        let got = collect(&args, &[tmp.path()]);
        assert!(got.iter().any(|p| p.ends_with("a")));
    }

    // Like `collect`, but keeps each entry's classified type.
    fn collect_typed(
        args: &Args,
        roots: &[&Path],
    ) -> Vec<(PathBuf, EntryType)> {
        let sink = Mutex::new(Vec::new());
        walk_parallel(args, roots, None, None, || {
            |e: Entry, _: &StatAt| {
                sink.lock().unwrap().push((e.path, e.file_type));
                WalkState::Continue
            }
        });
        sink.into_inner().unwrap()
    }

    #[test]
    fn multiple_roots_all_traversed() {
        let r1 = TempDir::new().unwrap();
        let r2 = TempDir::new().unwrap();
        std::fs::write(r1.path().join("one.txt"), b"x").unwrap();
        std::fs::write(r2.path().join("two.txt"), b"x").unwrap();
        let got = collect(&base_args(4), &[r1.path(), r2.path()]);
        assert!(got.iter().any(|p| p.ends_with("one.txt")));
        assert!(got.iter().any(|p| p.ends_with("two.txt")));
        // 2 roots + 2 files, each root emitted exactly once
        assert_eq!(got.len(), 4);
    }

    #[test]
    fn hidden_files_are_emitted() {
        // minifind shows dotfiles (unlike the ignore crate's default).
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".hidden"), b"x").unwrap();
        std::fs::create_dir(tmp.path().join(".dir")).unwrap();
        let got = collect(&base_args(4), &[tmp.path()]);
        assert!(got.iter().any(|p| p.ends_with(".hidden")));
        assert!(got.iter().any(|p| p.ends_with(".dir")));
    }

    #[test]
    fn no_duplicate_or_lost_entries() {
        // Exact set equality guards against the walker losing or duplicating
        // entries under work stealing.
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::create_dir(tmp.path().join("b")).unwrap();
        std::fs::write(tmp.path().join("a/x.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("b/y.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("c.txt"), b"x").unwrap();

        let got = collect(&base_args(8), &[tmp.path()]);
        let mut expected = vec![
            tmp.path().to_path_buf(),
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("a/x.txt"),
            tmp.path().join("b/y.txt"),
            tmp.path().join("c.txt"),
        ];
        let mut sorted = got.clone();
        sorted.sort();
        expected.sort();
        assert_eq!(sorted, expected, "walk must emit each entry exactly once");
        assert_eq!(got.len(), 6, "no duplicates");
    }

    #[test]
    fn max_depth_zero_emits_only_root() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("child.txt"), b"x").unwrap();
        let mut args = base_args(4);
        args.max_depth = Some(0);
        let got = collect(&args, &[tmp.path()]);
        assert_eq!(got, vec![tmp.path().to_path_buf()]);
    }

    #[cfg(unix)]
    #[test]
    fn classifies_entry_types() {
        // A symlink must be reported as Symlink (its own type, not the
        // target's) so `--type` selectors behave like find's default lstat.
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("f.txt"), b"x").unwrap();
        std::fs::create_dir(tmp.path().join("d")).unwrap();
        std::os::unix::fs::symlink(
            tmp.path().join("f.txt"),
            tmp.path().join("l"),
        )
        .unwrap();

        let got = collect_typed(&base_args(4), &[tmp.path()]);
        let ty = |name: &str| {
            got.iter()
                .find(|(p, _)| p.ends_with(name))
                .unwrap_or_else(|| panic!("{name} missing"))
                .1
        };
        assert_eq!(ty("f.txt"), EntryType::File);
        assert_eq!(ty("d"), EntryType::Dir);
        assert_eq!(ty("l"), EntryType::Symlink);
    }

    #[cfg(unix)]
    #[test]
    fn follow_symlinks_descends_into_symlinked_dir() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("link")).unwrap();

        let mut args = base_args(4);
        args.follow_symlinks = true;
        let got = collect(&args, &[tmp.path()]);
        // reached through the symlink, not just the real path
        assert!(
            got.iter().any(|p| p.starts_with(tmp.path().join("link"))
                && p.ends_with("inside.txt")),
            "follow_symlinks must descend into a symlinked directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_emitted_not_descended() {
        // A dangling symlink must be emitted (as a symlink) without error,
        // even when following is enabled (the open simply fails).
        let tmp = TempDir::new().unwrap();
        std::os::unix::fs::symlink(
            "/nonexistent/xyz/abc",
            tmp.path().join("broken"),
        )
        .unwrap();
        let mut args = base_args(4);
        args.follow_symlinks = true;
        let got = collect_typed(&args, &[tmp.path()]);
        let broken = got.iter().find(|(p, _)| p.ends_with("broken"));
        assert!(broken.is_some(), "broken symlink must still be emitted");
        assert_eq!(broken.unwrap().1, EntryType::Symlink);
    }

    // Like `collect`, but with an active `--exclude` glob set (matched against
    // each entry's file name).
    fn collect_excluding(
        args: &Args,
        roots: &[&Path],
        patterns: &[&str],
    ) -> Vec<PathBuf> {
        let pats: Vec<String> =
            patterns.iter().map(|s| (*s).to_string()).collect();
        let set =
            crate::glob::build_glob_set(Some(&pats), args.case_insensitive)
                .unwrap();
        let sink = Mutex::new(Vec::new());
        walk_parallel(args, roots, None, Some(&set), || {
            |e: Entry, _: &StatAt| {
                sink.lock().unwrap().push(e.path);
                WalkState::Continue
            }
        });
        sink.into_inner().unwrap()
    }

    #[test]
    fn exclude_prunes_directory_subtree() {
        // A matched directory is neither emitted nor descended: its whole
        // subtree is pruned before the child's opendir.
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
        std::fs::write(tmp.path().join(".git/config"), b"x").unwrap();
        std::fs::write(tmp.path().join("keep.txt"), b"x").unwrap();
        let got = collect_excluding(&base_args(4), &[tmp.path()], &[".git"]);
        assert!(got.iter().any(|p| p.ends_with("keep.txt")));
        assert!(
            !got.iter().any(|p| p.ends_with(".git")),
            "excluded directory must not be emitted"
        );
        assert!(
            !got.iter().any(|p| p.to_string_lossy().contains(".git/")),
            "excluded subtree must be pruned (not descended)"
        );
    }

    #[test]
    fn exclude_hides_matching_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.tmp"), b"x").unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
        let got = collect_excluding(&base_args(4), &[tmp.path()], &["*.tmp"]);
        assert!(got.iter().any(|p| p.ends_with("a.txt")));
        assert!(!got.iter().any(|p| p.ends_with("a.tmp")));
    }

    #[test]
    fn exclude_does_not_drop_the_root() {
        // The starting path is explicit and must survive even if its own name
        // matches an exclude glob.
        let tmp = TempDir::new().unwrap();
        let name =
            tmp.path().file_name().unwrap().to_string_lossy().to_string();
        let got = collect_excluding(&base_args(4), &[tmp.path()], &[&name]);
        assert!(got.iter().any(|p| p == tmp.path()));
    }

    #[cfg(unix)]
    #[test]
    fn followed_symlink_dir_emitted_exactly_once() {
        // A symlink-to-dir under --follow must be emitted once (by its
        // parent's read loop), not a second time by its own descend task.
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("inside.txt"), b"x").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut args = base_args(4);
        args.follow_symlinks = true;
        let got = collect(&args, &[tmp.path()]);
        let n = got.iter().filter(|p| p.ends_with("link")).count();
        assert_eq!(n, 1, "followed symlink-dir must be emitted exactly once");
    }

    #[cfg(unix)]
    #[test]
    fn normal_walk_takes_no_absolute_fallback() {
        // Healthy fd budget: every child must be opened relative to its
        // parent fd; the absolute-path fallback must never fire.
        let _guard = SEAM_LOCK.lock().unwrap();
        platform::reset_abs_fallback_count();
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b/c")).unwrap();
        std::fs::write(tmp.path().join("a/b/c/deep.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("top.txt"), b"x").unwrap();
        let _ = collect(&base_args(4), &[tmp.path()]);
        assert_eq!(
            platform::abs_fallback_count(),
            0,
            "normal walk must anchor every open on the parent fd"
        );
    }

    #[cfg(unix)]
    #[test]
    fn absolute_fallback_walk_is_complete() {
        // Force the EMFILE fallback path for every child open and assert the
        // emitted set is identical to a normal (anchored) walk.
        let _guard = SEAM_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::create_dir(tmp.path().join("a/sib")).unwrap();
        std::fs::write(tmp.path().join("a/b/deep.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("a/sib/s.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("top.txt"), b"x").unwrap();

        let mut expected = collect(&base_args(4), &[tmp.path()]);
        expected.sort();

        platform::set_force_abs_fallback(true);
        let mut got = collect(&base_args(4), &[tmp.path()]);
        platform::set_force_abs_fallback(false);
        got.sort();

        assert_eq!(got, expected, "fallback path must be complete");
        assert!(
            platform::abs_fallback_count() > 0,
            "the forced fallback must actually have fired"
        );
    }

    #[test]
    fn limiter_does_not_change_entry_set() {
        use crate::ratelimit::Limiter;
        use std::num::NonZeroU32;
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        std::fs::write(tmp.path().join("a/f.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("g.txt"), b"x").unwrap();

        let mut expected = collect(&base_args(4), &[tmp.path()]);
        expected.sort();

        // high rate: burst covers the tree, so no real sleeping occurs
        let limiter = Limiter::new(NonZeroU32::new(100_000).unwrap());
        let sink = Mutex::new(Vec::new());
        walk_parallel(
            &base_args(4),
            &[tmp.path()],
            Some(&limiter),
            None,
            || {
                |e: Entry, _: &StatAt| {
                    sink.lock().unwrap().push(e.path);
                    WalkState::Continue
                }
            },
        );
        let mut got = sink.into_inner().unwrap();
        got.sort();

        assert_eq!(got, expected);
    }
}
