use crate::args::Args;
use anyhow::{Error, anyhow};
use ignore::{WalkBuilder, WalkParallel};
use std::path::PathBuf;

/// Builds a walker for traversing paths based on the provided arguments and paths.
///
/// # Arguments
///
/// * `args` - A reference to the Args struct containing configuration options for the walker.
/// * `paths` - A slice of `PathBuf` representing the paths to traverse.
///
/// # Returns
///
/// * `Result<WalkParallel, Error>` - A parallel walker configured based on the provided arguments and paths, or an error if no paths are provided.
pub fn build_walker(
    args: &Args,
    paths: &[PathBuf],
) -> Result<WalkParallel, Error> {
    let first_path =
        paths.first().ok_or_else(|| anyhow!("no paths provided"))?;

    let mut builder = WalkBuilder::new(first_path);
    builder
        .hidden(false)
        .standard_filters(false)
        .follow_links(args.follow_symlinks)
        .same_file_system(args.one_filesystem)
        .max_depth(args.max_depth);

    for p in &paths[1..] {
        builder.add(p);
    }

    // reserve 1 thread for output
    Ok(builder.threads(args.threads - 1).build_parallel())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn base_args(path: PathBuf) -> Args {
        // Construct minimal valid Args; threads must be >= 2 because
        // build_walker reserves one thread for output (threads - 1).
        Args {
            threads: 2,
            path: vec![path],
            follow_symlinks: false,
            one_filesystem: true,
            max_depth: None,
            name: None,
            regex: None,
            case_insensitive: false,
            file_type: vec![
                args::FileType::File,
                args::FileType::Directory,
                args::FileType::Symlink,
            ],
        }
    }

    #[test]
    fn test_build_walker_no_panic() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let args = base_args(path.clone());
        let _walker = build_walker(&args, &[path]).unwrap();
    }

    #[test]
    fn test_build_walker_empty_paths_errors() {
        let tmp = TempDir::new().unwrap();
        let args = base_args(tmp.path().to_path_buf());
        assert!(build_walker(&args, &[]).is_err());
    }

    #[test]
    fn test_build_walker_with_max_depth() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.max_depth = Some(1);
        let _walker = build_walker(&args, &[path]).unwrap();
    }

    #[test]
    fn test_build_walker_follow_symlinks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.follow_symlinks = true;
        let _walker = build_walker(&args, &[path]).unwrap();
    }

    #[test]
    fn test_build_walker_no_one_filesystem() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.one_filesystem = false;
        let _walker = build_walker(&args, &[path]).unwrap();
    }

    #[test]
    fn test_build_walker_multiple_paths() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let p1 = tmp1.path().to_path_buf();
        let p2 = tmp2.path().to_path_buf();
        let args = base_args(p1.clone());
        let _walker = build_walker(&args, &[p1, p2]).unwrap();
    }

    fn collect_walk_parallel(walker: WalkParallel) -> Vec<PathBuf> {
        let paths: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        walker.run(|| {
            let paths = Arc::clone(&paths);
            Box::new(move |entry| {
                if let Ok(e) = entry {
                    paths.lock().unwrap().push(e.path().to_path_buf());
                }
                ignore::WalkState::Continue
            })
        });
        Arc::try_unwrap(paths).unwrap().into_inner().unwrap()
    }

    #[test]
    fn test_build_walker_yields_entries() {
        use std::fs;

        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hi").unwrap();
        let path = tmp.path().to_path_buf();
        let args = base_args(path.clone());
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        assert!(
            paths.iter().any(|p| p.ends_with("a.txt")),
            "a.txt not found in walk results"
        );
    }

    // W1 — hidden(false): dot-files must appear in results
    #[test]
    fn test_walk_hidden_files_are_visible() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".hidden"), b"x").unwrap();
        let path = tmp.path().to_path_buf();
        let args = base_args(path.clone());
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        assert!(
            paths.iter().any(|p| p.ends_with(".hidden")),
            ".hidden file must appear in walk results"
        );
    }

    // W2 — standard_filters(false): .gitignore rules must be ignored
    #[test]
    fn test_walk_gitignore_not_applied() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), b"target\n").unwrap();
        std::fs::write(tmp.path().join("target"), b"x").unwrap();
        let path = tmp.path().to_path_buf();
        let args = base_args(path.clone());
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        assert!(
            paths.iter().any(|p| p.ends_with("target")),
            "'target' must appear even though it is in .gitignore"
        );
    }

    // W3 — max_depth: entries deeper than the limit must be absent
    #[test]
    fn test_walk_max_depth_excludes_deep_entries() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("level1").join("level2"))
            .unwrap();
        std::fs::write(
            tmp.path().join("level1").join("level2").join("deep.txt"),
            b"x",
        )
        .unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.max_depth = Some(1);
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        assert!(
            !paths.iter().any(|p| p.ends_with("deep.txt")),
            "deep.txt at depth 2 must not appear when max_depth=1"
        );
    }

    // W4 — follow_links(false): symlink contents must not be traversed
    #[cfg(unix)]
    #[test]
    fn test_walk_symlink_not_followed_when_disabled() {
        let tmp = TempDir::new().unwrap();
        let real_dir = tmp.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        std::fs::write(real_dir.join("inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&real_dir, tmp.path().join("link"))
            .unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.follow_symlinks = false;
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        let link_traversed = paths.iter().any(|p| {
            p.starts_with(tmp.path().join("link")) && p.ends_with("inside.txt")
        });
        assert!(
            !link_traversed,
            "symlink contents must not be walked when follow_symlinks=false"
        );
    }

    // W4 — follow_links(true): symlink contents must be traversed
    #[cfg(unix)]
    #[test]
    fn test_walk_symlink_followed_when_enabled() {
        let tmp = TempDir::new().unwrap();
        let real_dir = tmp.path().join("real");
        std::fs::create_dir(&real_dir).unwrap();
        std::fs::write(real_dir.join("inside.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(&real_dir, tmp.path().join("link"))
            .unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.follow_symlinks = true;
        let walker = build_walker(&args, &[path]).unwrap();
        let paths = collect_walk_parallel(walker);
        let link_traversed = paths.iter().any(|p| {
            p.starts_with(tmp.path().join("link")) && p.ends_with("inside.txt")
        });
        assert!(
            link_traversed,
            "symlink contents must be walked when follow_symlinks=true"
        );
    }
}
