# minifind

[![GitHub license](https://img.shields.io/github/license/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/blob/master/LICENSE.txt)
[![GitHub release](https://img.shields.io/github/release/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/releases/latest)
[![Rust Report Card](https://rust-reportcard.xuri.me/badge/github.com/dkorunic/minifind)](https://rust-reportcard.xuri.me/report/github.com/dkorunic/minifind)
[![release](https://github.com/dkorunic/minifind/actions/workflows/release.yml/badge.svg)](https://github.com/dkorunic/minifind/actions/workflows/release.yml)

## About

`minifind` is a barebones Un\*x `find` tool implementation in Rust, meant just to list directory entries as fast as possible and little else. For filename or path matching, it is possible to use `--name` or `--regex` options, toggling case insensitivity with `--case-insensitive` or not. Additionally to narrow down matches, it is possible to use `--file-type` option and filter by file type (`f` for files, `d` for directories and `l` for symlinks).

It will not follow filesystem symlinks and it will not cross filesystem boundaries by default. Number of threads used is set to the number of available CPU cores in the system.

## Related projects

Let us also mention other notable projects dealing with this task:

- [sharkdp/fd](https://github.com/sharkdp/fd) which is a much more featured find alternative but with excellent performance,
- [LyonSyonII/hunt-rs](https://github.com/LyonSyonII/hunt-rs), a very similar high performance-oriented tool,
- [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) which also houses [globset](https://github.com/BurntSushi/ripgrep/tree/master/crates/globset) and [ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore) crates that are used in this project,
- Rust [findutils](https://github.com/uutils/findutils) reimplementation that can be used as drop-in replacement.

## Usage

```shell
Usage: minifind [OPTIONS] <PATH>...

Arguments:
  <PATH>...  Paths to check for large directories

Options:
  -f, --follow-symlinks <FOLLOW_SYMLINKS>    Follow symlinks [default: false] [short aliases: L] [possible values: true, false]
  -o, --one-filesystem <ONE_FILESYSTEM>      Do not cross mount points [default: true] [aliases: xdev] [possible values: true, false]
  -x, --threads <THREADS>                    Number of threads to use when calibrating and scanning [default: 20]
  -d, --max-depth <MAX_DEPTH>                Maximum depth to traverse
  -n, --name <NAME>                          Base of the file name matching globbing pattern
  -r, --regex <REGEX>                        File name (full path) matching regular expression pattern
  -i, --case-insensitive <CASE_INSENSITIVE>  Case-insensitive matching for globbing and regular expression patterns [default: false] [possible values: true, false]
  -t, --file-type <FILE_TYPE>                Filter matches by type. Also accepts 'f', 'd', and 'l' [default: directory file symlink] [possible values: file,
                                             directory, symlink]
  -h, --help                                 Print help
  -V, --version                              Print version
```

### Regular expressions

`--regex` option uses Rust [regex syntax](https://docs.rs/regex/latest/regex/#syntax) that is very similar to other engines but without support for look-around and backreferences.

### Glob expressions

`--name` option uses Unix-style [glob syntax](https://docs.rs/globset/latest/globset/#syntax).

## Minifind vs GNU find

Hardware: 8-core Xeon E5-1630 with 4-drive SATA RAID-10

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
