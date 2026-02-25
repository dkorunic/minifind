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

Hardware: 8-core Xeon E5-1630 with a 4-drive SATA RAID-10 array

Benchmark setup:

```shell
$ cat bench1.sh
#!/bin/dash
exec /usr/bin/find / -xdev

$ cat bench2.sh
#!/bin/dash
exec /usr/local/sbin/minifind /
```

```shell
Benchmark 1: ./bench1.sh
  Time (mean ± σ):      4.655 s ±  0.160 s    [User: 1.287 s, System: 3.366 s]
  Range (min … max):    4.525 s …  5.016 s    10 runs

Benchmark 2: ./bench2.sh
  Time (mean ± σ):      1.244 s ±  0.020 s    [User: 3.921 s, System: 5.908 s]
  Range (min … max):    1.199 s …  1.271 s    10 runs

Summary
  ./bench2.sh ran
    3.74 ± 0.14 times faster than ./bench1.sh
```
