# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                   # debug build
cargo build --release         # optimized release build
cargo test                    # run all tests
cargo test <test_name>        # run a single test by name
cargo clippy -- -D warnings   # lint (warnings as errors)
cargo fmt                     # format code
cargo fmt -- --check          # check formatting without modifying
```

The pinned toolchain (Rust 1.93) is declared in `rust-toolchain.toml` and activated automatically by rustup.

## Code Style

- Line width: **79 characters** (`rustfmt.toml`: `max_width = 79`, `use_small_heuristics = "max"`)
- Always run `cargo fmt` before committing.

## Architecture

`minifind` is a single-binary CLI with no async runtime. The concurrency model is:

1. **Walker threads** (`ignore::WalkParallel`, `threads - 1` workers) — traverse the filesystem and push `DirEntry` items onto a bounded `crossbeam-channel`.
2. **Output thread** (1 dedicated thread) — drains the channel, applies glob/regex filters, and writes matched paths to a 256 KB `BufWriter` over locked stdout.

### Module responsibilities

| Module | Role |
|---|---|
| `main.rs` | Wires everything together: parse args → build channel + walker → spawn output thread → run walker → join |
| `args.rs` | Clap `Parser` struct; validates thread count and normalizes input paths via `normpath` |
| `walk.rs` | `build_walker()` — configures `ignore::WalkBuilder` from `Args` and returns `WalkParallel` |
| `filetype.rs` | `FileType` bitmask; `ignore_filetype()` called **inside walker threads** for early rejection |
| `glob.rs` | `build_glob_set()` — builds a `globset::GlobSet`; matching happens in the output thread against `file_name()` only |
| `regex.rs` | `build_regex_set()` — builds a `regex::bytes::RegexSet`; matching happens in the output thread against the full path as bytes |
| `interrupt.rs` | Registers Unix/Windows signals via `signal-hook`; sets an `Arc<AtomicBool>` flag checked in the walker closure |

### Key design decisions

- **`mimalloc`** is the global allocator (`#[global_allocator]`) for improved allocation throughput.
- **`--name` and `--regex` are mutually exclusive** (`conflicts_with` in Clap); `--name` matches only the filename component while `--regex` matches the full path.
- Filetype filtering (`filetype.rs`) uses Unix-only traits (`std::os::unix::fs::FileTypeExt`) guarded by `#[cfg(unix)]`; non-Unix builds always return `false` for block/char/pipe/socket.
- Platform path output: Unix writes raw `OsStr` bytes; non-Unix uses `bstr::ByteVec::from_path_lossy`.
- Channel buffer size is `16 * (threads - 1)` — empirically tuned.
- The `--empty` (`-t e`) type implicitly enables both file and directory matching unless another type flag is also set.

### Release profile

`Cargo.toml` [profile.release] uses fat LTO, `codegen-units = 1`, `strip = "symbols"`, and `panic = "abort"` — intentional for a single-purpose CLI binary where binary size and throughput matter more than debug ergonomics.
