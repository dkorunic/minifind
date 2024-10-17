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
    let walker = builder.threads(args.threads - 1).build_parallel();

    walker
}
