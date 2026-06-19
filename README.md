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

Results can be filtered further by metadata: size (`--size`),
modification/change/access time (`--mtime`/`--ctime`/`--atime` in days,
`--mmin`/`--cmin`/`--amin` in minutes), permission bits (`--perm`, octal or
symbolic, with find's `/`/`-`/exact semantics), and owner
(`--uid`/`--gid`/`--user`/`--group`). Traversal can be bounded by depth
(`--min-depth`/`--max-depth`), whole subtrees pruned by name (`--exclude`),
and the walk stopped after the first N matches (`--max-results`). Output can be
NUL-terminated with `--null` (`-print0`) for safe piping into `xargs -0`. Most
flags also accept their find-style spellings (`-size`, `-mtime`, `-perm`, …).

By default, symlinks are not followed and filesystem boundaries are not
crossed. The thread count defaults to the number of available CPU cores. The
metadata predicates are the only ones that require a `stat`, and it is paid
lazily — only when such a predicate is set, and only for entries that pass the
cheaper name/type filters first.

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
  <PATH>...  Paths to traverse (must be existing directories)

Options:
  -f, --follow-symlinks    Follow symlinks [aliases: -L, -follow]
  -o, --one-filesystem     Do not cross mount points (default) [aliases: --xdev, -xdev, -mount]
      --no-one-filesystem  Cross mount points [alias: --cross-filesystem]
  -x, --threads <N>        Number of worker threads [default: logical CPU count]
  -d, --max-depth <N>      Maximum depth to traverse [alias: -maxdepth]
      --min-depth <N>      Minimum depth to emit (shallower entries are skipped) [alias: -mindepth]
  -s, --max-scan-rate <N>  Max directories scanned per second (0 = unlimited)
      --max-results <N>    Stop after the first N results (0 = unlimited)
  -n, --name <GLOB>        File-name globbing pattern (repeatable; conflicts with --regex) [aliases: -name; -iname adds -i]
  -r, --regex <RE>         Full-path regular expression (repeatable; conflicts with --name) [aliases: -regex; -iregex adds -i]
  -i, --case-insensitive   Case-insensitive glob/regex matching
  -E, --exclude <GLOB>     Exclude entries whose name matches GLOB; matched directories are pruned (repeatable)
  -0, --null               Terminate each path with NUL instead of newline [aliases: -print0, --print0]
  -t, --file-type <TYPE>   Filter matches by type (repeatable) [default: directory file symlink] [alias: -type]
                           values: empty, block-device, char-device, directory, pipe, file, socket, symlink
                           aliases: e, b, c, d, p, f, s, l
      --empty              Match empty files and directories (= --file-type empty) [alias: -empty]
      --size <[+-]N(c|k|M|G|T)>  Filter by size; unit required (c=bytes, k/M/G/T = 1024-based); +N greater, -N less [alias: -size]
      --mtime, --ctime, --atime <[+-]N>  Filter by modify/change/access time, in days [aliases: -mtime/-ctime/-atime]
      --mmin, --cmin, --amin <[+-]N>     Filter by modify/change/access time, in minutes [aliases: -mmin/-cmin/-amin]
      --perm <[/-]MODE>    Filter by permission bits, octal or symbolic; -MODE all set, /MODE any set, MODE exact [alias: -perm]
      --uid, --gid <[+-]N> Filter by numeric owner/group id [aliases: -uid/-gid]
      --user, --group <NAME>  Filter by owner/group name (or numeric id) [aliases: -user/-group]
  -h, --help               Print help
  -V, --version            Print version
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
