// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Bitmask `ignore_filetype` vs the original seven-term `||` chain.
//!
//! Entries are collected before the timed loop (measures classification, not
//! traversal), and `--empty` is left off so no `stat`/`read_dir` runs — the
//! comparison stays CPU-bound.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use ignore::{DirEntry, WalkBuilder};
use minifind::args;
use minifind::filetype::{EntryType, FileType};
use tempfile::TempDir;

/// Maps an `ignore::DirEntry`'s `file_type()` to the walker's `EntryType`,
/// mirroring the classification the real walker performs from `d_type`.
fn entry_type(dir_entry: &DirEntry) -> Option<EntryType> {
    let t = dir_entry.file_type()?;
    Some(if t.is_dir() {
        EntryType::Dir
    } else if t.is_symlink() {
        EntryType::Symlink
    } else {
        EntryType::File
    })
}

/// Populates a fixture tree and collects every `DirEntry` from a single walk.
fn collect_entries(tmp: &TempDir) -> Vec<DirEntry> {
    for i in 0..200 {
        std::fs::write(tmp.path().join(format!("file_{i}.txt")), b"x")
            .unwrap();
    }
    for i in 0..50 {
        let d = tmp.path().join(format!("dir_{i}"));
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("inner.txt"), b"y").unwrap();
    }
    #[cfg(unix)]
    for i in 0..50 {
        std::os::unix::fs::symlink(
            tmp.path().join(format!("file_{i}.txt")),
            tmp.path().join(format!("link_{i}")),
        )
        .unwrap();
    }

    WalkBuilder::new(tmp.path())
        .hidden(false)
        .standard_filters(false)
        .build()
        .filter_map(Result::ok)
        .collect()
}

// --- pre-refactor implementation, kept here only for comparison ---

#[cfg(unix)]
fn is_block(t: std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    t.is_block_device()
}
#[cfg(unix)]
fn is_char(t: std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    t.is_char_device()
}
#[cfg(unix)]
fn is_fifo(t: std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    t.is_fifo()
}
#[cfg(unix)]
fn is_sock(t: std::fs::FileType) -> bool {
    use std::os::unix::fs::FileTypeExt;
    t.is_socket()
}
#[cfg(not(unix))]
fn is_block(_: std::fs::FileType) -> bool {
    false
}
#[cfg(not(unix))]
fn is_char(_: std::fs::FileType) -> bool {
    false
}
#[cfg(not(unix))]
fn is_fifo(_: std::fs::FileType) -> bool {
    false
}
#[cfg(not(unix))]
fn is_sock(_: std::fs::FileType) -> bool {
    false
}

/// The original seven-term `||` chain. `sel` (file, dir, symlink, block, char,
/// pipe, socket) is taken at runtime so the caller can keep it opaque to the
/// optimizer — hardcoded flags would let LLVM fold the chain away and skew the
/// comparison.
fn ignore_boolchain(sel: [bool; 7], dir_entry: &DirEntry) -> bool {
    let [file, directory, symlink, block_device, char_device, pipe, socket] =
        sel;

    if let Some(t) = dir_entry.file_type() {
        (!file && t.is_file())
            || (!directory && t.is_dir())
            || (!symlink && t.is_symlink())
            || (!block_device && is_block(t))
            || (!char_device && is_char(t))
            || (!pipe && is_fifo(t))
            || (!socket && is_sock(t))
    } else {
        true
    }
}

fn bench_ignore_filetype(c: &mut Criterion) {
    let tmp = TempDir::new().unwrap();
    let entries = collect_entries(&tmp);
    let ft = FileType::new(&[
        args::FileType::File,
        args::FileType::Directory,
        args::FileType::Symlink,
    ]);

    let mut group = c.benchmark_group("ignore_filetype");
    group.throughput(criterion::Throughput::Elements(entries.len() as u64));

    // black_box the selection once outside the loop so neither side can fold
    // it, mirroring the real program's runtime-constant selection
    group.bench_function("bitmask", |b| {
        let ft = black_box(ft);
        b.iter(|| {
            let mut acc = 0usize;
            for e in &entries {
                let e = black_box(e);
                if let Some(ty) = entry_type(e) {
                    acc += usize::from(ft.ignore_filetype(ty, e.path()));
                }
            }
            black_box(acc)
        });
    });

    group.bench_function("boolchain", |b| {
        let sel = black_box([true, true, true, false, false, false, false]);
        b.iter(|| {
            let mut acc = 0usize;
            for e in &entries {
                acc += usize::from(ignore_boolchain(sel, black_box(e)));
            }
            black_box(acc)
        });
    });

    group.finish();
}

criterion_group!(benches, bench_ignore_filetype);
criterion_main!(benches);
