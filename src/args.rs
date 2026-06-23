// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

use crate::meta;
use anyhow::{anyhow, Error};
use lexopt::prelude::*;
use normpath::PathExt;
use std::path::{Path, PathBuf};
use std::thread;

/// Parsed CLI configuration. Fields are public with a deliberately stable
/// shape so the pipeline and tests can build it by struct literal.
#[derive(Debug, Default, Clone)]
pub struct Args {
    /// Follow symlinks (`-f`/`-L`/`--follow-symlinks`).
    pub follow_symlinks: bool,

    /// Do not cross mount points (`-o`/`--one-filesystem`/`--xdev`); disabled
    /// by `--no-one-filesystem`/`--cross-filesystem`.
    pub one_filesystem: bool,

    /// Number of worker threads (`-x`/`--threads`). With `--idle` and no
    /// explicit `--threads`, this defaults to 2 instead of the CPU count.
    pub threads: usize,

    /// Run unobtrusively (`--idle`, Linux): the walker pool runs in the
    /// SCHED_IDLE CPU class, the process nice is lowered to +19, and the
    /// default thread count drops to 2.
    pub idle: bool,

    /// Maximum depth to traverse (`-d`/`--max-depth`).
    pub max_depth: Option<usize>,

    /// Minimum depth to emit (`--min-depth`/`-mindepth`); shallower entries
    /// are suppressed but still descended through. Root is depth 0.
    pub min_depth: Option<usize>,

    /// Max directories scanned per second (`-s`/`--max-scan-rate`);
    /// `None`/`0` = unlimited.
    pub max_scan_rate: Option<u32>,

    /// Stop after emitting this many results (`--max-results`); `None`/`0` =
    /// unlimited. Enforced in the output thread; the walk halts early once the
    /// output channel closes.
    pub max_results: Option<usize>,

    /// Base of the file name matching globbing pattern (`-n`/`--name`).
    pub name: Option<Vec<String>>,

    /// File name (full path) matching regular expression (`-r`/`--regex`).
    pub regex: Option<Vec<String>>,

    /// Case-insensitive matching (`-i`/`--case-insensitive`).
    pub case_insensitive: bool,

    /// Filter matches by type (`-t`/`--file-type`).
    pub file_type: Vec<FileType>,

    /// Metadata predicates requiring a `stat` (`-size`/`-mtime`/`-perm`/…),
    /// parsed (and `-user`/`-group` resolved to ids) at arg-parse time.
    pub meta: meta::Predicates,

    /// Glob patterns matched against the **full path** (`-path`/`-wholename`).
    pub path_glob: Option<Vec<String>>,

    /// Glob patterns matched against a symlink's **target** (`-lname`).
    pub lname: Option<Vec<String>>,

    /// `faccessat` mode bits for `-readable`/`-writable`/`-executable`
    /// (see [`meta::access`]); 0 = no access check.
    pub access: u8,

    /// Glob patterns whose matching entries (by file name) are excluded
    /// (`-E`/`--exclude`); a matched directory is pruned (not descended).
    pub exclude: Option<Vec<String>>,

    /// Terminate each printed path with a NUL byte instead of a newline
    /// (`-0`/`--null`/`-print0`); for piping into `xargs -0` and friends.
    pub null: bool,

    /// Paths to traverse (positional; at least one required).
    pub path: Vec<PathBuf>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FileType {
    Empty,
    BlockDevice,
    CharDevice,
    Directory,
    Pipe,
    File,
    Symlink,
    Socket,
}

/// Result of parsing argv: either a request to print help/version, or the
/// fully-populated [`Args`] to run with. Kept separate from process exit so
/// [`parse_inner`] stays unit-testable.
#[derive(Debug)]
pub enum Outcome {
    Help,
    Version,
    // boxed: `Args` is much larger than the unit variants
    Run(Box<Args>),
}

const HELP: &str = "\
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
      --links <[+-]N>      Filter by hard-link count [alias: -links]
      --inum <[+-]N>       Filter by inode number [alias: -inum]
      --newer, --anewer, --cnewer <FILE>  Entry's m/a/c-time is newer than FILE's mtime [aliases: -newer/-anewer/-cnewer]
      --nouser, --nogroup  Owner uid/gid resolves to no passwd/group entry [aliases: -nouser/-nogroup]
      --path, --wholename <GLOB>  Glob over the full path (* crosses /) [aliases: -path/-wholename; -ipath/-iwholename add -i]
      --lname <GLOB>       Glob over a symlink's target [alias: -lname; -ilname adds -i]
      --readable, --writable, --executable  Filter by access (real uid/gid) [aliases: -readable/-writable/-executable]
      --quit               Stop after the first match (= --max-results 1) [alias: -quit]
      --idle               Run unobtrusively: SCHED_IDLE worker pool, nice +19, 2 threads (Linux)
  -h, --help               Print help
  -V, --version            Print version
";

impl Args {
    /// Parses process arguments. Prints help/version to stdout and exits 0;
    /// prints usage errors to stderr and exits 2; otherwise returns [`Args`].
    #[must_use]
    pub fn parse() -> Args {
        match parse_inner(std::env::args_os()) {
            Ok(Outcome::Run(args)) => *args,
            Ok(Outcome::Help) => {
                print!("{HELP}");
                std::process::exit(0);
            }
            Ok(Outcome::Version) => {
                println!(
                    "{} {}",
                    env!("CARGO_PKG_NAME"),
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("minifind: {e}");
                std::process::exit(2);
            }
        }
    }
}

/// Default worker-thread count: logical CPU count, falling back to 2.
fn default_threads() -> usize {
    thread::available_parallelism().map(|n| n.get()).unwrap_or(2)
}

/// Pure parser over any argv-like iterator (first item is the binary name).
/// Process-exit-free so it can be unit-tested directly.
fn parse_inner<I>(args: I) -> Result<Outcome, Error>
where
    I: IntoIterator,
    I::Item: Into<std::ffi::OsString>,
{
    // find spells these as single multi-char short tokens that lexopt would
    // split into `-p -r -i …`; rewrite each to its `--long` form first.
    let normalized = args.into_iter().map(|a| {
        let os: std::ffi::OsString = a.into();
        match os.to_str() {
            Some("-print0") => std::ffi::OsString::from("--null"),
            Some("-mindepth") => std::ffi::OsString::from("--min-depth"),
            Some("-size") => std::ffi::OsString::from("--size"),
            Some("-mtime") => std::ffi::OsString::from("--mtime"),
            Some("-ctime") => std::ffi::OsString::from("--ctime"),
            Some("-atime") => std::ffi::OsString::from("--atime"),
            Some("-mmin") => std::ffi::OsString::from("--mmin"),
            Some("-cmin") => std::ffi::OsString::from("--cmin"),
            Some("-amin") => std::ffi::OsString::from("--amin"),
            Some("-perm") => std::ffi::OsString::from("--perm"),
            Some("-uid") => std::ffi::OsString::from("--uid"),
            Some("-gid") => std::ffi::OsString::from("--gid"),
            Some("-user") => std::ffi::OsString::from("--user"),
            Some("-group") => std::ffi::OsString::from("--group"),
            Some("-name") => std::ffi::OsString::from("--name"),
            Some("-iname") => std::ffi::OsString::from("--iname"),
            Some("-regex") => std::ffi::OsString::from("--regex"),
            Some("-iregex") => std::ffi::OsString::from("--iregex"),
            Some("-type") => std::ffi::OsString::from("--file-type"),
            Some("-maxdepth") => std::ffi::OsString::from("--max-depth"),
            Some("-xdev" | "-mount") => std::ffi::OsString::from("--xdev"),
            Some("-follow") => std::ffi::OsString::from("--follow-symlinks"),
            Some("-empty") => std::ffi::OsString::from("--empty"),
            Some("-links") => std::ffi::OsString::from("--links"),
            Some("-inum") => std::ffi::OsString::from("--inum"),
            Some("-newer") => std::ffi::OsString::from("--newer"),
            Some("-anewer") => std::ffi::OsString::from("--anewer"),
            Some("-cnewer") => std::ffi::OsString::from("--cnewer"),
            Some("-path") => std::ffi::OsString::from("--path"),
            Some("-wholename") => std::ffi::OsString::from("--wholename"),
            Some("-ipath") => std::ffi::OsString::from("--ipath"),
            Some("-iwholename") => std::ffi::OsString::from("--iwholename"),
            Some("-lname") => std::ffi::OsString::from("--lname"),
            Some("-ilname") => std::ffi::OsString::from("--ilname"),
            Some("-readable") => std::ffi::OsString::from("--readable"),
            Some("-writable") => std::ffi::OsString::from("--writable"),
            Some("-executable") => std::ffi::OsString::from("--executable"),
            Some("-nouser") => std::ffi::OsString::from("--nouser"),
            Some("-nogroup") => std::ffi::OsString::from("--nogroup"),
            Some("-quit") => std::ffi::OsString::from("--quit"),
            Some("-print") => std::ffi::OsString::from("--print"),
            Some("-ignore_readdir_race") => {
                std::ffi::OsString::from("--ignore-readdir-race")
            }
            Some("-noignore_readdir_race") => {
                std::ffi::OsString::from("--noignore-readdir-race")
            }
            _ => os,
        }
    });
    let mut parser = lexopt::Parser::from_iter(normalized);

    let mut follow_symlinks = false;
    let mut one_filesystem = true;
    // None until --threads is given, so --idle can pick a different default.
    let mut threads: Option<usize> = None;
    // Only Linux has a parse arm for --idle, so keep it immutable elsewhere.
    #[cfg(target_os = "linux")]
    let mut idle = false;
    #[cfg(not(target_os = "linux"))]
    let idle = false;
    let mut max_depth = None;
    let mut min_depth = None;
    let mut max_scan_rate = None;
    let mut max_results = None;
    let mut name: Vec<String> = Vec::new();
    let mut regex: Vec<String> = Vec::new();
    let mut case_insensitive = false;
    let mut file_type: Vec<FileType> = Vec::new();
    let mut exclude: Vec<String> = Vec::new();
    let mut null = false;
    let mut meta = meta::Predicates::default();
    let mut path_glob: Vec<String> = Vec::new();
    let mut lname: Vec<String> = Vec::new();
    let mut access: u8 = 0;
    let mut path: Vec<PathBuf> = Vec::new();

    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => return Ok(Outcome::Help),
            Short('V') | Long("version") => return Ok(Outcome::Version),
            Short('f') | Short('L') | Long("follow-symlinks") => {
                follow_symlinks = true;
            }
            Short('o') | Long("one-filesystem") | Long("xdev") => {
                one_filesystem = true;
            }
            Long("no-one-filesystem") | Long("cross-filesystem") => {
                one_filesystem = false;
            }
            Short('x') | Long("threads") => {
                threads = Some(parse_threads(&parser.value()?.string()?)?);
            }
            #[cfg(target_os = "linux")]
            Long("idle") => {
                idle = true;
            }
            Short('d') | Long("max-depth") => {
                max_depth = Some(parser.value()?.parse()?);
            }
            Long("min-depth") => {
                min_depth = Some(parser.value()?.parse()?);
            }
            Short('s') | Long("max-scan-rate") => {
                max_scan_rate = Some(parser.value()?.parse()?);
            }
            Long("max-results") => {
                max_results = Some(parser.value()?.parse()?);
            }
            Short('n') | Long("name") => {
                name.push(parser.value()?.string()?);
            }
            Short('r') | Long("regex") => {
                regex.push(parser.value()?.string()?);
            }
            Short('i') | Long("case-insensitive") => {
                case_insensitive = true;
            }
            // -iname/-iregex = matcher + case-insensitivity; since --name and
            // --regex are exclusive, the global flag affects only the one used
            Long("iname") => {
                case_insensitive = true;
                name.push(val_str(&mut parser)?);
            }
            Long("iregex") => {
                case_insensitive = true;
                regex.push(val_str(&mut parser)?);
            }
            Short('t') | Long("file-type") => {
                file_type.push(parse_file_type(&parser.value()?.string()?)?);
            }
            // find's `-empty` predicate; equivalent to `--file-type empty`
            Long("empty") => file_type.push(FileType::Empty),
            Short('E') | Long("exclude") => {
                exclude.push(parser.value()?.string()?);
            }
            // Parsed now so a bad pattern errors before the walk. size/time
            // work everywhere; mode/owner are Unix-only (no mode bits on the
            // fallback leaf), so absent off-Unix → "unexpected option".
            Long("size") => {
                meta.size =
                    Some(meta::SizePred::parse(&parser.value()?.string()?)?);
            }
            Long("mtime") => meta.times.push(meta::TimePred::mtime(
                &val_str(&mut parser)?,
                meta::DAY,
            )?),
            Long("ctime") => meta.times.push(meta::TimePred::ctime(
                &val_str(&mut parser)?,
                meta::DAY,
            )?),
            Long("atime") => meta.times.push(meta::TimePred::atime(
                &val_str(&mut parser)?,
                meta::DAY,
            )?),
            Long("mmin") => meta.times.push(meta::TimePred::mtime(
                &val_str(&mut parser)?,
                meta::MIN,
            )?),
            Long("cmin") => meta.times.push(meta::TimePred::ctime(
                &val_str(&mut parser)?,
                meta::MIN,
            )?),
            Long("amin") => meta.times.push(meta::TimePred::atime(
                &val_str(&mut parser)?,
                meta::MIN,
            )?),
            #[cfg(unix)]
            Long("perm") => {
                meta.perm =
                    Some(meta::PermPred::parse(&parser.value()?.string()?)?);
            }
            #[cfg(unix)]
            Long("uid") => {
                meta.uid =
                    Some(meta::IdPred::parse(&parser.value()?.string()?)?);
            }
            #[cfg(unix)]
            Long("gid") => {
                meta.gid =
                    Some(meta::IdPred::parse(&parser.value()?.string()?)?);
            }
            #[cfg(unix)]
            Long("user") => {
                let id = meta::resolve_user(&parser.value()?.string()?)?;
                meta.uid = Some(meta::IdPred::exact(id));
            }
            #[cfg(unix)]
            Long("group") => {
                let id = meta::resolve_group(&parser.value()?.string()?)?;
                meta.gid = Some(meta::IdPred::exact(id));
            }
            // -links/-inum read Unix stat fields (nlink/ino); Unix-only.
            #[cfg(unix)]
            Long("links") => {
                meta.links =
                    Some(meta::IdPred::parse(&parser.value()?.string()?)?);
            }
            #[cfg(unix)]
            Long("inum") => {
                meta.inum =
                    Some(meta::IdPred::parse(&parser.value()?.string()?)?);
            }
            // -newer family: stat the reference file once, here.
            Long("newer") => meta.newer.push(meta::NewerPred::newer(
                meta::file_mtime(Path::new(&parser.value()?))?,
            )),
            Long("anewer") => meta.newer.push(meta::NewerPred::anewer(
                meta::file_mtime(Path::new(&parser.value()?))?,
            )),
            Long("cnewer") => meta.newer.push(meta::NewerPred::cnewer(
                meta::file_mtime(Path::new(&parser.value()?))?,
            )),
            // -nouser/-nogroup need reverse NSS; Unix-only.
            #[cfg(unix)]
            Long("nouser") => meta.nouser = true,
            #[cfg(unix)]
            Long("nogroup") => meta.nogroup = true,
            // full-path globs; -ipath/-iwholename add case-insensitivity
            Long("path") | Long("wholename") => {
                path_glob.push(val_str(&mut parser)?);
            }
            Long("ipath") | Long("iwholename") => {
                case_insensitive = true;
                path_glob.push(val_str(&mut parser)?);
            }
            // symlink-target globs; -ilname adds case-insensitivity
            Long("lname") => lname.push(val_str(&mut parser)?),
            Long("ilname") => {
                case_insensitive = true;
                lname.push(val_str(&mut parser)?);
            }
            // access checks via faccessat (real uid/gid); Unix-only.
            #[cfg(unix)]
            Long("readable") => access |= meta::access::READ,
            #[cfg(unix)]
            Long("writable") => access |= meta::access::WRITE,
            #[cfg(unix)]
            Long("executable") => access |= meta::access::EXEC,
            // -quit: stop after the first match (= --max-results 1).
            Long("quit") => max_results = Some(1),
            // no-ops: minifind always prints and already skips readdir races.
            Long("print")
            | Long("ignore-readdir-race")
            | Long("noignore-readdir-race") => {}
            // `-print0` is rewritten to `--null` above; `--print0` (fd-style)
            // and `-0` (xargs/grep-style) are accepted directly.
            Short('0') | Long("null") | Long("print0") => {
                null = true;
            }
            Value(val) => path.push(parse_paths(&val.string()?)?),
            _ => return Err(arg.unexpected().into()),
        }
    }

    if !name.is_empty() && !regex.is_empty() {
        return Err(anyhow!(
            "the argument '--name' cannot be used with '--regex'"
        ));
    }

    if path.is_empty() {
        return Err(anyhow!(
            "the following required arguments were not provided: <PATH>..."
        ));
    }

    if file_type.is_empty() {
        file_type =
            vec![FileType::Directory, FileType::File, FileType::Symlink];
    }

    // --idle defaults to 2 threads; an explicit --threads always wins.
    // unwrap_or_else so default_threads() (an OS query) is skipped when set.
    let threads =
        threads.unwrap_or_else(|| if idle { 2 } else { default_threads() });

    Ok(Outcome::Run(Box::new(Args {
        follow_symlinks,
        one_filesystem,
        threads,
        idle,
        max_depth,
        min_depth,
        max_scan_rate,
        max_results,
        name: (!name.is_empty()).then_some(name),
        regex: (!regex.is_empty()).then_some(regex),
        case_insensitive,
        file_type,
        meta,
        path_glob: (!path_glob.is_empty()).then_some(path_glob),
        lname: (!lname.is_empty()).then_some(lname),
        access,
        exclude: (!exclude.is_empty()).then_some(exclude),
        null,
        path,
    })))
}

/// Next option value as a `String`; keeps the metadata parse arms terse.
fn val_str(parser: &mut lexopt::Parser) -> Result<String, Error> {
    Ok(parser.value()?.string()?)
}

/// Parses a `--file-type` value: a canonical name or its single-char alias.
fn parse_file_type(s: &str) -> Result<FileType, Error> {
    let ft = match s {
        "empty" | "e" => FileType::Empty,
        "block-device" | "b" => FileType::BlockDevice,
        "char-device" | "c" => FileType::CharDevice,
        "directory" | "d" => FileType::Directory,
        "pipe" | "p" => FileType::Pipe,
        "file" | "f" => FileType::File,
        "symlink" | "l" => FileType::Symlink,
        "socket" | "s" => FileType::Socket,
        other => {
            return Err(anyhow!(
                "invalid file type '{other}' (expected one of: empty, \
                 block-device, char-device, directory, pipe, file, symlink, \
                 socket)"
            ))
        }
    };

    Ok(ft)
}

/// Warns (the caller still honors the value) when `threads > available`:
/// throughput typically drops past the core count as the output thread and
/// channel become the bottleneck.
#[must_use]
pub fn oversubscription_warning(
    threads: usize,
    available: usize,
) -> Option<String> {
    (threads > available).then(|| {
        format!(
            "--threads {threads} exceeds available parallelism \
             ({available}); throughput may decrease"
        )
    })
}

/// Parses `--threads`, enforcing `2..=65535`; the floor is 2 because one
/// thread is always reserved for output.
fn parse_threads(x: &str) -> Result<usize, Error> {
    let v = x.parse::<usize>()?;

    if (2..=65535).contains(&v) {
        Ok(v)
    } else {
        Err(anyhow!("threads should be in [2..=65535] range"))
    }
}

/// Parses a path argument, requiring an existing directory and normalizing it.
fn parse_paths(x: &str) -> Result<PathBuf, Error> {
    let p = Path::new(x);

    if p.is_dir() {
        Ok(p.normalize()?.into_path_buf())
    } else {
        Err(anyhow!("'{x}' is not an existing directory"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `parse_inner` over `["minifind", <extra...>]`, returning the
    /// `Outcome`.
    fn parse_argv(extra: &[&str]) -> Result<Outcome, Error> {
        let mut argv: Vec<String> = vec!["minifind".to_string()];
        argv.extend(extra.iter().map(|s| (*s).to_string()));
        parse_inner(argv)
    }

    /// Unwraps `parse_argv` to the `Args` it produced, panicking otherwise.
    fn run(extra: &[&str]) -> Args {
        match parse_argv(extra).unwrap() {
            Outcome::Run(a) => *a,
            other => panic!("expected Outcome::Run, got {other:?}"),
        }
    }

    /// An existing directory usable as a positional path argument.
    fn tmp_dir() -> String {
        std::env::temp_dir().to_str().unwrap().to_string()
    }

    #[test]
    fn test_parse_inner_defaults() {
        let dir = tmp_dir();
        let a = run(&[&dir]);
        assert!(!a.follow_symlinks);
        assert!(a.one_filesystem);
        assert!(!a.case_insensitive);
        assert_eq!(a.max_depth, None);
        assert_eq!(a.max_scan_rate, None);
        assert_eq!(a.name, None);
        assert_eq!(a.regex, None);
        assert_eq!(
            a.file_type,
            vec![FileType::Directory, FileType::File, FileType::Symlink]
        );
        assert_eq!(a.path.len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_inner_idle_defaults_to_two_threads() {
        let dir = tmp_dir();
        let a = run(&["--idle", &dir]);
        assert!(a.idle);
        assert_eq!(a.threads, 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_inner_idle_explicit_threads_wins() {
        let dir = tmp_dir();
        // explicit --threads overrides --idle's 2-thread default, either order
        assert_eq!(run(&["--idle", "--threads", "8", &dir]).threads, 8);
        assert_eq!(run(&["--threads", "8", "--idle", &dir]).threads, 8);
    }

    #[test]
    fn test_parse_inner_idle_off_by_default() {
        let a = run(&[&tmp_dir()]);
        assert!(!a.idle);
    }

    #[test]
    fn test_parse_inner_bare_flags() {
        let dir = tmp_dir();
        let a = run(&["-f", "-i", "--no-one-filesystem", &dir]);
        assert!(a.follow_symlinks);
        assert!(a.case_insensitive);
        assert!(!a.one_filesystem);
    }

    #[test]
    fn test_parse_inner_follow_alias_capital_l() {
        let dir = tmp_dir();
        assert!(run(&["-L", &dir]).follow_symlinks);
    }

    #[test]
    fn test_parse_inner_find_name_alias() {
        let dir = tmp_dir();
        assert_eq!(
            run(&["-name", "*.rs", &dir]).name,
            Some(vec!["*.rs".to_string()])
        );
    }

    #[test]
    fn test_parse_inner_find_type_alias() {
        let dir = tmp_dir();
        assert_eq!(run(&["-type", "f", &dir]).file_type, vec![FileType::File]);
    }

    #[test]
    fn test_parse_inner_find_maxdepth_alias() {
        let dir = tmp_dir();
        assert_eq!(run(&["-maxdepth", "2", &dir]).max_depth, Some(2));
    }

    #[test]
    fn test_parse_inner_find_regex_alias() {
        let dir = tmp_dir();
        assert_eq!(
            run(&["-regex", ".*", &dir]).regex,
            Some(vec![".*".to_string()])
        );
    }

    #[test]
    fn test_parse_inner_find_xdev_and_mount_aliases() {
        let dir = tmp_dir();
        assert!(run(&["--cross-filesystem", "-xdev", &dir]).one_filesystem);
        assert!(run(&["--cross-filesystem", "-mount", &dir]).one_filesystem);
    }

    #[test]
    fn test_parse_inner_find_follow_alias() {
        let dir = tmp_dir();
        assert!(run(&["-follow", &dir]).follow_symlinks);
    }

    #[test]
    fn test_parse_inner_path_glob_aliases() {
        let dir = tmp_dir();
        assert!(run(&["-path", "*/x", &dir]).path_glob.is_some());
        assert!(run(&["-wholename", "*/x", &dir]).path_glob.is_some());
        let a = run(&["-ipath", "*/X", &dir]);
        assert!(a.path_glob.is_some() && a.case_insensitive);
    }

    #[test]
    fn test_parse_inner_lname_aliases() {
        let dir = tmp_dir();
        assert!(run(&["-lname", "*.so", &dir]).lname.is_some());
        let a = run(&["-ilname", "*.SO", &dir]);
        assert!(a.lname.is_some() && a.case_insensitive);
    }

    #[test]
    fn test_parse_inner_newer_reads_reference_file() {
        let dir = tmp_dir();
        // an existing path is a valid reference; a missing one errors
        assert_eq!(run(&["-newer", &dir, &dir]).meta.newer.len(), 1);
        assert!(parse_argv(&["-newer", "/no/such/ref", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_quit_caps_at_one() {
        let dir = tmp_dir();
        assert_eq!(run(&["-quit", &dir]).max_results, Some(1));
    }

    #[test]
    fn test_parse_inner_noop_aliases_accepted() {
        let dir = tmp_dir();
        assert!(parse_argv(&["-print", &dir]).is_ok());
        assert!(parse_argv(&["-ignore_readdir_race", &dir]).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_inner_links_inum_aliases() {
        let dir = tmp_dir();
        assert!(run(&["-links", "2", &dir]).meta.links.is_some());
        assert!(run(&["-inum", "+0", &dir]).meta.inum.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_inner_nouser_nogroup() {
        let dir = tmp_dir();
        assert!(run(&["-nouser", &dir]).meta.nouser);
        assert!(run(&["-nogroup", &dir]).meta.nogroup);
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_inner_access_aliases() {
        let dir = tmp_dir();
        assert_eq!(run(&["-readable", &dir]).access, meta::access::READ);
        let a = run(&["-writable", "-executable", &dir]);
        assert_eq!(a.access, meta::access::WRITE | meta::access::EXEC);
    }

    #[test]
    fn test_parse_inner_find_empty_alias() {
        let dir = tmp_dir();
        assert!(run(&["-empty", &dir]).file_type.contains(&FileType::Empty));
    }

    #[test]
    fn test_parse_inner_iname_sets_name_and_case_insensitive() {
        let dir = tmp_dir();
        let a = run(&["-iname", "*.RS", &dir]);
        assert_eq!(a.name, Some(vec!["*.RS".to_string()]));
        assert!(a.case_insensitive);
    }

    #[test]
    fn test_parse_inner_iregex_sets_regex_and_case_insensitive() {
        let dir = tmp_dir();
        let a = run(&["-iregex", ".*", &dir]);
        assert_eq!(a.regex, Some(vec![".*".to_string()]));
        assert!(a.case_insensitive);
    }

    #[test]
    fn test_parse_inner_combined_shorts() {
        let dir = tmp_dir();
        let a = run(&["-fi", &dir]);
        assert!(a.follow_symlinks);
        assert!(a.case_insensitive);
    }

    #[test]
    fn test_parse_inner_one_filesystem_toggle_last_wins() {
        let dir = tmp_dir();
        // disable then re-enable
        assert!(run(&["--cross-filesystem", "--xdev", &dir]).one_filesystem);
        // enable then disable
        assert!(!run(&["--xdev", "--cross-filesystem", &dir]).one_filesystem);
    }

    #[test]
    fn test_parse_inner_exclude_default_none() {
        let dir = tmp_dir();
        assert_eq!(run(&[&dir]).exclude, None);
    }

    #[test]
    fn test_parse_inner_exclude_repeatable() {
        let dir = tmp_dir();
        let a = run(&["--exclude", ".git", "-E", "node_modules", &dir]);
        assert_eq!(
            a.exclude,
            Some(vec![".git".to_string(), "node_modules".to_string()])
        );
    }

    #[test]
    fn test_parse_inner_null_default_false() {
        let dir = tmp_dir();
        assert!(!run(&[&dir]).null);
    }

    #[test]
    fn test_parse_inner_null_long() {
        let dir = tmp_dir();
        assert!(run(&["--null", &dir]).null);
    }

    #[test]
    fn test_parse_inner_null_short_zero() {
        let dir = tmp_dir();
        assert!(run(&["-0", &dir]).null);
    }

    #[test]
    fn test_parse_inner_print0_find_alias() {
        // find(1)-style `-print0` is normalized to `--null`.
        let dir = tmp_dir();
        assert!(run(&["-print0", &dir]).null);
    }

    #[test]
    fn test_parse_inner_max_depth() {
        let dir = tmp_dir();
        assert_eq!(run(&["-d", "3", &dir]).max_depth, Some(3));
    }

    #[test]
    fn test_parse_inner_no_metadata_predicate_is_inactive() {
        let dir = tmp_dir();
        assert!(!run(&[&dir]).meta.is_active());
    }

    #[test]
    fn test_parse_inner_size_wires_predicate_and_find_alias() {
        let dir = tmp_dir();
        assert!(run(&["--size", "+1k", &dir]).meta.is_active());
        assert!(run(&["-size", "+1k", &dir]).meta.is_active());
    }

    #[test]
    fn test_parse_inner_size_requires_unit_suffix() {
        let dir = tmp_dir();
        assert!(parse_argv(&["--size", "10", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_time_flags_and_aliases() {
        let dir = tmp_dir();
        assert!(run(&["-mtime", "-7", &dir]).meta.is_active());
        assert!(run(&["--amin", "+30", &dir]).meta.is_active());
        // multiple time predicates accumulate (AND)
        let a = run(&["-mtime", "-7", "-atime", "+1", &dir]);
        assert!(a.meta.is_active());
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_inner_perm_uid_gid() {
        let dir = tmp_dir();
        assert!(run(&["-perm", "644", &dir]).meta.is_active());
        assert!(run(&["-perm", "-u+w", &dir]).meta.is_active());
        assert!(run(&["--uid", "0", &dir]).meta.is_active());
        assert!(run(&["-gid", "+10", &dir]).meta.is_active());
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_inner_user_resolves_and_rejects_unknown() {
        let dir = tmp_dir();
        assert!(run(&["-user", "root", &dir]).meta.is_active());
        assert!(parse_argv(&["--user", "no-such-user-xyz-123", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_max_results_default_none() {
        let dir = tmp_dir();
        assert_eq!(run(&[&dir]).max_results, None);
    }

    #[test]
    fn test_parse_inner_max_results() {
        let dir = tmp_dir();
        assert_eq!(run(&["--max-results", "5", &dir]).max_results, Some(5));
    }

    #[test]
    fn test_parse_inner_min_depth_default_none() {
        let dir = tmp_dir();
        assert_eq!(run(&[&dir]).min_depth, None);
    }

    #[test]
    fn test_parse_inner_min_depth() {
        let dir = tmp_dir();
        assert_eq!(run(&["--min-depth", "2", &dir]).min_depth, Some(2));
    }

    #[test]
    fn test_parse_inner_mindepth_find_alias() {
        // find(1)-style `-mindepth N` is normalized to `--min-depth N`.
        let dir = tmp_dir();
        assert_eq!(run(&["-mindepth", "2", &dir]).min_depth, Some(2));
    }

    #[test]
    fn test_parse_inner_threads_valid() {
        let dir = tmp_dir();
        assert_eq!(run(&["-x", "4", &dir]).threads, 4);
    }

    #[test]
    fn test_parse_inner_threads_out_of_range() {
        let dir = tmp_dir();
        assert!(parse_argv(&["-x", "1", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_name_repeatable() {
        let dir = tmp_dir();
        let a = run(&["-n", "a", "--name", "b", &dir]);
        assert_eq!(a.name, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn test_parse_inner_name_regex_conflict() {
        let dir = tmp_dir();
        assert!(parse_argv(&["-n", "*.rs", "-r", ".*", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_file_type_names_and_aliases() {
        let dir = tmp_dir();
        let a = run(&["-t", "f", "--file-type", "directory", &dir]);
        assert_eq!(a.file_type, vec![FileType::File, FileType::Directory]);
    }

    #[test]
    fn test_parse_inner_file_type_invalid() {
        let dir = tmp_dir();
        assert!(parse_argv(&["-t", "zzz", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_missing_path() {
        assert!(parse_argv(&["-f"]).is_err());
    }

    #[test]
    fn test_parse_inner_unknown_option() {
        let dir = tmp_dir();
        assert!(parse_argv(&["--bogus", &dir]).is_err());
    }

    #[test]
    fn test_parse_inner_help() {
        assert!(matches!(parse_argv(&["--help"]).unwrap(), Outcome::Help));
        assert!(matches!(parse_argv(&["-h"]).unwrap(), Outcome::Help));
    }

    #[test]
    fn test_parse_inner_version() {
        assert!(matches!(
            parse_argv(&["--version"]).unwrap(),
            Outcome::Version
        ));
        assert!(matches!(parse_argv(&["-V"]).unwrap(), Outcome::Version));
    }

    #[test]
    fn test_parse_file_type_all_canonical() {
        assert_eq!(parse_file_type("empty").unwrap(), FileType::Empty);
        assert_eq!(
            parse_file_type("block-device").unwrap(),
            FileType::BlockDevice
        );
        assert_eq!(
            parse_file_type("char-device").unwrap(),
            FileType::CharDevice
        );
        assert_eq!(parse_file_type("directory").unwrap(), FileType::Directory);
        assert_eq!(parse_file_type("pipe").unwrap(), FileType::Pipe);
        assert_eq!(parse_file_type("file").unwrap(), FileType::File);
        assert_eq!(parse_file_type("symlink").unwrap(), FileType::Symlink);
        assert_eq!(parse_file_type("socket").unwrap(), FileType::Socket);
    }

    #[test]
    fn test_parse_threads_min_valid() {
        assert_eq!(parse_threads("2").unwrap(), 2);
    }

    #[test]
    fn test_parse_threads_max_valid() {
        assert_eq!(parse_threads("65535").unwrap(), 65535);
    }

    #[test]
    fn test_parse_threads_mid_valid() {
        assert_eq!(parse_threads("100").unwrap(), 100);
    }

    #[test]
    fn test_parse_threads_zero_invalid() {
        assert!(parse_threads("0").is_err());
    }

    #[test]
    fn test_parse_threads_one_invalid() {
        assert!(parse_threads("1").is_err());
    }

    #[test]
    fn test_parse_threads_too_large() {
        assert!(parse_threads("65536").is_err());
    }

    #[test]
    fn test_parse_threads_non_numeric() {
        assert!(parse_threads("abc").is_err());
        assert!(parse_threads("").is_err());
    }

    #[test]
    fn test_parse_threads_negative() {
        // Negative strings fail usize parse
        assert!(parse_threads("-1").is_err());
    }

    #[test]
    fn test_parse_paths_valid_dir() {
        let tmp = std::env::temp_dir();
        let result = parse_paths(tmp.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_paths_normalizes() {
        let tmp = std::env::temp_dir();
        let result = parse_paths(tmp.to_str().unwrap()).unwrap();
        assert!(result.is_dir());
    }

    #[test]
    fn test_parse_paths_nonexistent() {
        assert!(parse_paths("/nonexistent/xyz/abc123").is_err());
    }

    #[test]
    fn test_parse_paths_file_not_dir() {
        // /etc/hosts exists on macOS and Linux but is not a directory
        assert!(parse_paths("/etc/hosts").is_err());
    }

    #[test]
    fn test_oversubscription_warning_above_cpu_count() {
        let w = oversubscription_warning(16, 8);
        assert!(w.is_some());
        let msg = w.unwrap();
        assert!(msg.contains("16") && msg.contains('8'));
    }

    #[test]
    fn test_oversubscription_warning_at_or_below_cpu_count() {
        assert!(oversubscription_warning(8, 8).is_none());
        assert!(oversubscription_warning(4, 8).is_none());
        assert!(oversubscription_warning(2, 8).is_none());
    }

    #[test]
    fn test_parse_inner_max_scan_rate_value() {
        let dir = tmp_dir();
        assert_eq!(
            run(&["--max-scan-rate", "50", &dir]).max_scan_rate,
            Some(50)
        );
    }

    #[test]
    fn test_parse_inner_max_scan_rate_short() {
        let dir = tmp_dir();
        assert_eq!(run(&["-s", "50", &dir]).max_scan_rate, Some(50));
    }

    #[test]
    fn test_parse_inner_max_scan_rate_zero_parses() {
        // 0 is accepted at parse time; run() treats it as unlimited
        let dir = tmp_dir();
        assert_eq!(
            run(&["--max-scan-rate", "0", &dir]).max_scan_rate,
            Some(0)
        );
    }

    #[test]
    fn test_parse_inner_max_scan_rate_non_numeric_errors() {
        let dir = tmp_dir();
        assert!(parse_argv(&["--max-scan-rate", "abc", &dir]).is_err());
    }

    #[test]
    fn test_parse_paths_normalizes_dotdot() {
        use std::path::Component;
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().parent().unwrap();
        let name = tmp.path().file_name().unwrap();
        // Construct: <parent>/<name>/../<name> — contains a redundant ".."
        let with_dotdot = parent.join(name).join("..").join(name);
        let result = parse_paths(with_dotdot.to_str().unwrap()).unwrap();
        assert!(
            !result.components().any(|c| matches!(c, Component::ParentDir)),
            "normalized path must not contain .. components"
        );
    }
}
