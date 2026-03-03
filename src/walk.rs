use crate::args::Args;
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
/// * `WalkParallel` - A parallel walker configured based on the provided arguments and paths.
pub fn build_walker(args: &Args, paths: &[PathBuf]) -> WalkParallel {
    let first_path = &paths[0];

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
    builder.threads(args.threads - 1).build_parallel()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;
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
        let _walker = build_walker(&args, &[path]);
    }

    #[test]
    fn test_build_walker_with_max_depth() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.max_depth = Some(1);
        let _walker = build_walker(&args, &[path]);
    }

    #[test]
    fn test_build_walker_follow_symlinks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.follow_symlinks = true;
        let _walker = build_walker(&args, &[path]);
    }

    #[test]
    fn test_build_walker_no_one_filesystem() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut args = base_args(path.clone());
        args.one_filesystem = false;
        let _walker = build_walker(&args, &[path]);
    }

    #[test]
    fn test_build_walker_multiple_paths() {
        let tmp1 = TempDir::new().unwrap();
        let tmp2 = TempDir::new().unwrap();
        let p1 = tmp1.path().to_path_buf();
        let p2 = tmp2.path().to_path_buf();
        let args = base_args(p1.clone());
        let _walker = build_walker(&args, &[p1, p2]);
    }

    #[test]
    fn test_build_walker_yields_entries() {
        use std::fs;
        use std::sync::{Arc, Mutex};

        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hi").unwrap();
        let path = tmp.path().to_path_buf();
        let args = base_args(path.clone());
        let walker = build_walker(&args, &[path]);

        let paths: Arc<Mutex<Vec<PathBuf>>> =
            Arc::new(Mutex::new(Vec::new()));
        walker.run(|| {
            let paths = Arc::clone(&paths);
            Box::new(move |entry| {
                if let Ok(e) = entry {
                    paths.lock().unwrap().push(e.path().to_path_buf());
                }
                ignore::WalkState::Continue
            })
        });

        let paths = paths.lock().unwrap();
        assert!(
            paths.iter().any(|p| p.ends_with("a.txt")),
            "a.txt not found in walk results"
        );
    }
}
