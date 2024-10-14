# minifind

[![GitHub license](https://img.shields.io/github/license/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/blob/master/LICENSE.txt)
[![GitHub release](https://img.shields.io/github/release/dkorunic/minifind.svg)](https://github.com/dkorunic/minifind/releases/latest)
[![Rust Report Card](https://rust-reportcard.xuri.me/badge/github.com/dkorunic/minifind)](https://rust-reportcard.xuri.me/report/github.com/dkorunic/minifind)
[![release](https://github.com/dkorunic/minifind/actions/workflows/release.yml/badge.svg)](https://github.com/dkorunic/minifind/actions/workflows/release.yml)

## About

`minifind` is a barebones Un\*x `find` tool implementation in Rust, meant just to list directory entries as fast as possible and nothing else. Directories have trailing slash added.

It will not follow filesystem symlinks and it will not cross filesystem boundaries by default. Number of threads used is set to the number of available CPU cores in the system.

## Usage

```shell
Usage: minifind [OPTIONS] <PATH>...

Arguments:
  <PATH>...  Paths to check for large directories

Options:
  -f, --follow-symlinks <FOLLOW_SYMLINKS>  Follow symlinks [default: false] [possible values: true, false]
  -o, --one-filesystem <ONE_FILESYSTEM>    Do not cross mount points [default: true] [possible values: true, false]
  -x, --threads <THREADS>                  Number of threads to use when calibrating and scanning [default: 20]
  -d, --max-depth <MAX_DEPTH>              Maximum depth to traverse
  -h, --help                               Print help
  -V, --version                            Print version
```

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
