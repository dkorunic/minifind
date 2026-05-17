# AGENTS.md

High-signal facts for working in minifind. Every line answers: "Would an agent miss this without help?"

## Commands

```sh
cargo test                    # all tests (inline in modules)
cargo test <test_name>        # single test
cargo clippy -- -D warnings   # lint
cargo fmt                     # format
```

Pinned toolchain: none declared — use the Rust version from your environment.

## Architecture

Single-binary CLI, no async. `CLAUDE.md` has the full module map and execution pipeline — don't duplicate it here.

## Constraints and gotchas

- **`--threads` range**: validated to `2..=65535` in `args.rs`. Because `mimalloc` has a global mutex, `--threads > 21` yields no throughput gain (20 walker + 1 output saturates the allocator).
- **Path inputs**: must be *existing directories*. `parse_paths()` rejects non-directories and non-existent paths.
- **Default file types**: `directory`, `file`, `symlink`.
- **`--empty` behavior**: auto-expands to include `file` and `directory` unless another type flag is also set.
- **`--name` vs `--regex`**: mutually exclusive. `--name` matches `file_name()` only via `globset`; `--regex` matches the full path via `regex::bytes::RegexSet`.
- **Duplicate paths**: deduplicated via `itertools::Itertools::unique` before walking.
- **`--one-filesystem`**: defaults to `true` (crosses `-o` / `--xdev` CLI alias).
- **`--follow-symlinks`**: defaults to `false`; `-L` is a visible short alias.
- **`FileType::Empty`**: checks via `read_dir` (dirs) or `metadata().len()` (files) — an extra syscall per entry.
- **`--regex` uses Rust regex syntax**: no look-around, no backreferences.
- **`ignore::WalkBuilder`**: standard `.gitignore` filters are enabled by default (WalkBuilder defaults); override via `--one-filesystem false` for mount cross, but not for ignore files.
- **Output path**: Unix writes raw `OsStr` bytes; non-Unix uses `to_string_lossy()`.

## Release

Uses `cargo-dist` (`dist-workspace.toml`). Generates shell, PowerShell, MSI installers for:
`aarch64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `x86_64-pc-windows-msvc`.

Release profile: fat LTO, `codegen-units = 1`, `strip = "symbols"`, `panic = "abort"`.
