// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Custom parallel filesystem walker: a `crossbeam-deque` work-stealing
//! engine over a `cfg`-split leaf (`unix`/`fallback`).

use crate::args::Args;
use crate::filetype::EntryType;
use crate::ratelimit::Limiter;
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use crossbeam_utils::Backoff;
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
}

impl Entry {
    /// Final path component (used for `--name` glob matching); falls back to
    /// the whole path for roots like `/`.
    #[inline]
    pub fn file_name(&self) -> &OsStr {
        self.path.file_name().unwrap_or_else(|| self.path.as_os_str())
    }
}

struct Task {
    path: PathBuf,
    file_type: EntryType,
    depth: usize,
    root_dev: u64,
    // (dev, ino) of every ancestor directory; Some only when following
    // symlinks, so the common path stays allocation-free.
    ancestors: Option<Arc<Vec<(u64, u64)>>>,
}

/// Immutable shared state for one `walk_parallel` run, bundled so the
/// recursive worker functions keep small signatures.
struct WalkCtx<'a> {
    args: &'a Args,
    pending: &'a AtomicUsize,
    quit: &'a AtomicBool,
    limiter: Option<&'a Limiter>,
}

/// Walks `roots` in parallel, invoking a fresh per-thread visitor (from
/// `make_visitor`) for every entry. Directory read/open errors are skipped.
pub fn walk_parallel<F, V>(
    args: &Args,
    roots: &[&Path],
    limiter: Option<&Limiter>,
    make_visitor: F,
) where
    F: Fn() -> V + Sync,
    V: FnMut(Entry) -> WalkState + Send,
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
            file_type: EntryType::Dir,
            depth: 0,
            root_dev: dev,
            ancestors,
        });
    }

    let workers: Vec<Worker<Task>> =
        (0..n_workers).map(|_| Worker::new_lifo()).collect();
    let stealers: Vec<Stealer<Task>> =
        workers.iter().map(Worker::stealer).collect();

    let ctx = WalkCtx { args, pending: &pending, quit: &quit, limiter };

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

fn run_worker<V: FnMut(Entry) -> WalkState>(
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

fn process<V: FnMut(Entry) -> WalkState>(
    ctx: &WalkCtx,
    task: Task,
    local: &Worker<Task>,
    visitor: &mut V,
) {
    if let WalkState::Quit =
        visitor(Entry { path: task.path.clone(), file_type: task.file_type })
    {
        ctx.quit.store(true, Ordering::Relaxed);
        return;
    }
    descend(ctx, &task, local);
}

fn descend(ctx: &WalkCtx, task: &Task, local: &Worker<Task>) {
    let follow = match task.file_type {
        // depth 0 is a command-line root: follow it even if it is a
        // symlink-to-dir (like find(1)); deeper real dirs use O_NOFOLLOW.
        EntryType::Dir => task.depth == 0,
        EntryType::Symlink if ctx.args.follow_symlinks => true,
        _ => return,
    };
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

    let Ok(dir) = platform::open_dir(&task.path, follow) else {
        return;
    };
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

    // Iterate inline (no collected Vec); build each child path directly.
    let _ = platform::for_each_entry(&dir, &task.path, |path, raw_type| {
        let file_type = match raw_type {
            Some(t) => t,
            // DT_UNKNOWN: resolve the entry's own type; skip on failure
            None => match platform::lstat_type(&path) {
                Ok(t) => t,
                Err(_) => return,
            },
        };
        ctx.pending.fetch_add(1, Ordering::SeqCst);
        local.push(Task {
            path,
            file_type,
            depth: task.depth + 1,
            root_dev: task.root_dev,
            ancestors: child_ancestors.clone(),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn base_args(threads: usize) -> Args {
        Args {
            threads,
            path: vec![],
            follow_symlinks: false,
            one_filesystem: true,
            max_depth: None,
            max_iops: None,
            name: None,
            regex: None,
            case_insensitive: false,
            file_type: vec![],
        }
    }

    fn collect(args: &Args, roots: &[&Path]) -> Vec<PathBuf> {
        let sink = Mutex::new(Vec::new());
        walk_parallel(args, roots, None, || {
            |e: Entry| {
                sink.lock().unwrap().push(e.path);
                WalkState::Continue
            }
        });
        sink.into_inner().unwrap()
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
        walk_parallel(args, roots, None, || {
            |e: Entry| {
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
        walk_parallel(&base_args(4), &[tmp.path()], Some(&limiter), || {
            |e: Entry| {
                sink.lock().unwrap().push(e.path);
                WalkState::Continue
            }
        });
        let mut got = sink.into_inner().unwrap();
        got.sort();

        assert_eq!(got, expected);
    }
}
