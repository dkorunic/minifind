// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Non-Unix leaf I/O for the walker, built on std::fs.
//!
//! There is no directory fd to anchor on here, so [`DirFd`] is just the
//! directory path: children are opened by absolute path and `open_child`
//! ignores its parent anchor. `dev` is reported as 0 (so `--one-filesystem`
//! is a no-op on this platform — a documented limitation); `ino` is a stable
//! hash of the canonical path so symlink-loop detection still works.

use crate::filetype::EntryType;
use crate::meta::Meta;
use std::ffi::OsStr;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

/// The shareable directory anchor. With no fd to carry, it is the path; the
/// walker still wraps it in `Arc` for a uniform cross-platform signature.
pub(crate) type DirFd = PathBuf;

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

pub(crate) fn open_root(path: &Path, _follow: bool) -> io::Result<DirFd> {
    let _ = std::fs::read_dir(path)?;
    Ok(path.to_path_buf())
}

/// Opens a child directory. With no parent fd to anchor on, this resolves the
/// full path directly (the parent anchor and leaf are unused on this leaf).
pub(crate) fn open_child(
    _parent: &DirFd,
    _leaf: &OsStr,
    full: &Path,
    follow: bool,
) -> io::Result<DirFd> {
    open_root(full, follow)
}

pub(crate) fn dir_id(d: &DirFd) -> io::Result<(u64, u64)> {
    Ok((0, path_hash(d)))
}

pub(crate) fn for_each_entry(
    d: &DirFd,
    parent: &Path,
    mut f: impl FnMut(PathBuf, &OsStr, EntryType) -> bool,
) -> io::Result<()> {
    for entry in std::fs::read_dir(d)? {
        let entry = entry?;
        let name = entry.file_name();
        let path = parent.join(&name);
        let ty = match entry.file_type() {
            Ok(ft) => map_type(ft),
            // DT_UNKNOWN equivalent: resolve the entry's own type; skip on
            // failure.
            Err(_) => match std::fs::symlink_metadata(&path) {
                Ok(m) => map_type(m.file_type()),
                Err(_) => continue,
            },
        };
        if !f(path, &name, ty) {
            break;
        }
    }
    Ok(())
}

/// Fetches metadata for a child (no dir fd to anchor on, so by full path).
/// `mask` is ignored — `std::fs` always returns the full set; uid/gid/mode are
/// unavailable on this platform (those predicates are rejected at parse).
pub(crate) fn stat_at(
    dir: &DirFd,
    name: &OsStr,
    follow: bool,
    _mask: u32,
) -> io::Result<Meta> {
    meta_of(&dir.join(name), follow)
}

pub(crate) fn stat_root(
    path: &Path,
    follow: bool,
    _mask: u32,
) -> io::Result<Meta> {
    meta_of(path, follow)
}

fn meta_of(path: &Path, follow: bool) -> io::Result<Meta> {
    let md = if follow {
        std::fs::metadata(path)?
    } else {
        std::fs::symlink_metadata(path)?
    };
    let secs = |t: io::Result<std::time::SystemTime>| -> i64 {
        t.ok()
            .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64)
    };
    Ok(Meta {
        size: md.len(),
        mtime: secs(md.modified()),
        ctime: secs(md.created()),
        atime: secs(md.accessed()),
        // mode/uid/gid/nlink/ino need Unix metadata; those predicates are
        // rejected at parse off-Unix, so zero placeholders are never read.
        mode: 0,
        uid: 0,
        gid: 0,
        nlink: 0,
        ino: 0,
    })
}

/// `-readable`/`-writable`/`-executable` are Unix-only (rejected at parse off
/// Unix), so these never run; they exist to keep the leaf API uniform.
pub(crate) fn access_at(_dir: &DirFd, _name: &OsStr, _mode: u8) -> bool {
    true
}

pub(crate) fn access_root(_path: &Path, _mode: u8) -> bool {
    true
}

/// Reads a symlink's target (for `-lname`) by full path.
pub(crate) fn readlink_at(
    dir: &DirFd,
    name: &OsStr,
) -> Option<std::ffi::OsString> {
    std::fs::read_link(dir.join(name)).ok().map(PathBuf::into_os_string)
}

pub(crate) fn readlink_root(path: &Path) -> Option<std::ffi::OsString> {
    std::fs::read_link(path).ok().map(PathBuf::into_os_string)
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
