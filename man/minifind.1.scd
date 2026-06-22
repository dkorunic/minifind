minifind(1)

# NAME

minifind - fast, parallel *find*(1) reimplementation

# SYNOPSIS

*minifind* [_OPTIONS_] _PATH_...

# DESCRIPTION

*minifind* lists filesystem entries under each _PATH_ as fast as possible,
using its own parallel, work-stealing directory walker. It is a focused subset
of *find*(1): it *prints* matching paths and has no actions (*-exec*, *-delete*)
and no boolean grammar (*-o*, *-not*, parentheses) — every predicate is ANDed
together.

Each _PATH_ must be an existing directory. By default symbolic links are not
followed, mount points are not crossed, and one worker thread is spawned per
logical CPU (with one thread reserved for output).

Most options also accept their *find*(1)-style single-dash spelling (for
example *-name*, *-type*, *-size*, *-newer*), noted below as aliases.

# OPTIONS

## Traversal

*-f*, *--follow-symlinks*
	Follow symbolic links. Aliases: *-L*, *-follow*.

*-o*, *--one-filesystem*
	Do not descend into directories on other filesystems (the default).
	Aliases: *--xdev*, *-xdev*, *-mount*.

*--no-one-filesystem*
	Cross mount points. Alias: *--cross-filesystem*.

*-x*, *--threads* _N_
	Number of worker threads (2..=65535). Default: the logical CPU count.

*-d*, *--max-depth* _N_
	Descend at most _N_ levels below each starting path (the root is depth 0).
	Alias: *-maxdepth*.

*--min-depth* _N_
	Do not print entries shallower than depth _N_; deeper entries are still
	reached. Combine with *--max-depth* _N_ for an exact-depth slice.
	Alias: *-mindepth*.

*-s*, *--max-scan-rate* _N_
	Throttle traversal to at most _N_ directories scanned per second across all
	threads. 0 means unlimited.

*--max-results* _N_
	Print at most _N_ results, then stop the walk. 0 means unlimited.

*--quit*
	Stop after the first match (equivalent to *--max-results* _1_).
	Alias: *-quit*.

## Name, path and type matching

*-n*, *--name* _GLOB_
	Match _GLOB_ against the file-name component only. Repeatable; conflicts
	with *--regex*. Aliases: *-name*; *-iname* additionally sets *-i*.

*-r*, *--regex* _RE_
	Match the Rust regular expression _RE_ against the whole path. Repeatable;
	conflicts with *--name*. Aliases: *-regex*; *-iregex* additionally sets *-i*.

*--path* _GLOB_
	Match _GLOB_ against the whole path; here *\** also matches */*, like
	*find*(1)'s *-path*. Aliases: *-path*, *-wholename*; *-ipath* and
	*-iwholename* additionally set *-i*.

*--lname* _GLOB_
	Match _GLOB_ against a symbolic link's target (non-symlinks never match).
	Alias: *-lname*; *-ilname* additionally sets *-i*.

*-i*, *--case-insensitive*
	Case-insensitive glob and regex matching.

*-t*, *--file-type* _TYPE_
	Keep only entries of _TYPE_ (repeatable). _TYPE_ is one of _empty_,
	_block-device_, _char-device_, _directory_, _pipe_, _file_, _socket_ or
	_symlink_, or the single-letter alias _e_, _b_, _c_, _d_, _p_, _f_, _s_,
	_l_. Default: _directory file symlink_. Alias: *-type*.

*--empty*
	Match empty files and directories (same as *--file-type* _empty_).
	Alias: *-empty*.

*-E*, *--exclude* _GLOB_
	Exclude entries whose file name matches _GLOB_ (repeatable). A matched
	directory is pruned: its whole subtree is skipped without being opened.

## Metadata predicates

These require a *stat*(2) and are evaluated only on entries that already passed
the cheaper filters above. Numeric arguments use *find*(1)'s tristate: _N_
matches exactly, _+N_ means "greater than", _-N_ means "less than". The
permission, ownership, link and inode predicates are Unix-only.

*--size* _[+-]N(c|k|M|G|T)_
	Match by size. A unit suffix is required: _c_ = bytes, _k_/_M_/_G_/_T_ are
	1024-based. The file size is rounded up to the unit. Alias: *-size*.

*--mtime*, *--ctime*, *--atime* _[+-]N_
	Match by modification / status-change / access time, in days (find's
	24-hour integer-division semantics). Aliases: *-mtime*, *-ctime*, *-atime*.

*--mmin*, *--cmin*, *--amin* _[+-]N_
	As above, but in minutes. Aliases: *-mmin*, *-cmin*, *-amin*.

*--newer*, *--anewer*, *--cnewer* _FILE_
	Match when the entry's modification / access / status-change time is newer
	than _FILE_'s modification time. Aliases: *-newer*, *-anewer*, *-cnewer*.

*--perm* _[/-]MODE_
	Match permission bits. _MODE_ is octal (_644_) or symbolic (_u+w,g-x_).
	Plain _MODE_ matches exactly; _-MODE_ requires all of those bits set;
	_/MODE_ requires any of them set. Alias: *-perm*.

*--uid*, *--gid* _[+-]N_
	Match by numeric owner / group id. Aliases: *-uid*, *-gid*.

*--user*, *--group* _NAME_
	Match by owner / group name (a numeric id is also accepted), resolved once
	at startup. Aliases: *-user*, *-group*.

*--links* _[+-]N_
	Match by hard-link count. Alias: *-links*.

*--inum* _[+-]N_
	Match by inode number. Alias: *-inum*.

*--nouser*, *--nogroup*
	Match when the owner uid / gid resolves to no passwd / group entry.
	Aliases: *-nouser*, *-nogroup*.

*--readable*, *--writable*, *--executable*
	Match entries the real uid/gid can read / write / execute (*access*(2)).
	Aliases: *-readable*, *-writable*, *-executable*.

## Output

*-0*, *--null*
	Terminate each printed path with a NUL byte instead of a newline, for safe
	piping into *xargs -0*. Aliases: *-print0*, *--print0*.

## Information

*-h*, *--help*
	Print a short usage summary and exit.

*-V*, *--version*
	Print version information and exit.

# EXAMPLES

All Rust source files under _src_:

```
minifind -t f -name '*.rs' src
```

Log files modified in the last day, removed safely via *xargs*(1):

```
minifind -t f -name '*.log' -mtime -1 -print0 . | xargs -0 rm -f
```

Find TOML files, pruning heavy directories before they are opened:

```
minifind -E .git -E node_modules -E target -name '*.toml' .
```

Files larger than 100 MiB under _/var_:

```
minifind -t f -size +100M /var
```

World-writable files (any "other write" bit set) — a quick security sweep:

```
minifind -t f -perm -0002 /etc
```

Empty files and directories:

```
minifind -empty .
```

Regular files owned by _root_:

```
minifind -t f -user root /usr/local
```

Symbolic links that point into _/usr/lib_:

```
minifind -t l -lname '/usr/lib/*' /etc
```

Directories exactly two levels deep:

```
minifind -t d --min-depth 2 --max-depth 2 .
```

Does any C header exist below here? Stop at the first match:

```
minifind -name '*.h' --quit . && echo "found one"
```

# EXIT STATUS

*0*
	Success.

*1*
	A runtime error occurred — for example an invalid *--name* glob, *--regex*,
	*--path*, *--lname* or *--exclude* pattern.

*2*
	Invalid command-line usage — an unrecognized option, a bad numeric, size or
	permission value, an unknown user or group, a missing *--newer* reference
	file, or no starting path.

# SEE ALSO

*find*(1), *fd*(1), *xargs*(1)

# AUTHORS

Maintained by Dinko Korunic <dinko.korunic@gmail.com>. Source, bug reports and
the complete option list: <https://github.com/dkorunic/minifind>.
