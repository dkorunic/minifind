// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Non-Unix leaf I/O for the walker, built on std::fs.
//!
//! `dev` is reported as 0 (so `--one-filesystem` is a no-op on this platform —
//! a documented limitation); `ino` is a stable hash of the canonical path so
//! symlink-loop detection still works.

use crate::filetype::EntryType;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) struct DirHandle(PathBuf);

fn path_hash(path: &Path) -> u64 {
    let canon =
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    h.finish()
}

pub(crate) fn path_id(path: &Path) -> io::Result<(u64, u64)> {
    Ok((0, path_hash(path)))
}

pub(crate) fn open_dir(path: &Path, _follow: bool) -> io::Result<DirHandle> {
    let _ = std::fs::read_dir(path)?;
    Ok(DirHandle(path.to_path_buf()))
}

pub(crate) fn dir_id(d: &DirHandle) -> io::Result<(u64, u64)> {
    Ok((0, path_hash(&d.0)))
}

pub(crate) fn for_each_entry(
    d: &DirHandle,
    parent: &Path,
    mut f: impl FnMut(PathBuf, Option<EntryType>),
) -> io::Result<()> {
    for entry in std::fs::read_dir(&d.0)? {
        let entry = entry?;
        f(
            parent.join(entry.file_name()),
            entry.file_type().ok().map(map_type),
        );
    }
    Ok(())
}

pub(crate) fn lstat_type(path: &Path) -> io::Result<EntryType> {
    Ok(map_type(std::fs::symlink_metadata(path)?.file_type()))
}

fn map_type(ft: std::fs::FileType) -> EntryType {
    if ft.is_dir() {
        EntryType::Dir
    } else if ft.is_symlink() {
        EntryType::Symlink
    } else {
        EntryType::File
    }
}
