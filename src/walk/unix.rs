// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Unix leaf I/O for the walker: getdents-based directory listing via rustix.
//!
//! Children are opened and stat'd *relative to their parent directory fd*
//! ([`DirFd`]), which the walker shares down the queue as `Arc<DirFd>`. The
//! anchor fd is only ever an `openat`/`statat` lookup target — never iterated
//! (`Dir::read_from` opens its own fd), so a sibling worker stat'ing through
//! the same description cannot disturb a concurrent iteration.

use crate::filetype::EntryType;
use crate::meta::{self, Meta};
use rustix::fs::{self, AtFlags, FileType as RFileType, Mode, OFlags, CWD};
use rustix::io::Errno;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A shareable open-directory anchor: one fd, refcounted by the walker via
/// `Arc` so children can `openat`/`statat` relative to it.
pub(crate) type DirFd = OwnedFd;

/// Counts opens that fell back to an absolute-path open — the EMFILE/ENFILE
/// fd-exhaustion backstop (and the test seam). Zero on a healthy walk, where
/// every open is anchored on the parent fd.
pub(crate) static ABS_FALLBACK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Test seam: force every child open onto the absolute-path fallback so the
/// fallback can be exercised deterministically without exhausting fds.
#[cfg(test)]
pub(crate) static FORCE_ABS_FALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn abs_fallback_count() -> usize {
    ABS_FALLBACK_COUNT.load(Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) fn reset_abs_fallback_count() {
    ABS_FALLBACK_COUNT.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn set_force_abs_fallback(on: bool) {
    FORCE_ABS_FALLBACK.store(on, Ordering::Relaxed);
}

fn dir_flags(follow: bool) -> OFlags {
    let mut flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    if !follow {
        // O_NOFOLLOW makes a swapped-to-symlink leaf fail closed
        // (ELOOP/ENOTDIR) rather than redirecting the walk.
        flags |= OFlags::NOFOLLOW;
    }
    flags
}

/// `(dev, ino)` of `path`, following symlinks (used to seed roots).
pub(crate) fn path_id(path: &Path) -> io::Result<(u64, u64)> {
    let st = fs::statat(CWD, path, AtFlags::empty())?;
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// Opens a command-line root by absolute path. `follow` resolves a final
/// symlink (roots behave like `find(1)`); deeper dirs use `O_NOFOLLOW`.
pub(crate) fn open_root(path: &Path, follow: bool) -> io::Result<DirFd> {
    Ok(fs::openat(CWD, path, dir_flags(follow), Mode::empty())?)
}

/// Opens a child directory relative to its parent's fd — closing the
/// intermediate-component swap window an absolute reopen leaves open. On fd
/// exhaustion (`EMFILE`/`ENFILE`) it falls back to an absolute open of `full`
/// as a completeness backstop: same fd cost, weaker TOCTOU, counted in
/// [`ABS_FALLBACK_COUNT`].
pub(crate) fn open_child(
    parent: &DirFd,
    leaf: &OsStr,
    full: &Path,
    follow: bool,
) -> io::Result<DirFd> {
    let flags = dir_flags(follow);
    #[cfg(test)]
    if FORCE_ABS_FALLBACK.load(Ordering::Relaxed) {
        return open_abs(full, flags);
    }
    match fs::openat(parent, leaf, flags, Mode::empty()) {
        Ok(fd) => Ok(fd),
        Err(e) if e == Errno::MFILE || e == Errno::NFILE => {
            open_abs(full, flags)
        }
        Err(e) => Err(e.into()),
    }
}

fn open_abs(full: &Path, flags: OFlags) -> io::Result<DirFd> {
    ABS_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(fs::openat(CWD, full, flags, Mode::empty())?)
}

/// `(dev, ino)` of an open directory (for same-fs and loop checks).
pub(crate) fn dir_id(fd: &DirFd) -> io::Result<(u64, u64)> {
    let st = fs::fstat(fd)?;
    Ok((st.st_dev as u64, st.st_ino as u64))
}

/// Invokes `f` for each entry (excluding `.`/`..`) with its full path — built
/// directly from the raw name, no intermediate `OsString` or collected `Vec` —
/// and its resolved type, stopping early when `f` returns `false`.
///
/// `DT_UNKNOWN` is resolved with a `statat` relative to the directory fd
/// (cheaper and TOCTOU-consistent with the anchored open); an entry whose type
/// cannot be resolved is skipped.
pub(crate) fn for_each_entry(
    fd: &DirFd,
    parent: &Path,
    mut f: impl FnMut(PathBuf, &OsStr, EntryType) -> bool,
) -> io::Result<()> {
    let dir = fs::Dir::read_from(fd)?;
    for entry in dir {
        let entry = entry?;
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        let name = OsStr::from_bytes(bytes);
        let ty = match map_type(entry.file_type()) {
            Some(t) => t,
            None => match statat_type(fd, name) {
                Ok(t) => t,
                Err(_) => continue,
            },
        };
        // `name` borrows the dir-stream buffer (valid this call); the caller
        // reuses it for the leaf-relative statx
        if !f(parent.join(name), name, ty) {
            break;
        }
    }
    Ok(())
}

/// Fetches the metadata fields named by `mask` for a child, relative to its
/// parent dir fd. On Linux a `statx` with a minimal field mask; on other Unix
/// a `statat` (full `stat`, mask ignored). `follow` resolves a final symlink.
pub(crate) fn stat_at(
    dir: &DirFd,
    name: &OsStr,
    follow: bool,
    mask: u32,
) -> io::Result<Meta> {
    do_stat(dir, name, follow, mask)
}

/// Like [`stat_at`] but for a command-line root, addressed by absolute path.
pub(crate) fn stat_root(
    path: &Path,
    follow: bool,
    mask: u32,
) -> io::Result<Meta> {
    do_stat(CWD, path, follow, mask)
}

#[cfg(target_os = "linux")]
fn do_stat(
    dirfd: impl rustix::fd::AsFd,
    path: impl rustix::path::Arg,
    follow: bool,
    mask: u32,
) -> io::Result<Meta> {
    let flags = stat_flags(follow);
    let sx = fs::statx(dirfd, path, flags, to_statx_flags(mask))?;
    Ok(Meta {
        size: sx.stx_size,
        mtime: sx.stx_mtime.tv_sec,
        ctime: sx.stx_ctime.tv_sec,
        atime: sx.stx_atime.tv_sec,
        mode: u32::from(sx.stx_mode),
        uid: sx.stx_uid,
        gid: sx.stx_gid,
        nlink: u64::from(sx.stx_nlink),
        ino: sx.stx_ino,
    })
}

#[cfg(target_os = "linux")]
fn to_statx_flags(mask: u32) -> rustix::fs::StatxFlags {
    use rustix::fs::StatxFlags as S;
    let pairs = [
        (meta::mask::SIZE, S::SIZE),
        (meta::mask::MTIME, S::MTIME),
        (meta::mask::CTIME, S::CTIME),
        (meta::mask::ATIME, S::ATIME),
        (meta::mask::MODE, S::MODE),
        (meta::mask::UID, S::UID),
        (meta::mask::GID, S::GID),
        (meta::mask::NLINK, S::NLINK),
        (meta::mask::INO, S::INO),
    ];
    pairs.iter().fold(S::empty(), |acc, &(bit, flag)| {
        if mask & bit != 0 {
            acc | flag
        } else {
            acc
        }
    })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn do_stat(
    dirfd: impl rustix::fd::AsFd,
    path: impl rustix::path::Arg,
    follow: bool,
    _mask: u32,
) -> io::Result<Meta> {
    let st = fs::statat(dirfd, path, stat_flags(follow))?;
    Ok(Meta {
        size: st.st_size as u64,
        mtime: st.st_mtime as i64,
        ctime: st.st_ctime as i64,
        atime: st.st_atime as i64,
        mode: st.st_mode as u32,
        uid: st.st_uid,
        gid: st.st_gid,
        nlink: st.st_nlink as u64,
        ino: st.st_ino as u64,
    })
}

fn stat_flags(follow: bool) -> AtFlags {
    if follow {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    }
}

/// `faccessat` for `-readable`/`-writable`/`-executable`. Checks the *real*
/// uid/gid (no `AT_EACCESS`), like find's `access(2)`-based predicates.
pub(crate) fn access_at(dir: &DirFd, name: &OsStr, mode: u8) -> bool {
    do_access(dir, name, mode)
}

pub(crate) fn access_root(path: &Path, mode: u8) -> bool {
    do_access(CWD, path, mode)
}

fn do_access(
    dirfd: impl rustix::fd::AsFd,
    path: impl rustix::path::Arg,
    mode: u8,
) -> bool {
    use rustix::fs::Access;
    let mut acc = Access::empty();
    if mode & meta::access::READ != 0 {
        acc |= Access::READ_OK;
    }
    if mode & meta::access::WRITE != 0 {
        acc |= Access::WRITE_OK;
    }
    if mode & meta::access::EXEC != 0 {
        acc |= Access::EXEC_OK;
    }
    fs::accessat(dirfd, path, acc, AtFlags::empty()).is_ok()
}

/// Reads a symlink's target (for `-lname`), relative to the parent dir fd.
pub(crate) fn readlink_at(dir: &DirFd, name: &OsStr) -> Option<OsString> {
    do_readlink(dir, name)
}

pub(crate) fn readlink_root(path: &Path) -> Option<OsString> {
    do_readlink(CWD, path)
}

fn do_readlink(
    dirfd: impl rustix::fd::AsFd,
    path: impl rustix::path::Arg,
) -> Option<OsString> {
    let target = fs::readlinkat(dirfd, path, Vec::new()).ok()?;
    Some(OsStr::from_bytes(target.to_bytes()).to_owned())
}

/// Resolves a `DT_UNKNOWN` entry's own type via a `statat` relative to its
/// directory fd.
fn statat_type(dir: &DirFd, name: &OsStr) -> io::Result<EntryType> {
    let st = fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::TempDir;

    // Locks the fd-sharing invariant: a sibling stat on the shared fd must not
    // perturb iteration (Dir::read_from iterates its own openat(".") fd).
    #[test]
    fn concurrent_statat_does_not_perturb_iteration() {
        let tmp = TempDir::new().unwrap();
        for i in 0..200 {
            std::fs::write(tmp.path().join(format!("f{i}")), b"x").unwrap();
        }
        let dir = Arc::new(open_root(tmp.path(), false).unwrap());

        let stop = Arc::new(AtomicBool::new(false));
        let hammer = {
            let dir = Arc::clone(&dir);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // statat against the shared description; must not touch
                    // the directory-stream position.
                    let _ = fs::statat(&*dir, "f0", AtFlags::SYMLINK_NOFOLLOW);
                }
            })
        };

        let mut names = Vec::new();
        for_each_entry(&dir, tmp.path(), |path, _leaf, _ty| {
            names.push(path.file_name().unwrap().to_owned());
            true
        })
        .unwrap();

        stop.store(true, Ordering::Relaxed);
        hammer.join().unwrap();

        names.sort();
        names.dedup();
        assert_eq!(names.len(), 200, "iteration set must be unperturbed");
    }
}
