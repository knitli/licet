//! REUSE differential conformance (task 11; finding F17).
//!
//! `licet lint` against the pinned reference tool (`reuse[charset-normalizer]`
//! 6.2.0, implementing REUSE 3.3) over a compact cross-tool fixture matrix.
//! Every fixture compares semantic fields — covered paths, effective
//! references, pass/fail — never human-output strings. Mismatches are
//! recorded as documented fixtures, never blessed silently.
//!
//! The comparator runs against fully committed fixtures (tracked files), with
//! one dedicated untracked-file fixture. Absence or failure of the comparator
//! is fatal under `LICET_REQUIRE_REUSE=1`; locally a missing comparator
//! prints an explicit skip. Override the binary with `LICET_REUSE_BIN`.
//!
//! Documented divergences (same gate, different vocabulary/provenance):
//! - Deprecated SPDX ids (e.g. `GPL-2.0`): both tools fail, but licet rejects
//!   the value as invalid (never inventoried, text reported unused) while the
//!   reference reports it as deprecated-but-used. Licet's strictness is the
//!   normative reading (deprecated identifiers must not be used); no false
//!   pass is possible on either side.
//! - Undecodable license texts: licet reports incomplete validation
//!   (`unsupported_encoding`); the reference lists the entry as unused. Both
//!   fail; licet additionally refuses to claim anything about the bytes.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

/// Makes a missing/broken comparator fatal (set in CI).
const REQUIRE_ENV: &str = "LICET_REQUIRE_REUSE";
/// Override the comparator binary location.
const BIN_ENV: &str = "LICET_REUSE_BIN";

struct Comparator {
    bin: PathBuf,
}

fn comparator() -> Option<Comparator> {
    let bin = std::env::var(BIN_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("reuse"));
    match Command::new(&bin).arg("--version").output() {
        Ok(o) if o.status.success() => Some(Comparator { bin }),
        _ => {
            let msg = format!(
                "skipping differential conformance: no working `{}` comparator on PATH (set {BIN_ENV} or install 'reuse[charset-normalizer]==6.2.0')",
                bin.display()
            );
            if std::env::var(REQUIRE_ENV).as_deref() == Ok("1") {
                panic!("{msg}; {REQUIRE_ENV}=1 makes this fatal");
            }
            eprintln!("{msg}");
            None
        }
    }
}

fn strset(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}

/// One fixture evaluated by both tools.
struct Comparison {
    licet_exit: i32,
    licet_pass: bool,
    licet_coverage: BTreeSet<String>,
    licet_referenced: BTreeSet<String>,
    licet_missing: BTreeSet<String>,
    licet_unused: BTreeSet<String>,
    licet_noext: BTreeSet<String>,
    licet_unrec_stems: BTreeSet<String>,
    /// Files failing with a license problem (`missing_license` or
    /// `invalid_license` diagnostics).
    licet_license_files: BTreeSet<String>,
    licet_copyright_files: BTreeSet<String>,
    reuse_exit: i32,
    reuse_compliant: bool,
    reuse_spec: String,
    reuse_tool: String,
    reuse_files: BTreeSet<String>,
    /// Union of every per-file SPDX expression value.
    reuse_effective: BTreeSet<String>,
    reuse_missing_licensing: BTreeSet<String>,
    reuse_missing_copyright: BTreeSet<String>,
    reuse_missing_licenses: BTreeSet<String>,
    reuse_unused: BTreeSet<String>,
    reuse_noext: BTreeSet<String>,
    reuse_bad: BTreeSet<String>,
    reuse_deprecated: BTreeSet<String>,
    reuse_read_errors: BTreeSet<String>,
}

fn diag_paths(report: &serde_json::Value, code: &str) -> BTreeSet<String> {
    report["diagnostics"]
        .as_array()
        .map(|ds| {
            ds.iter()
                .filter(|d| d["code"] == code)
                .filter_map(|d| d["path"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn compare(f: &Fixture, comp: &Comparator) -> Comparison {
    let lout = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    let licet_exit = lout.status.code().unwrap_or(-1);
    let licet: serde_json::Value = serde_json::from_slice(&lout.stdout).unwrap();

    let out = Command::new(&comp.bin)
        .args(["lint", "--json"])
        .current_dir(f.path())
        .output()
        .unwrap();
    assert!(
        out.status.success() || out.status.code() == Some(1),
        "reference lint runs, got {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let reuse: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    let licet_diag = |codes: &[&str]| {
        codes
            .iter()
            .flat_map(|c| diag_paths(&licet, c))
            .collect::<BTreeSet<_>>()
    };
    let nc = &reuse["non_compliant"];
    let reuse_files: BTreeSet<String> = reuse["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["path"].as_str().map(str::to_string))
        .collect();
    let reuse_effective: BTreeSet<String> = reuse["files"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|e| {
            e["spdx_expressions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|x| x["value"].as_str().map(str::to_string))
        .collect();

    Comparison {
        licet_exit,
        licet_pass: licet["summary"]["pass"].as_bool().unwrap(),
        licet_coverage: strset(&licet["coverage"]),
        licet_referenced: strset(&licet["license_texts"]["referenced"]),
        licet_missing: strset(&licet["license_texts"]["missing"]),
        licet_unused: strset(
            &licet["license_texts"]
                .get("unused")
                .cloned()
                .unwrap_or_default(),
        ),
        licet_noext: strset(
            &licet["license_texts"]
                .get("missing_extension")
                .cloned()
                .unwrap_or_default(),
        ),
        licet_unrec_stems: licet["license_texts"]
            .get("unrecognized")
            .map(|v| {
                v.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|p| {
                        p.as_str().and_then(|s| {
                            std::path::Path::new(s)
                                .file_stem()
                                .and_then(|n| n.to_str().map(str::to_string))
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        licet_license_files: licet_diag(&["missing_license", "invalid_license"]),
        licet_copyright_files: licet_diag(&["missing_copyright"]),
        reuse_exit: out.status.code().unwrap_or(-1),
        reuse_compliant: reuse["summary"]["compliant"].as_bool().unwrap(),
        reuse_spec: reuse["reuse_spec_version"]
            .as_str()
            .unwrap_or("?")
            .to_string(),
        reuse_tool: reuse["reuse_tool_version"]
            .as_str()
            .unwrap_or("?")
            .to_string(),
        reuse_files,
        reuse_effective,
        reuse_missing_licensing: strset(&nc["missing_licensing_info"]),
        reuse_missing_copyright: strset(&nc["missing_copyright_info"]),
        reuse_missing_licenses: strset(&nc["missing_licenses"]),
        reuse_unused: strset(&nc["unused_licenses"]),
        reuse_noext: strset(&nc["licenses_without_extension"]),
        reuse_bad: strset(&nc["bad_licenses"]),
        reuse_deprecated: strset(&nc["deprecated_licenses"]),
        reuse_read_errors: strset(&nc["read_errors"]),
    }
}

impl Comparison {
    /// The gate agrees: same exit and same pass/fail.
    fn assert_gate(&self) {
        assert_eq!(self.licet_exit, self.reuse_exit, "exit codes agree");
        assert_eq!(self.licet_pass, self.reuse_compliant, "pass/fail agrees");
    }

    /// The same files are covered.
    fn assert_coverage(&self) {
        assert_eq!(self.licet_coverage, self.reuse_files, "covered paths agree");
    }

    /// The same effective license references are inventoried.
    fn assert_effective_refs(&self) {
        assert_eq!(
            self.licet_referenced, self.reuse_effective,
            "effective references agree"
        );
    }

    /// Missing / unused / extensionless text sets agree.
    fn assert_text_sets(&self) {
        assert_eq!(
            self.licet_missing, self.reuse_missing_licenses,
            "missing texts agree"
        );
        assert_eq!(self.licet_unused, self.reuse_unused, "unused texts agree");
        assert_eq!(
            self.licet_noext, self.reuse_noext,
            "missing extensions agree"
        );
    }

    /// Per-file license/copyright failure attribution agrees.
    fn assert_file_attribution(&self) {
        assert_eq!(
            self.licet_license_files, self.reuse_missing_licensing,
            "files missing licensing agree"
        );
        assert_eq!(
            self.licet_copyright_files, self.reuse_missing_copyright,
            "files missing copyright agree"
        );
    }

    /// Everything agrees: gate, coverage, references, texts, attribution.
    fn assert_full_agreement(&self) {
        self.assert_gate();
        self.assert_coverage();
        self.assert_effective_refs();
        self.assert_text_sets();
        self.assert_file_attribution();
    }
}

/// Records the pinned comparator's versions in the test output and pins the
/// REUSE specification target (3.3); the tool version itself is evidence only.
#[test]
fn comparator_versions_are_recorded() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .texts(&["MIT"])
    .commit("init");
    let c = compare(&f, &comp);
    eprintln!(
        "differential comparator: reuse tool {}, REUSE spec {}",
        c.reuse_tool, c.reuse_spec
    );
    assert_eq!(c.reuse_spec, "3.3", "the pinned tool must target REUSE 3.3");
    assert!(c.reuse_bad.is_empty(), "no bad licenses on a clean fixture");
    assert!(
        c.reuse_read_errors.is_empty(),
        "no read errors on a clean fixture"
    );
    c.assert_full_agreement();
}

/// Write a bundled license text into the fixture.
fn bundled<'a>(f: &'a Fixture, id: &str) -> &'a Fixture {
    f.write(
        &format!("LICENSES/{id}.txt"),
        licet::spdx::bundled_text(id).unwrap_or_else(|| panic!("no bundled text for {id}")),
    )
}

/// Audited disagreement 1 (F08): a header without copyright fails on both.
#[test]
fn diff_header_without_copyright_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write("a.rs", "// SPDX-License-Identifier: MIT\nfn a(){}\n");
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// Audited disagreement 2 (F08): two header licenses, one text — both fail.
#[test]
fn diff_two_licenses_one_text_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-License-Identifier: Apache-2.0\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!(c.licet_missing, BTreeSet::from(["Apache-2.0".to_string()]));
    c.assert_full_agreement();
}

/// Audited disagreement 3 (F08): a late-snippet license is inventoried and
/// required by both — never silently dropped.
#[test]
fn diff_late_snippet_license_required_by_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    let mut body =
        String::from("// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\n");
    while body.len() < 10 * 1024 {
        body.push_str("// filler to push the snippet late\n");
    }
    body.push_str("// SPDX-SnippetBegin: s1\n// SPDX-License-Identifier: Apache-2.0\n// SPDX-SnippetEnd: s1\n");
    f.write("a.rs", &body);
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert!(c.licet_referenced.contains("Apache-2.0"));
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// Audited disagreement 4 (F08): a deep headerless file under an annotation
/// fails copyright and license on both.
#[test]
fn diff_deep_annotated_file_without_metadata_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"src/*\"\nSPDX-License-Identifier = \"MIT\"\n",
    )
    .write("src/deep/a.rs", "fn a(){}\n");
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// Audited disagreement 5 (F08): a valid license array with both texts
/// passes on both — one table must not discard the other.
#[test]
fn diff_license_array_with_both_texts_passes_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = [\"MIT\", \"Apache-2.0\"]\nSPDX-FileCopyrightText = \"2026 T\"\n",
    )
    .write("a.rs", "fn a(){}\n");
    bundled(&f, "MIT");
    bundled(&f, "Apache-2.0");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// A snippet-only file carries its license and copyright in the snippet: the
/// reference tool flattens snippet notices into the file's info, and lint
/// counts them the same way (declared policy never does).
#[test]
fn diff_snippet_only_file_passes_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "s.rs",
        "// SPDX-SnippetBegin: s1\n// SPDX-FileCopyrightText: 2026 Snip\n// SPDX-License-Identifier: Apache-2.0\n// SPDX-SnippetEnd: s1\nfn x(){}\n",
    );
    bundled(&f, "Apache-2.0");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// Nested REUSE documents resolve root-first: the nearest `closest` license
/// wins (the file keeps its copyright fallback), so the root's MIT text is
/// unused on both sides and both fail for it.
#[test]
fn diff_nested_precedence_agrees() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
    )
    .write(
        "sub/REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
    )
    .write("sub/f.rs", "fn x(){}\n");
    bundled(&f, "MIT");
    bundled(&f, "Apache-2.0");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    assert_eq!(c.licet_unused, BTreeSet::from(["MIT".to_string()]));
    c.assert_full_agreement();
}

/// An `override` barrier suppresses the in-file header: both tools evaluate
/// the annotation's license — and both still fail the missing copyright.
#[test]
fn diff_override_barrier_agrees() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\nprecedence = \"override\"\n",
    )
    .write("a.rs", "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n");
    bundled(&f, "MIT");
    bundled(&f, "Apache-2.0");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!(
        c.licet_referenced,
        BTreeSet::from(["Apache-2.0".to_string()])
    );
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// A binary asset covered by a sidecar passes on both.
#[test]
fn diff_sidecar_binary_passes_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "asset.bin.license",
        "SPDX-License-Identifier: MIT\nSPDX-FileCopyrightText: 2026 T\n",
    );
    bundled(&f, "MIT");
    std::fs::write(f.path().join("asset.bin"), [0x00, 0xFF, 0x89, 0x50]).unwrap();
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// An extensionless license text satisfies presence on both, and both flag
/// its missing extension (REUSE 3.3 requires one); the unused text fails both.
#[test]
fn diff_extensionless_and_unused_texts_fail_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("LICENSES/MIT", licet::spdx::bundled_text("MIT").unwrap())
    .write(
        "LICENSES/Apache-2.0.txt",
        licet::spdx::bundled_text("Apache-2.0").unwrap(),
    );
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// Zero-byte files, symlinks, and SPDX documents are ignored by both.
#[test]
fn diff_ignored_files_agree() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("empty.rs", "")
    .write("doc.spdx", "SPDXVersion: SPDX-2.3\nPackageName: x\n");
    bundled(&f, "MIT");
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.path().join("a.rs"), f.path().join("link.rs")).unwrap();
    f.commit("init");
    let c = compare(&f, &comp);
    assert!(!c.licet_coverage.contains("empty.rs"));
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// Legacy DEP5 coverage aggregates with file-level metadata on both.
#[test]
fn diff_dep5_coverage_passes_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        ".reuse/dep5",
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\nFiles: src/*\nCopyright: 2026 T\nLicense: MIT\n",
    )
    .write("src/f.c", "fn x(){}\n");
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// A custom LicenseRef with its text passes on both; without it both fail.
#[test]
fn diff_licenseref_text_present_and_missing() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: LicenseRef-Acme-1.0\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("LICENSES/LicenseRef-Acme-1.0.txt", "Acme license text.\n");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);

    let g = Fixture::new();
    g.write(
        "a.rs",
        "// SPDX-License-Identifier: LicenseRef-Acme-1.0\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    g.commit("init");
    let c = compare(&g, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// An invalid license expression fails on both with the file attributed.
#[test]
fn diff_invalid_expression_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT OR\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_coverage();
    c.assert_file_attribution();
}

/// DOCUMENTED DIVERGENCE — deprecated SPDX ids: both tools fail, but licet
/// rejects the value as invalid (never inventoried, text unused) while the
/// reference reports it as deprecated-but-used. Same gate, same coverage.
#[test]
fn diff_deprecated_id_fails_both_with_documented_vocabulary() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: GPL-2.0\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("LICENSES/GPL-2.0.txt", "Deprecated license text.\n");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_coverage();
    assert_eq!(c.reuse_deprecated, BTreeSet::from(["GPL-2.0".to_string()]));
    assert!(diag_paths(&licet_report(&f), "invalid_license").contains("a.rs"));
}

/// An unclosed snippet is treated as running to end-of-input by both.
#[test]
fn diff_unclosed_snippet_agrees() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-FileCopyrightText: 2026 T\n// SPDX-SnippetBegin: s1\n// SPDX-License-Identifier: Apache-2.0\nfn x(){}\n",
    );
    bundled(&f, "Apache-2.0");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
}

/// REUSE.toml and DEP5 together are mutually exclusive: both tools refuse
/// with exit 2 rather than guessing.
#[test]
fn diff_reuse_and_dep5_coexistence_refused_by_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write(
        "REUSE.toml",
        "version = 1\n[[annotations]]\npath = \"a.rs\"\nSPDX-License-Identifier = \"MIT\"\n",
    )
    .write(
        ".reuse/dep5",
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
    );
    bundled(&f, "MIT");
    f.commit("init");

    let lout = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(lout.status.code(), Some(2));

    let rout = Command::new(&comp.bin)
        .args(["lint", "--json"])
        .current_dir(f.path())
        .output()
        .unwrap();
    assert_eq!(rout.status.code(), Some(2));
}

/// A malformed REUSE document fails before any evaluation on both sides.
#[test]
fn diff_malformed_reuse_document_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("REUSE.toml", "version = 1\n[[annotations\n");
    bundled(&f, "MIT");
    f.commit("init");

    let lout = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(lout.status.code(), Some(2));

    let rout = Command::new(&comp.bin)
        .args(["lint", "--json"])
        .current_dir(f.path())
        .output()
        .unwrap();
    assert_ne!(rout.status.code(), Some(0));
}

/// Tracked-plus-untracked coverage agrees: an uncommitted file is evaluated
/// by both tools, not just the committed tree.
#[test]
fn diff_untracked_file_covered_by_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    bundled(&f, "MIT");
    f.commit("init");
    // Untracked after the commit: covered by lint on both sides.
    f.write("u.rs", "// SPDX-License-Identifier: MIT\nfn u(){}\n");
    let c = compare(&f, &comp);
    assert!(c.licet_coverage.contains("u.rs"));
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_full_agreement();
}

/// An undecodable source file fails validation on both (never a pass).
#[test]
fn diff_unreadable_source_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    bundled(&f, "MIT");
    std::fs::write(f.path().join("bin.rs"), [0xFF, 0xFE, 0x00]).unwrap();
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_coverage();
    c.assert_file_attribution();
}

/// DOCUMENTED DIVERGENCE — undecodable license texts: licet reports
/// incomplete validation (`unsupported_encoding`) while the reference lists
/// the entry as unused. Both fail; licet refuses to claim anything about
/// bytes it cannot read.
#[test]
fn diff_unreadable_text_fails_both_with_documented_vocabulary() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    );
    bundled(&f, "MIT");
    std::fs::write(f.path().join("LICENSES/Apache-2.0.txt"), [0xFF, 0xFE]).unwrap();
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    c.assert_coverage();
    assert!(
        diag_paths(&licet_report(&f), "unsupported_encoding").contains("LICENSES/Apache-2.0.txt")
    );
}

/// An unrecognized LICENSES entry fails both; names map by file stem.
#[test]
fn diff_unrecognized_entry_fails_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("LICENSES/junk.txt", "hello\n");
    bundled(&f, "MIT");
    f.commit("init");
    let c = compare(&f, &comp);
    assert_eq!((c.licet_exit, c.reuse_exit), (1, 1));
    assert_eq!(c.licet_unrec_stems, c.reuse_bad);
}

/// An empty but correctly named text satisfies presence on both: neither
/// tool claims to prove legal correctness of file contents.
#[test]
fn diff_empty_text_accepted_by_both() {
    let Some(comp) = comparator() else { return };
    let f = Fixture::new();
    f.write(
        "a.rs",
        "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 T\nfn a(){}\n",
    )
    .write("LICENSES/MIT.txt", "");
    f.commit("init");
    let c = compare(&f, &comp);
    c.assert_full_agreement();
    assert!(c.licet_pass && c.reuse_compliant);
}

/// Helper for single-diagnostic assertions: the licet lint JSON report.
fn licet_report(f: &Fixture) -> serde_json::Value {
    let out = f
        .licet()
        .args(["lint", "--format", "json"])
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}
// REUSE-IgnoreEnd
