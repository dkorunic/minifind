# minifind

[![GitHub license](https://img.shields.io/github/license/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/blob/master/LICENSE.txt)
[![GitHub release](https://img.shields.io/github/release/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/releases/latest)
[![release](https://github.com/dkorunic/minifind/actions/workflows/release.yml/badge.svg)](https://github.com/dkorunic/minifind/actions/workflows/release.yml)

## About

`minifind` is a minimal Unix `find` reimplementation in Rust, designed to list
directory entries as fast as possible. Filename or path matching is supported
via `--name` (glob) or `--regex` (regular expression) options, with optional
case-insensitive matching controlled by `--case-insensitive`. Results can be
narrowed further using `--file-type` to filter by entry type: `b` for block
device, `c` for character device, `d` for directory, `p` for named FIFO, `f`
for regular file, `l` for symlink, `s` for socket, or `e` for empty
file/directory. Both `--name` and `--regex` accept multiple patterns.

By default, symlinks are not followed and filesystem boundaries are not
crossed. The thread count defaults to the number of available CPU cores.

## Related projects

Other notable projects in this space:

- [sharkdp/fd](https://github.com/sharkdp/fd) — a much more fully-featured
  `find` alternative with excellent performance
- [LyonSyonII/hunt-rs](https://github.com/LyonSyonII/hunt-rs) — a similar
  high-performance-oriented tool
- [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) — also home to
  the [globset](https://github.com/BurntSushi/ripgrep/tree/master/crates/globset)
  and [ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore)
  crates used by this project
- [uutils/findutils](https://github.com/uutils/findutils) — a Rust
  reimplementation of findutils intended as a drop-in replacement

## Usage

```shell
minimal find reimplementation

Usage: minifind [OPTIONS] <PATH>...

Arguments:
  <PATH>...  Paths to check for large directories

Options:
  -f, --follow-symlinks <FOLLOW_SYMLINKS>    Follow symlinks [default: false] [aliases: -L] [possible values: true, false]
  -o, --one-filesystem <ONE_FILESYSTEM>      Do not cross mount points [default: true] [aliases: --xdev] [possible values: true, false]
  -x, --threads <THREADS>                    Number of threads to use when calibrating and scanning [default: 20]
  -d, --max-depth <MAX_DEPTH>                Maximum depth to traverse
  -n, --name <NAME>                          Base of the file name matching globbing pattern
  -r, --regex <REGEX>                        File name (full path) matching regular expression pattern
  -i, --case-insensitive <CASE_INSENSITIVE>  Case-insensitive matching for globbing and regular expression patterns [default: false] [possible values: true, false]
  -t, --file-type <FILE_TYPE>                Filter matches by type. Also accepts 'b', 'c', 'd', 'p', 'f', 'l', 's' and 'e' aliases [default: directory file symlink]
                                             [possible values: empty, block-device, char-device, directory, pipe, file, symlink, socket]
  -h, --help                                 Print help
  -V, --version                              Print version
```

### Regular expressions

The `--regex` option uses Rust [regex syntax](https://docs.rs/regex/latest/regex/#syntax),
which is similar to other engines but does not support look-around or
backreferences.

### Glob expressions

The `--name` option uses Unix-style [glob syntax](https://docs.rs/globset/latest/globset/#syntax).

## minifind vs GNU find

Hardware: 4-core / 8-thread Intel Xeon E5-1630 v3 @ 3.70 GHz, 48 GB RAM.

Measured with the Criterion benchmark in [`benches/walk.rs`](benches/walk.rs)
over a shallow clone of the mainline Linux kernel tree (99,893 entries across
6,158 directories, ~2 GB) with a warm page cache. Both `minifind` (defaults)
and GNU `find` run as subprocesses, so each pays process-startup cost; output
is discarded for both. 100 samples each:

```text
walk_linux_kernel/minifind   time: [20.630 ms 20.710 ms 20.797 ms]
walk_linux_kernel/find       time: [78.989 ms 79.237 ms 79.497 ms]
```

So `minifind` walks the tree in **~20.7 ms vs ~79.2 ms — about 3.8× faster**
(≈4.8M vs ≈1.3M entries/second). Reproduce with `cargo bench --bench walk`
(set `BENCH_WALK_DIR=/path/to/tree` to benchmark an existing checkout).

### Why it is faster

- **Parallel traversal.** GNU `find` walks on a single thread; `minifind`
  fans out across all cores with its own work-stealing walker (one worker per
  core, minus one thread reserved for output), overlapping directory reads. On
  this 8-thread machine that accounts for most of the gap — the advantage
  scales with core count and shrinks toward parity on a 1–2 core host.
- **Purpose-built walker.** `minifind` uses its own walker (raw `getdents64`
  via `rustix` on Unix, `std::fs` elsewhere) rather than a general-purpose
  crate, so it carries no gitignore/hidden-file bookkeeping it does not need.
- **No extra `stat(2)`.** File-type filtering uses the `d_type` already
  returned by `getdents(2)`, avoiding a per-entry `stat` for `-type`-style
  matching.
- **Batched, lock-light output.** Matched entries are streamed to a dedicated
  output thread in batches (amortizing channel synchronization), then written
  straight into a 256 KB buffered writer with one copy per entry.
- **Fast allocator.** `mimalloc` keeps the unavoidable per-entry path
  allocations cheap.

The warm-cache setup isolates CPU and syscall efficiency rather than disk
latency; on a cold cache both tools are bound by I/O and the gap narrows.
