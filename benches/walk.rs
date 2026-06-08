// SPDX-FileCopyrightText: 2022 Dinko Korunic <dinko.korunic@gmail.com>
// SPDX-License-Identifier: MIT

//! End-to-end `minifind` vs GNU `find` over a Linux kernel tree, mirroring
//! <https://github.com/dkorunic/bench_walk>.
//!
//! Both run as subprocesses so the comparison is fair (each pays process
//! startup). The corpus is shallow-cloned once into `benches/linux_root`; set
//! `BENCH_WALK_DIR` to reuse a checkout (CI/offline). Heavy by design (several
//! minutes); shorten with `cargo bench --bench walk -- --measurement-time 20`.

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};

const TEST_DIR: &str = "benches/linux_root";
const WARMUP_TIME: u64 = 80;
const MEASURE_TIME: u64 = 400;

/// Returns the corpus path, shallow-cloning the kernel on first use unless
/// `BENCH_WALK_DIR` points at an existing checkout.
fn prepare_test_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("BENCH_WALK_DIR") {
        return PathBuf::from(dir);
    }

    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join(TEST_DIR);
    if !target.exists() {
        eprintln!("Cloning Linux kernel into {} ...", target.display());
        let status = Command::new("git")
            .args([
                "clone",
                "https://github.com/torvalds/linux.git",
                "--depth",
                "1",
            ])
            .arg(&target)
            .status()
            .expect("failed to spawn git; is it installed?");
        assert!(status.success(), "git clone of the Linux kernel failed");
        let _ = Command::new("sync").status();
        eprintln!("Clone complete.");
    }

    target
}

/// Walks `root` with the compiled `minifind` binary (defaults), discarding output.
fn minifind_walk(root: &Path) {
    Command::new(env!("CARGO_BIN_EXE_minifind"))
        .arg(root)
        .output()
        .expect("failed to spawn minifind");
}

/// Walks `root` with system GNU `find`, discarding output.
fn find_walk(root: &Path) {
    Command::new("find").arg(root).output().expect("failed to spawn find");
}

fn bench_walk(c: &mut Criterion) {
    let work_dir = prepare_test_dir();

    let mut g = c.benchmark_group("walk_linux_kernel");
    g.bench_function("minifind", |b| {
        b.iter(|| minifind_walk(black_box(&work_dir)));
    });
    g.bench_function("find", |b| {
        b.iter(|| find_walk(black_box(&work_dir)));
    });
    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_secs(WARMUP_TIME))
        .measurement_time(Duration::from_secs(MEASURE_TIME));
    targets = bench_walk
}
criterion_main!(benches);
