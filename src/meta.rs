// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! Metadata predicates — the find-style filters that require a `stat`:
//! `-size`, `-mtime`/`-ctime`/`-atime` (+ `-mmin`/`-cmin`/`-amin`), `-perm`,
//! `-uid`/`-gid`, and `-user`/`-group`.
//!
//! The predicates here are pure: parsing happens at arg-parse time (so a bad
//! pattern or unknown user errors before any traversal, and `-user`/`-group`
//! resolve to a numeric id once via NSS), and matching runs against a
//! platform-filled [`Meta`]. The walker fetches a `Meta` lazily — only when
//! [`Predicates::is_active`] and only for entries that survived the cheaper
//! type/name/regex filters — using a `statx` with [`Predicates::mask`] limited
//! to the fields the active predicates actually read.

use anyhow::{anyhow, Error};

/// `statx` field-mask bits, translated to platform `StatxFlags` by the leaf.
/// The leaf fills only the requested fields; predicates only read the fields
/// they requested, so unmasked [`Meta`] fields are never observed.
pub mod mask {
    pub const SIZE: u32 = 1 << 0;
    pub const MTIME: u32 = 1 << 1;
    pub const CTIME: u32 = 1 << 2;
    pub const ATIME: u32 = 1 << 3;
    pub const MODE: u32 = 1 << 4;
    pub const UID: u32 = 1 << 5;
    pub const GID: u32 = 1 << 6;
    pub const NLINK: u32 = 1 << 7;
    pub const INO: u32 = 1 << 8;
}

/// Seconds per day / minute, the units for the time predicates.
pub const DAY: i64 = 86_400;
pub const MIN: i64 = 60;

/// `faccessat` mode bits for `-readable`/`-writable`/`-executable`, translated
/// to platform `Access` flags by the leaf. Checked against the *real* uid/gid,
/// like find.
pub mod access {
    pub const READ: u8 = 1 << 0;
    pub const WRITE: u8 = 1 << 1;
    pub const EXEC: u8 = 1 << 2;
}

/// Stat fields a predicate may read. The platform leaf fills the masked ones;
/// the rest are unspecified and never observed.
#[derive(Debug, Clone, Copy)]
pub struct Meta {
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub atime: i64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub ino: u64,
}

/// find's `N` / `+N` / `-N` numeric comparison (`+` = greater, `-` = less).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Exact(i64),
    Greater(i64),
    Less(i64),
}

impl Comparison {
    fn parse(s: &str) -> Result<Self, Error> {
        let (ctor, num): (fn(i64) -> Self, &str) =
            if let Some(n) = s.strip_prefix('+') {
                (Comparison::Greater, n)
            } else if let Some(n) = s.strip_prefix('-') {
                (Comparison::Less, n)
            } else {
                (Comparison::Exact, s)
            };
        let v: i64 =
            num.parse().map_err(|_| anyhow!("invalid number '{s}'"))?;
        Ok(ctor(v))
    }

    fn matches(self, v: i64) -> bool {
        match self {
            Comparison::Exact(n) => v == n,
            Comparison::Greater(n) => v > n,
            Comparison::Less(n) => v < n,
        }
    }
}

/// `-size [+-]?N(c|k|M|G|T)` — a unit suffix is required (no bare 512-byte
/// blocks). The file size is rounded **up** to the unit before comparison, so
/// `-size 1k` matches files of 1..=1024 bytes, like find.
#[derive(Debug, Clone, Copy)]
pub struct SizePred {
    cmp: Comparison,
    unit: u64,
}

impl SizePred {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let Some(last) = s.chars().last() else {
            return Err(anyhow!("empty --size value"));
        };
        let unit = match last {
            'c' => 1,
            'k' => 1 << 10,
            'M' => 1 << 20,
            'G' => 1 << 30,
            'T' => 1 << 40,
            _ => {
                return Err(anyhow!(
                    "--size requires a unit suffix (c/k/M/G/T): '{s}'"
                ))
            }
        };
        let num = &s[..s.len() - last.len_utf8()];
        Ok(SizePred { cmp: Comparison::parse(num)?, unit })
    }

    fn matches(&self, size: u64) -> bool {
        self.cmp.matches(size.div_ceil(self.unit) as i64)
    }
}

#[derive(Debug, Clone, Copy)]
enum TimeField {
    Mtime,
    Ctime,
    Atime,
}

fn time_field_mask(field: TimeField) -> u32 {
    match field {
        TimeField::Mtime => mask::MTIME,
        TimeField::Ctime => mask::CTIME,
        TimeField::Atime => mask::ATIME,
    }
}

/// `-mtime`/`-ctime`/`-atime` (days) and `-mmin`/`-cmin`/`-amin` (minutes).
/// find semantics: elapsed time is integer-divided by the unit (remainder
/// discarded), so `-mtime +1` means "modified at least two days ago".
#[derive(Debug, Clone, Copy)]
pub struct TimePred {
    cmp: Comparison,
    unit_secs: i64,
    field: TimeField,
}

impl TimePred {
    fn new(s: &str, unit_secs: i64, field: TimeField) -> Result<Self, Error> {
        Ok(TimePred { cmp: Comparison::parse(s)?, unit_secs, field })
    }

    pub fn mtime(s: &str, unit_secs: i64) -> Result<Self, Error> {
        Self::new(s, unit_secs, TimeField::Mtime)
    }

    pub fn ctime(s: &str, unit_secs: i64) -> Result<Self, Error> {
        Self::new(s, unit_secs, TimeField::Ctime)
    }

    pub fn atime(s: &str, unit_secs: i64) -> Result<Self, Error> {
        Self::new(s, unit_secs, TimeField::Atime)
    }

    fn matches(&self, m: &Meta, now: i64) -> bool {
        let t = match self.field {
            TimeField::Mtime => m.mtime,
            TimeField::Ctime => m.ctime,
            TimeField::Atime => m.atime,
        };
        self.cmp.matches((now - t) / self.unit_secs)
    }
}

#[derive(Debug, Clone, Copy)]
enum PermKind {
    /// `-perm MODE`: exactly these permission bits.
    Exact,
    /// `-perm -MODE`: all of these bits set.
    AllOf,
    /// `-perm /MODE`: any of these bits set.
    AnyOf,
}

/// `-perm` with find's `/`, `-`, exact tri-state, over an octal or symbolic
/// (`u+w,g-x`) mode.
#[derive(Debug, Clone, Copy)]
pub struct PermPred {
    mode: u32,
    kind: PermKind,
}

impl PermPred {
    pub fn parse(s: &str) -> Result<Self, Error> {
        let (kind, rest) = if let Some(r) = s.strip_prefix('/') {
            (PermKind::AnyOf, r)
        } else if let Some(r) = s.strip_prefix('-') {
            (PermKind::AllOf, r)
        } else {
            (PermKind::Exact, s)
        };
        Ok(PermPred { mode: parse_mode(rest)?, kind })
    }

    fn matches(&self, st_mode: u32) -> bool {
        let m = st_mode & 0o7777;
        match self.kind {
            PermKind::Exact => m == self.mode,
            PermKind::AllOf => (m & self.mode) == self.mode,
            // find: `/000` matches everything.
            PermKind::AnyOf => self.mode == 0 || (m & self.mode) != 0,
        }
    }
}

/// Parses a `-perm` mode: octal (`644`) or symbolic (`u+w,g-x`).
fn parse_mode(s: &str) -> Result<u32, Error> {
    if s.is_empty() {
        return Err(anyhow!("empty --perm mode"));
    }
    if s.bytes().all(|b| b.is_ascii_digit() && b <= b'7') {
        let m = u32::from_str_radix(s, 8)
            .map_err(|_| anyhow!("invalid octal mode '{s}'"))?;
        if m > 0o7777 {
            return Err(anyhow!("octal mode out of range '{s}'"));
        }
        return Ok(m);
    }
    parse_symbolic_mode(s)
}

/// Builds a numeric mask from a comma-separated symbolic mode applied to a
/// zero base (e.g. `u+w` → 0o200, `a+rx` → 0o555, `u+s` → 0o4100).
fn parse_symbolic_mode(s: &str) -> Result<u32, Error> {
    let mut mode = 0u32;
    for clause in s.split(',') {
        let bytes = clause.as_bytes();
        let mut i = 0;
        // who: bit 4 = user, 2 = group, 1 = other
        let mut who = 0u8;
        while let Some(&b) = bytes.get(i) {
            match b {
                b'u' => who |= 0b100,
                b'g' => who |= 0b010,
                b'o' => who |= 0b001,
                b'a' => who |= 0b111,
                _ => break,
            }
            i += 1;
        }
        if who == 0 {
            who = 0b111; // no who given → all, like chmod
        }
        let op = match bytes.get(i) {
            Some(b @ (b'+' | b'-' | b'=')) => *b,
            _ => {
                return Err(anyhow!(
                    "expected +, - or = in --perm clause '{clause}'"
                ))
            }
        };
        i += 1;
        let (mut r, mut w, mut x, mut s, mut t) =
            (false, false, false, false, false);
        while let Some(&b) = bytes.get(i) {
            match b {
                b'r' => r = true,
                b'w' => w = true,
                b'x' => x = true,
                b's' => s = true,
                b't' => t = true,
                _ => {
                    return Err(anyhow!(
                        "invalid permission char in --perm clause '{clause}'"
                    ))
                }
            }
            i += 1;
        }
        let rwx = u32::from(r) << 2 | u32::from(w) << 1 | u32::from(x);
        let mut bits = 0u32;
        if who & 0b100 != 0 {
            bits |= rwx << 6;
            if s {
                bits |= 0o4000;
            }
        }
        if who & 0b010 != 0 {
            bits |= rwx << 3;
            if s {
                bits |= 0o2000;
            }
        }
        if who & 0b001 != 0 {
            bits |= rwx;
        }
        if t {
            bits |= 0o1000;
        }
        match op {
            b'+' => mode |= bits,
            b'-' => mode &= !bits,
            b'=' => {
                let mut clear = 0u32;
                if who & 0b100 != 0 {
                    clear |= 0o700 | 0o4000;
                }
                if who & 0b010 != 0 {
                    clear |= 0o070 | 0o2000;
                }
                if who & 0b001 != 0 {
                    clear |= 0o007 | 0o1000;
                }
                mode = (mode & !clear) | bits;
            }
            _ => unreachable!(),
        }
    }
    Ok(mode)
}

/// A numeric `+N`/`-N`/`N` predicate over a single unsigned field. Backs
/// `-uid`/`-gid` (and the resolved id behind `-user`/`-group`), `-links`
/// (nlink), and `-inum` (inode).
#[derive(Debug, Clone, Copy)]
pub struct IdPred {
    cmp: Comparison,
}

impl IdPred {
    pub fn parse(s: &str) -> Result<Self, Error> {
        Ok(IdPred { cmp: Comparison::parse(s)? })
    }

    pub fn exact(id: u32) -> Self {
        IdPred { cmp: Comparison::Exact(i64::from(id)) }
    }

    fn matches(&self, value: u64) -> bool {
        self.cmp.matches(value as i64)
    }
}

/// `-newer FILE` / `-anewer` / `-cnewer`: the entry's m/a/c-time is strictly
/// newer than the reference file's **modification** time (captured once at
/// parse). find compares whole seconds.
#[derive(Debug, Clone, Copy)]
pub struct NewerPred {
    ref_mtime: i64,
    field: TimeField,
}

impl NewerPred {
    pub fn newer(ref_mtime: i64) -> Self {
        NewerPred { ref_mtime, field: TimeField::Mtime }
    }

    pub fn anewer(ref_mtime: i64) -> Self {
        NewerPred { ref_mtime, field: TimeField::Atime }
    }

    pub fn cnewer(ref_mtime: i64) -> Self {
        NewerPred { ref_mtime, field: TimeField::Ctime }
    }

    fn matches(&self, m: &Meta) -> bool {
        let t = match self.field {
            TimeField::Mtime => m.mtime,
            TimeField::Ctime => m.ctime,
            TimeField::Atime => m.atime,
        };
        t > self.ref_mtime
    }
}

/// Modification time (whole seconds since the epoch) of `path`, the reference
/// for the `-newer` family. Errors if the file can't be stat'd.
pub fn file_mtime(path: &std::path::Path) -> Result<i64, Error> {
    use std::time::UNIX_EPOCH;
    let mtime =
        std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| {
            anyhow!("cannot stat reference file '{}': {e}", path.display())
        })?;
    Ok(mtime.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64))
}

/// All active metadata predicates for one run. Built at arg-parse; the walker
/// consults it only when [`is_active`](Self::is_active).
#[derive(Debug, Default, Clone)]
pub struct Predicates {
    pub size: Option<SizePred>,
    pub times: Vec<TimePred>,
    pub perm: Option<PermPred>,
    pub uid: Option<IdPred>,
    pub gid: Option<IdPred>,
    pub links: Option<IdPred>,
    pub inum: Option<IdPred>,
    pub newer: Vec<NewerPred>,
    /// `-nouser` / `-nogroup`: the uid/gid resolves to no NSS entry. Evaluated
    /// in the visitor (needs a reverse lookup with a per-thread cache), not in
    /// [`matches`](Self::matches); `mask` still requests the id field.
    pub nouser: bool,
    pub nogroup: bool,
}

impl Predicates {
    /// Whether any predicate is set (gates the entire stat path).
    pub fn is_active(&self) -> bool {
        self.size.is_some()
            || !self.times.is_empty()
            || self.perm.is_some()
            || self.uid.is_some()
            || self.gid.is_some()
            || self.links.is_some()
            || self.inum.is_some()
            || !self.newer.is_empty()
            || self.nouser
            || self.nogroup
    }

    /// The `statx` field mask covering exactly the active predicates.
    pub fn mask(&self) -> u32 {
        let mut m = 0;
        if self.size.is_some() {
            m |= mask::SIZE;
        }
        for t in &self.times {
            m |= time_field_mask(t.field);
        }
        for n in &self.newer {
            m |= time_field_mask(n.field);
        }
        if self.perm.is_some() {
            m |= mask::MODE;
        }
        if self.uid.is_some() || self.nouser {
            m |= mask::UID;
        }
        if self.gid.is_some() || self.nogroup {
            m |= mask::GID;
        }
        if self.links.is_some() {
            m |= mask::NLINK;
        }
        if self.inum.is_some() {
            m |= mask::INO;
        }
        m
    }

    /// Whether `meta` satisfies every active predicate (`now` = run start, for
    /// the time predicates).
    pub fn matches(&self, meta: &Meta, now: i64) -> bool {
        if let Some(s) = &self.size {
            if !s.matches(meta.size) {
                return false;
            }
        }
        for t in &self.times {
            if !t.matches(meta, now) {
                return false;
            }
        }
        if let Some(p) = &self.perm {
            if !p.matches(meta.mode) {
                return false;
            }
        }
        if let Some(u) = &self.uid {
            if !u.matches(u64::from(meta.uid)) {
                return false;
            }
        }
        if let Some(g) = &self.gid {
            if !g.matches(u64::from(meta.gid)) {
                return false;
            }
        }
        if let Some(l) = &self.links {
            if !l.matches(meta.nlink) {
                return false;
            }
        }
        if let Some(i) = &self.inum {
            if !i.matches(meta.ino) {
                return false;
            }
        }
        for n in &self.newer {
            if !n.matches(meta) {
                return false;
            }
        }
        true
    }
}

/// Wall-clock now in whole seconds since the Unix epoch — captured once at run
/// start as the reference for the time predicates.
pub fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Resolves `-user` to a uid: NSS name lookup first, then a numeric fallback.
#[cfg(unix)]
pub fn resolve_user(name: &str) -> Result<u32, Error> {
    if let Some(uid) = nss_uid(name) {
        Ok(uid)
    } else if let Ok(n) = name.parse::<u32>() {
        Ok(n)
    } else {
        Err(anyhow!("unknown user '{name}'"))
    }
}

/// Resolves `-group` to a gid: NSS name lookup first, then a numeric fallback.
#[cfg(unix)]
pub fn resolve_group(name: &str) -> Result<u32, Error> {
    if let Some(gid) = nss_gid(name) {
        Ok(gid)
    } else if let Ok(n) = name.parse::<u32>() {
        Ok(n)
    } else {
        Err(anyhow!("unknown group '{name}'"))
    }
}

/// `getpwnam_r` (reentrant) → uid, or `None` if the name is unknown.
#[cfg(unix)]
fn nss_uid(name: &str) -> Option<u32> {
    use std::ffi::CString;
    let cname = CString::new(name).ok()?;
    // SAFETY: `passwd` is repr(C) of ints and nullable pointers; all-zero is a
    // valid initial state that getpwnam_r overwrites on success.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    loop {
        // SAFETY: valid NUL-terminated name, valid out-pointers, live owned
        // buffer; getpwnam_r is reentrant (safe under parallelism).
        let rc = unsafe {
            libc::getpwnam_r(
                cname.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            return (!result.is_null()).then_some(pwd.pw_uid);
        }
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return None;
    }
}

/// `getgrnam_r` (reentrant) → gid, or `None` if the name is unknown.
#[cfg(unix)]
fn nss_gid(name: &str) -> Option<u32> {
    use std::ffi::CString;
    let cname = CString::new(name).ok()?;
    // SAFETY: `group` is repr(C) of ints and nullable pointers; all-zero is a
    // valid initial state that getgrnam_r overwrites on success.
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::group = std::ptr::null_mut();
    loop {
        // SAFETY: valid NUL-terminated name, valid out-pointers, live owned
        // buffer; getgrnam_r is reentrant (safe under parallelism).
        let rc = unsafe {
            libc::getgrnam_r(
                cname.as_ptr(),
                &mut grp,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            return (!result.is_null()).then_some(grp.gr_gid);
        }
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return None;
    }
}

/// Per-walker-thread memo for the reverse lookups behind `-nouser`/`-nogroup`.
/// Owner ids repeat heavily within a tree, so caching `uid/gid → has-entry`
/// turns the per-entry `getpwuid_r`/`getgrgid_r` into one lookup per distinct
/// id. One cache per thread keeps it lock-free.
#[cfg(unix)]
#[derive(Default)]
pub struct NssCache {
    users: std::collections::HashMap<u32, bool>,
    groups: std::collections::HashMap<u32, bool>,
}

#[cfg(unix)]
impl NssCache {
    /// Whether `uid` resolves to a passwd entry (`-nouser` matches when not).
    pub fn user_exists(&mut self, uid: u32) -> bool {
        *self.users.entry(uid).or_insert_with(|| nss_user_exists(uid))
    }

    /// Whether `gid` resolves to a group entry (`-nogroup` matches when not).
    pub fn group_exists(&mut self, gid: u32) -> bool {
        *self.groups.entry(gid).or_insert_with(|| nss_group_exists(gid))
    }
}

/// `getpwuid_r` (reentrant): does a passwd entry exist for `uid`?
#[cfg(unix)]
fn nss_user_exists(uid: u32) -> bool {
    // SAFETY: `passwd` is repr(C) of ints and nullable pointers; all-zero is a
    // valid initial state that getpwuid_r overwrites on success.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    loop {
        // SAFETY: valid out-pointers and a live owned buffer; getpwuid_r is
        // reentrant (safe under parallelism).
        let rc = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            return !result.is_null();
        }
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        // On lookup error, assume the id resolves (so `-nouser` stays
        // conservative and does not over-match).
        return true;
    }
}

/// `getgrgid_r` (reentrant): does a group entry exist for `gid`?
#[cfg(unix)]
fn nss_group_exists(gid: u32) -> bool {
    // SAFETY: `group` is repr(C) of ints and nullable pointers; all-zero is a
    // valid initial state that getgrgid_r overwrites on success.
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 1024];
    let mut result: *mut libc::group = std::ptr::null_mut();
    loop {
        // SAFETY: valid out-pointers and a live owned buffer; getgrgid_r is
        // reentrant (safe under parallelism).
        let rc = unsafe {
            libc::getgrgid_r(
                gid,
                &mut grp,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            return !result.is_null();
        }
        if rc == libc::ERANGE {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            size: 0,
            mtime: 0,
            ctime: 0,
            atime: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            nlink: 0,
            ino: 0,
        }
    }

    #[test]
    fn comparison_tristate() {
        assert!(Comparison::parse("5").unwrap().matches(5));
        assert!(!Comparison::parse("5").unwrap().matches(6));
        assert!(Comparison::parse("+5").unwrap().matches(6));
        assert!(!Comparison::parse("+5").unwrap().matches(5));
        assert!(Comparison::parse("-5").unwrap().matches(4));
        assert!(!Comparison::parse("-5").unwrap().matches(5));
        assert!(Comparison::parse("bad").is_err());
    }

    #[test]
    fn size_requires_unit_suffix() {
        assert!(SizePred::parse("10").is_err());
        assert!(SizePred::parse("+10").is_err());
        assert!(SizePred::parse("10x").is_err());
    }

    #[test]
    fn size_units_and_rounding() {
        // bytes: exact comparison, no rounding
        assert!(SizePred::parse("100c").unwrap().matches(100));
        assert!(!SizePred::parse("100c").unwrap().matches(101));
        assert!(SizePred::parse("+100c").unwrap().matches(101));
        assert!(SizePred::parse("-100c").unwrap().matches(99));
        // 1k = 1024 bytes; size rounds UP to the unit
        let k = SizePred::parse("1k").unwrap();
        assert!(k.matches(1)); // 1 byte rounds up to 1 KiB
        assert!(k.matches(1024));
        assert!(!k.matches(1025)); // rounds up to 2 KiB
                                   // suffix scale is 1024-based
        assert!(SizePred::parse("+1M").unwrap().matches((1 << 20) + 1));
        assert!(!SizePred::parse("+1M").unwrap().matches(1 << 20));
    }

    #[test]
    fn time_find_day_semantics() {
        let mut m = meta();
        let now = 100 * DAY;
        // modified exactly 2 days ago
        m.mtime = now - 2 * DAY;
        assert!(TimePred::mtime("2", DAY).unwrap().matches(&m, now));
        // "+1" means at least two days ago
        assert!(TimePred::mtime("+1", DAY).unwrap().matches(&m, now));
        // modified 12h ago: 0 days elapsed → matches "-1", not "1"
        m.mtime = now - DAY / 2;
        assert!(TimePred::mtime("-1", DAY).unwrap().matches(&m, now));
        assert!(!TimePred::mtime("1", DAY).unwrap().matches(&m, now));
    }

    #[test]
    fn time_minutes_and_fields() {
        let mut m = meta();
        let now = 10_000;
        m.atime = now - 5 * MIN;
        assert!(TimePred::atime("5", MIN).unwrap().matches(&m, now));
        m.ctime = now - 3 * MIN;
        assert!(TimePred::ctime("-5", MIN).unwrap().matches(&m, now));
    }

    #[test]
    fn links_predicate_over_nlink() {
        let mut m = meta();
        m.nlink = 2;
        let p = Predicates {
            links: Some(IdPred::parse("2").unwrap()),
            ..meta_p()
        };
        assert!(p.matches(&m, 0));
        m.nlink = 3;
        assert!(!p.matches(&m, 0));
    }

    #[test]
    fn inum_predicate_over_ino() {
        let mut m = meta();
        m.ino = 4096;
        let p = Predicates {
            inum: Some(IdPred::parse("+4000").unwrap()),
            ..meta_p()
        };
        assert!(p.matches(&m, 0));
        m.ino = 100;
        assert!(!p.matches(&m, 0));
    }

    #[test]
    fn newer_compares_against_reference_mtime() {
        let mut m = meta();
        let p = Predicates { newer: vec![NewerPred::newer(1000)], ..meta_p() };
        m.mtime = 1001;
        assert!(p.matches(&m, 0));
        m.mtime = 1000; // strictly newer required
        assert!(!p.matches(&m, 0));
    }

    #[test]
    fn newer_mask_requests_the_right_field() {
        let p = Predicates { newer: vec![NewerPred::anewer(0)], ..meta_p() };
        assert_eq!(p.mask(), mask::ATIME);
    }

    #[cfg(unix)]
    #[test]
    fn nss_cache_resolves_root_and_misses_high_id() {
        let mut cache = NssCache::default();
        assert!(cache.user_exists(0)); // root exists on every unix
        assert!(!cache.user_exists(4_000_000_000)); // no such uid
    }

    /// A `Predicates` with everything empty, for `..` struct-update in tests.
    fn meta_p() -> Predicates {
        Predicates::default()
    }

    #[test]
    fn perm_octal_tristate() {
        let exact = PermPred::parse("644").unwrap();
        assert!(exact.matches(0o644));
        assert!(!exact.matches(0o645));

        let all = PermPred::parse("-644").unwrap();
        assert!(all.matches(0o644));
        assert!(all.matches(0o744)); // superset
        assert!(!all.matches(0o600)); // missing group/other read

        let any = PermPred::parse("/022").unwrap();
        assert!(any.matches(0o020)); // group-write only
        assert!(any.matches(0o002)); // other-write only
        assert!(!any.matches(0o644)); // neither write bit
    }

    #[test]
    fn perm_symbolic() {
        assert!(PermPred::parse("u+w").unwrap().matches(0o200));
        assert!(PermPred::parse("-u+w").unwrap().matches(0o600)); // all-of u+w
        assert!(!PermPred::parse("-u+w").unwrap().matches(0o400));
        // a+rx = 0o555
        let arx = PermPred::parse("a+rx").unwrap();
        assert!(arx.matches(0o555));
        // setuid + user-execute
        assert!(PermPred::parse("u+s").unwrap().matches(0o4000));
        // sticky bit
        assert!(PermPred::parse("/+t").unwrap().matches(0o1000));
        assert!(PermPred::parse("bad?").is_err());
    }

    #[test]
    fn id_predicates() {
        assert!(IdPred::parse("1000").unwrap().matches(1000));
        assert!(IdPred::parse("+1000").unwrap().matches(1001));
        assert!(IdPred::exact(0).matches(0));
        assert!(!IdPred::exact(0).matches(1));
    }

    #[test]
    fn predicates_mask_and_and_logic() {
        let p = Predicates {
            size: Some(SizePred::parse("+0c").unwrap()),
            times: vec![TimePred::mtime("-1", DAY).unwrap()],
            uid: Some(IdPred::exact(0)),
            ..Default::default()
        };
        assert!(p.is_active());
        assert_eq!(p.mask(), mask::SIZE | mask::MTIME | mask::UID);
        assert!(!Predicates::default().is_active());

        let mut m = meta();
        m.size = 10;
        m.uid = 0;
        m.mtime = 0;
        // all three hold (size>0, mtime recent, uid 0)
        assert!(p.matches(&m, DAY / 2));
        // break the uid predicate
        m.uid = 5;
        assert!(!p.matches(&m, DAY / 2));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_user_root_and_numeric() {
        // root is uid 0 on every unix
        assert_eq!(resolve_user("root").unwrap(), 0);
        // unknown name falls back to a numeric id
        assert_eq!(resolve_user("4242").unwrap(), 4242);
        assert!(resolve_user("definitely-no-such-user-xyz").is_err());
    }
}
