//! T048 — scan-throughput benchmark (SC-006 budget tracking).
//!
//! Drives the in-process engine (`Engine::scan`) over a synthetic tree, measuring both the
//! cold path (cache disabled) and the warm path (cache loaded from disk). This is a trend
//! tracker, not a hard gate — the spec's 10k-file <1s warm / <3s cold bars are asserted on
//! the reference runner; here we watch for regressions in relative throughput.
//!
//! Run with `cargo bench`; results land under `target/criterion/`.

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use licet::config::LicensingConfiguration;
use licet::engine::Engine;
use licet::walk::Selection;
use licet::walk::cache::{ScanCache, config_fingerprint};

const CONFIG: &str = "[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"license.toml\"]\n";

/// Populate `root` with `n` compliant Rust files spread across subdirectories.
fn build_tree(root: &Path, n: usize) {
    std::fs::write(root.join("license.toml"), CONFIG).unwrap();
    for i in 0..n {
        let dir = root.join(format!("src/d{}", i / 100));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("f{i}.rs")),
            "// SPDX-License-Identifier: MIT\nfn f() {}\n",
        )
        .unwrap();
    }
}

fn bench_scan(c: &mut Criterion) {
    let config = LicensingConfiguration::from_toml(CONFIG).unwrap();
    let fingerprint = config_fingerprint(CONFIG);

    let mut group = c.benchmark_group("scan");
    for &n in &[500usize, 2000] {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        build_tree(&root, n);
        let cache_path = root.join(".licet-bench-cache");

        group.throughput(Throughput::Elements(n as u64));

        // Cold: cache disabled, every file fully classified.
        group.bench_with_input(BenchmarkId::new("cold", n), &n, |b, _| {
            let engine = Engine::new(root.clone(), &config, CONFIG);
            b.iter(|| {
                let mut cache = ScanCache::disabled();
                engine.scan(&Selection::FullTree, &mut cache).unwrap();
            });
        });

        // Warm: prime an on-disk cache once, then measure load + scan per iteration.
        {
            let engine = Engine::new(root.clone(), &config, CONFIG);
            let mut warm = ScanCache::open(&cache_path, &fingerprint);
            engine.scan(&Selection::FullTree, &mut warm).unwrap();
            warm.flush().ok();
        }
        group.bench_with_input(BenchmarkId::new("warm", n), &n, |b, _| {
            let engine = Engine::new(root.clone(), &config, CONFIG);
            b.iter(|| {
                let mut cache = ScanCache::open(&cache_path, &fingerprint);
                engine.scan(&Selection::FullTree, &mut cache).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_scan);
criterion_main!(benches);
