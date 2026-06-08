// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Unix leaf I/O for the walker: getdents-based directory listing via rustix.

use crate::filetype::EntryType;
use rustix::fs::{self, AtFlags, FileType as RFileType, Mode, OFlags, CWD};
use std::ffi::OsStr;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// An open directory file descriptor.
pub(crate) struct DirHandle(OwnedFd);

/// `(dev, ino)` of `path`, following symlinks (used to seed roots).
pub(crate) fn path_id(path: &Path) -> io::Result<(u64, u64)> {
    let st = fs::statat(CWD, path, AtFlags::empty())?;
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// Opens `path` as a directory. `follow` controls whether a final symlink is
/// resolved (true) or rejected with O_NOFOLLOW (false).
pub(crate) fn open_dir(path: &Path, follow: bool) -> io::Result<DirHandle> {
    let mut flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    if !follow {
        flags |= OFlags::NOFOLLOW;
    }
    let fd = fs::openat(CWD, path, flags, Mode::empty())?;
    Ok(DirHandle(fd))
}

/// `(dev, ino)` of an open directory (for same-fs and loop checks).
pub(crate) fn dir_id(d: &DirHandle) -> io::Result<(u64, u64)> {
    let st = fs::fstat(&d.0)?;
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// Invokes `f` for each entry (excluding `.`/`..`) with its full path — built
/// directly from the raw name, no intermediate `OsString` or collected `Vec` —
/// and its `d_type` (`None` on `DT_UNKNOWN`).
pub(crate) fn for_each_entry(
    d: &DirHandle,
    parent: &Path,
    mut f: impl FnMut(PathBuf, Option<EntryType>),
) -> io::Result<()> {
    let dir = fs::Dir::read_from(&d.0)?;
    for entry in dir {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        f(parent.join(OsStr::from_bytes(bytes)), map_type(entry.file_type()));
    }
    Ok(())
}

/// Resolves a `DT_UNKNOWN` entry's own type via lstat.
pub(crate) fn lstat_type(path: &Path) -> io::Result<EntryType> {
    let st = fs::statat(CWD, path, AtFlags::SYMLINK_NOFOLLOW)?;
    map_type(RFileType::from_raw_mode(st.st_mode))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))
}

fn map_type(ft: RFileType) -> Option<EntryType> {
    match ft {
        RFileType::RegularFile => Some(EntryType::File),
        RFileType::Directory => Some(EntryType::Dir),
        RFileType::Symlink => Some(EntryType::Symlink),
        RFileType::BlockDevice => Some(EntryType::BlockDevice),
        RFileType::CharacterDevice => Some(EntryType::CharDevice),
        RFileType::Fifo => Some(EntryType::Fifo),
        RFileType::Socket => Some(EntryType::Socket),
        RFileType::Unknown => None,
    }
}
