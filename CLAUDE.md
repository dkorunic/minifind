# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                   # debug build
cargo build --release         # optimized release build
cargo test                    # run all tests
cargo test <test_name>        # run a single test by name
cargo bench                   # run Criterion benchmarks (benches/)
cargo clippy -- -D warnings   # lint (warnings as errors)
cargo fmt                     # format code
cargo fmt -- --check          # check formatting without modifying
```

The minimum supported Rust version (`rust-version = "1.85.1"`) and `edition = "2021"` are declared in `Cargo.toml`. There is no pinned `rust-toolchain.toml`, so builds use whatever toolchain rustup has active.

## Code Style

- Line width: **79 characters** (`rustfmt.toml`: `max_width = 79`, `use_small_heuristics = "max"`)
- Always run `cargo fmt` before committing.

## Architecture

`minifind` is a CLI with no async runtime, split into a thin binary (`main.rs`) and a library crate (`lib.rs`) so the pipeline can be integration-tested through its public API. The concurrency model, driven by `lib::run()`, is:

1. **Walker threads** (`walk::walk_parallel`, minifind's own work-stealing walker, `threads - 1` workers) — traverse the filesystem and push matched `walk::Entry` items onto a bounded `crossbeam-channel`. Each worker runs a per-thread visitor closure (built by a factory passed to `walk_parallel`) that filters by type and accumulates entries in a per-thread `BatchSender` (`lib.rs`), sending them in batches of `BATCH_SIZE` (256, tuned by sweeping a warm Linux-kernel tree); the trailing partial batch is flushed on `Drop` when the visitor closure ends. Batching amortizes the per-send atomic synchronization (~21% faster wall, ~83% fewer context switches on a Linux-kernel tree) and removes the prior throughput regression past the core count. `--threads` defaults to `available_parallelism()` and is validated to `2..=65535` in `args.rs::parse_threads`; the floor of 2 exists because one thread is always reserved for output (`threads - 1` must be ≥ 1). `main.rs` warns (but still honors) when `--threads` exceeds `available_parallelism()` via `args::oversubscription_warning`.
2. **Output thread** (1 dedicated thread) — drains batches, applies glob/regex filters, and writes matched paths to a 256 KB `BufWriter` over a caller-supplied sink (locked stdout in the binary). The path bytes and newline are written straight into the `BufWriter` (which already coalesces) — no per-entry scratch buffer and one byte-copy per entry.

`run()` takes a `make_out: FnOnce() -> impl Write` factory rather than a writer, so the sink is constructed *on the output thread* — this lets the binary hand over a non-`Send` `StdoutLock` while tests inject an in-memory sink.

### Module responsibilities

| Module | Role |
|---|---|
| `main.rs` | Thin binary entry point: declares the `mimalloc` global allocator, parses args, warns on thread oversubscription, resets `SIGPIPE`, calls `minifind::run()` with `\|\| io::stdout().lock()` |
| `lib.rs` | `run()` — wires everything together: register signals → build glob/regex sets → build channel + walker → spawn output thread → run walker → join. Also defines `BatchSender` (per-thread batched channel send, flush-on-`Drop`). Re-exports all modules as `pub` |
| `args.rs` | `lexopt`-based parser (`parse_inner` core + `Args::parse()` wrapper); validates thread count, normalizes input paths via `normpath`, exposes `oversubscription_warning()` |
| `walk.rs` | The custom parallel walker: `walk_parallel()` drives a `crossbeam-deque` work-stealing engine (atomic-counter termination) over `Args`, emitting `Entry { path, file_type }` through a per-thread visitor; handles `max_depth`, `one_filesystem`, `follow_symlinks` + symlink-loop detection, and multi-root |
| `walk/unix.rs` | `#[cfg(unix)]` leaf I/O: `rustix` `getdents`/`openat`/`fstat`/`statat`; maps `d_type` to `EntryType`, lstat fallback for `DT_UNKNOWN` |
| `walk/fallback.rs` | `#[cfg(not(unix))]` leaf I/O via `std::fs`; `dev=0` (so `--one-filesystem` is a no-op) and `ino`=canonical-path hash for loop detection |
| `filetype.rs` | `EntryType` enum + `FileType` bitmask; `ignore_filetype(EntryType, &Path)` called **inside walker threads** for early rejection |
| `glob.rs` | `build_glob_set()` — builds a `globset::GlobSet`; matching happens in the output thread against `file_name()` only |
| `regex.rs` | `build_regex_set()` — builds a `regex::bytes::RegexSet`; matching happens in the output thread against the full path as bytes |
| `interrupt.rs` | Registers Unix/Windows signals via `signal-hook`; sets an `Arc<AtomicBool>` flag checked in the walker closure |
| `ratelimit.rs` | `Limiter` wrapping `governor` — a synchronous, shutdown-aware token bucket (`acquire` loops `check()` + capped sleep); throttles one token per directory in `walk::descend` |

### Key design decisions

- **`mimalloc`** is the global allocator (`#[global_allocator]`) for improved allocation throughput.
- **`--name` and `--regex` are mutually exclusive** (enforced after the `lexopt` parse loop); `--name` matches only the filename component while `--regex` matches the full path. The three former clap `ArgAction::Set` bool options (`--follow-symlinks`, `--one-filesystem`, `--case-insensitive`) are now bare flags; `--one-filesystem` defaults on and is cleared via `--no-one-filesystem`/`--cross-filesystem`.
- The walker is minifind's own (no `ignore` dependency): a `crossbeam-deque` work-stealing engine in `walk.rs` over a `cfg`-split leaf (`rustix` `getdents` on Unix, `std::fs` elsewhere). `ignore` remains only as a dev-dependency for the `benches/filetype.rs` comparison.
- Entry type comes from the directory `d_type` (no per-entry `stat`); `filetype.rs` classifies on the platform-agnostic `EntryType` enum. Device/pipe/socket types are only distinguished on Unix (the `std::fs` fallback collapses non-dir/non-symlink to `File`).
- Platform path output: Unix writes raw `OsStr` bytes (`OsStrExt::as_bytes`); non-Unix uses `Path::to_string_lossy().as_bytes()`. The same split lives in `regex::path_to_bytes`, which returns a `Cow<[u8]>` (borrowed on Unix, owned-on-lossy elsewhere) for full-path regex matching.
- Channel buffer size is `CHAN_MULT * (threads - 1)` **batches** (`CHAN_MULT` = 4, each up to `BATCH_SIZE` = 256 entries); entries cross the channel in batches, not individually, to cut synchronization overhead. Both constants were tuned by sweeping a warm Linux-kernel tree (throughput is flat across `CHAN_MULT` 2–16; batch size flattens by ~256).
- The `--empty` (`-t e`) type implicitly enables both file and directory matching unless another type flag is also set.
- **`--max-iops <N>`** caps directories visited per second globally across walker threads via `ratelimit::Limiter` (a `governor` token bucket); the single throttle point is in `walk::descend` before `open_dir`, so each directory read spends one token. `None`/`0` means unlimited (no limiter is built, so the hot path is unchanged). A throttled worker rechecks the walker's abort flag in ≤100 ms chunks rather than blocking on a full directory's I/O; since that flag is only set once a worker next reaches the visitor, the stop can lag by up to roughly one token interval (≈1 s at `--max-iops 1`).
- `Cargo.toml` declares a `[lints.clippy]` table (`all = deny`, `redundant_clone = deny`), so these are enforced on every `cargo clippy`/`build`, not just via CLI flags.

### Testing

- **Unit tests** live in each module's `#[cfg(test)] mod tests` and exercise private helpers.
- **Integration tests** in `tests/pipeline.rs` drive the real `minifind::run()` end-to-end, capturing output via a shared in-memory `Write` sink rather than re-implementing the pipeline.
- **Doc-tests** run because of the library crate (binary crates skip them); e.g. `interrupt::setup_interrupt_handler`'s example is a live, compiled test.
- **Benchmarks** (Criterion, `harness = false`):
  - `benches/filetype.rs` compares the bitmask `ignore_filetype` against the historical seven-term `||` chain over pre-collected `DirEntry`s. The selection is made runtime-opaque with `black_box` *outside* the timed loop on both sides so the comparison is fair (hardcoded flags would let LLVM fold the chain away).
  - `benches/walk.rs` is an end-to-end `minifind` vs GNU `find` comparison over a shallow Linux-kernel clone, mirroring the [bench_walk](https://github.com/dkorunic/bench_walk) methodology. Both contenders run as subprocesses (minifind via `CARGO_BIN_EXE_minifind`) so each pays process startup. The clone is cached under `benches/linux_root/` (gitignored); set `BENCH_WALK_DIR` to reuse an existing checkout. It uses bench_walk's long 80 s / 400 s windows — override with `cargo bench --bench walk -- --warm-up-time <s> --measurement-time <s>` for a quick run.

### Release profile

`Cargo.toml` [profile.release] uses fat LTO, `codegen-units = 1`, `strip = "symbols"`, and `panic = "abort"` — intentional for a single-purpose CLI binary where binary size and throughput matter more than debug ergonomics.
