//! Scan-throughput benchmark: the stateless engine over synthetic trees.
//!
//! No cache artifact exists anymore — every iteration classifies from current
//! bytes, so "cold" and "warm" are the same code path (filesystem page cache
//! aside). This is a trend tracker, not a hard gate: count/status equality is
//! asserted once outside the timed loops, and engine-only timing is kept
//! separate from end-to-end CLI timing (see tests/perf.rs for the CLI side).
//!
//! Run with `cargo bench --bench scan`; results land under `target/criterion/`.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use licet::config::LicensingConfiguration;
use licet::engine::Engine;
use licet::walk::Selection;
use licet::walk::{Discovered, Purpose, prepare};

const BASE_CONFIG: &str = "[default]\nlicense=\"MIT\"\n[exclude]\npaths=[\"licet.toml\"]\n";

/// Populate `root` with `n` compliant tiny Rust files across subdirectories.
fn build_tiny_tree(root: &Path, n: usize) {
    std::fs::write(root.join("licet.toml"), BASE_CONFIG).unwrap();
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

/// A mixed tree: late snippets in large texts, binary sidecars, nested
/// metadata, and dozens of rules — the workloads a head-only scanner skipped.
fn build_mixed_tree(root: &Path, n: usize) -> String {
    let mut config = String::from(BASE_CONFIG);
    for i in 0..24 {
        config.push_str(&format!(
            "[[rule]]\nglob=\"src/d{i}/**\"\nlicense=\"MIT\"\n"
        ));
    }
    std::fs::write(root.join("licet.toml"), &config).unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(
        root.join("sub/REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"*.dat\"\nSPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    for i in 0..n {
        let dir = root.join(format!("src/d{}", i / 100));
        std::fs::create_dir_all(&dir).unwrap();
        match i % 5 {
            // Late snippet just before EOF in an 8 KiB file.
            0 => {
                let mut body = String::from("// SPDX-License-Identifier: MIT\n");
                while body.len() < 8 * 1024 {
                    body.push_str("// filler line to push the snippet late\n");
                }
                body.push_str("// SPDX-SnippetBegin: s1\n// SPDX-License-Identifier: MIT\n// SPDX-SnippetEnd: s1\n");
                std::fs::write(dir.join(format!("f{i}.rs")), body).unwrap();
            }
            // Binary asset covered by a sidecar.
            1 => {
                std::fs::write(dir.join(format!("f{i}.bin")), [0x00, 0xFF, 0x89]).unwrap();
                std::fs::write(
                    dir.join(format!("f{i}.bin.license")),
                    "SPDX-License-Identifier: MIT\nSPDX-FileCopyrightText: 2026 Bench\n",
                )
                .unwrap();
            }
            // File covered by nested metadata.
            2 => {
                let sub = root.join("sub");
                std::fs::create_dir_all(&sub).unwrap();
                std::fs::write(sub.join(format!("f{i}.dat")), "opaque\n").unwrap();
            }
            _ => {
                std::fs::write(
                    dir.join(format!("f{i}.rs")),
                    "// SPDX-License-Identifier: MIT\nfn f() {}\n",
                )
                .unwrap();
            }
        }
    }
    config
}

fn prepared_paths(
    root: &Path,
) -> (
    LicensingConfiguration,
    Vec<Discovered>,
    licet::walk::Snapshot,
) {
    let prep = prepare(
        root,
        &root.join("licet.toml"),
        &Selection::FullTree,
        Purpose::Policy,
        false,
    )
    .unwrap();
    let config = LicensingConfiguration::from_toml(&prep.config_text).unwrap();
    (config, prep.paths, prep.snapshot)
}

/// Assert the evaluated set is exactly what the timed loop will classify:
/// same file count, every file compliant or excluded. Runs once, outside
/// timing. (Excluded files such as `licet.toml` stay in the path set with
/// an `Excluded` verdict — they are part of the equivalent set.)
fn assert_equivalent(engine: &Engine, paths: &[Discovered]) {
    let result = engine.scan(paths).unwrap();
    assert_eq!(result.states.len(), paths.len(), "equivalent file set");
    assert!(
        result.states.iter().all(|s| matches!(
            s.drift,
            licet::domain::DriftClass::Compliant | licet::domain::DriftClass::Excluded
        )),
        "all compliant or excluded outside the timed loop"
    );
}

fn bench_scan(c: &mut Criterion) {
    for &n in &[500usize, 2000, 10_000] {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        build_tiny_tree(&root, n);
        let (config, paths, snapshot) = prepared_paths(&root);
        assert_eq!(paths.len(), n + 1, "n files plus the excluded config");

        let engine = Engine::new(root.clone(), &config, snapshot, true);
        assert_equivalent(&engine, &paths);
        let mut iter_group = c.benchmark_group(format!("scan/tiny/{n}"));
        iter_group.throughput(Throughput::Elements(n as u64));
        if n >= 10_000 {
            iter_group.sample_size(10);
        }
        iter_group.bench_function(BenchmarkId::new("engine", n), |b| {
            b.iter(|| engine.scan(&paths).unwrap());
        });
        iter_group.finish();
    }

    // Mixed workload at one representative size.
    {
        let n = 2000usize;
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        build_mixed_tree(&root, n);
        let (config, paths, snapshot) = prepared_paths(&root);
        let engine = Engine::new(root.clone(), &config, snapshot, true);
        assert_equivalent(&engine, &paths);
        let mut mixed = c.benchmark_group("scan/mixed/2000");
        mixed.throughput(Throughput::Elements(paths.len() as u64));
        mixed.bench_function("engine", |b| {
            b.iter(|| engine.scan(&paths).unwrap());
        });
        mixed.finish();

        // One-file scan after a full run (the staged-check shape).
        let one = &paths[..1.min(paths.len())];
        let mut single = c.benchmark_group("scan/one_file");
        single.bench_function("engine", |b| {
            b.iter(|| engine.scan(one).unwrap());
        });
        single.finish();
    }
}

criterion_group!(benches, bench_scan);
criterion_main!(benches);
// REUSE-IgnoreEnd
