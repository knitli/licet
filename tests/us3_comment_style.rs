//! US3 — new comment style persists & round-trips (SC-005, FR-010, FR-011).
//! Quickstart Scenario 3.
// REUSE-IgnoreStart — SPDX tags below are test fixtures, not this file's licensing.

mod common;
use common::Fixture;

#[test]
fn pkl_gets_c_style_header_and_round_trips() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"LicenseRef-MarqueLicense-1.0\"\n\
         [[comment_style]]\next=\"pkl\"\nstyle=\"c\"\n",
    )
    .write("hk.pkl", "amends \"...\"\n")
    // Policy check requires the referenced custom text to exist.
    .write(
        "LICENSES/LicenseRef-MarqueLicense-1.0.txt",
        "Custom marque license text.\n",
    )
    .commit("init");

    // Apply writes a header using the configured C (//) style — no per-file flag.
    let apply = f
        .licet()
        .args(["apply", "--files", "hk.pkl"])
        .output()
        .unwrap();
    assert_eq!(
        apply.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&apply.stdout)
    );
    let content = f.read("hk.pkl");
    assert!(
        content.contains("// SPDX-License-Identifier: LicenseRef-MarqueLicense-1.0"),
        "{content}"
    );

    // The subsequent check recognizes the header (compliant) — persisted association.
    let check = f
        .licet()
        .args(["check", "--files", "hk.pkl"])
        .output()
        .unwrap();
    assert_eq!(
        check.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&check.stdout)
    );
}

#[test]
fn inline_custom_style_is_used() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"MIT\"\n\
         [[comment_style]]\nfile=\"Jenkinsfile\"\nstyle={ line_prefix = \"//\" }\n",
    )
    .write("Jenkinsfile", "pipeline {}\n")
    .commit("init");

    f.licet()
        .args(["apply", "--files", "Jenkinsfile"])
        .output()
        .unwrap();
    let content = f.read("Jenkinsfile");
    assert!(
        content.starts_with("// SPDX-License-Identifier: MIT"),
        "{content}"
    );
}

#[test]
fn additive_block_style_appends_self_contained_line() {
    // Additive apply on a block-only style replays the style's closer on the
    // appended line (derived from the registry, not the file bytes), keeping
    // every record inside a comment.
    let f = Fixture::new();
    f.config("[default]\nlicense=\"Apache-2.0\"\n")
        .write(
            "page.html",
            "<!-- SPDX-License-Identifier: MIT -->\n<p>hi</p>\n",
        )
        .commit("init");

    let out = f
        .licet()
        .args(["apply", "--additive", "--files", "page.html"])
        .output()
        .unwrap();
    // Additive writes that all succeed but leave declaration drift (the old
    // record still counts) are exit 1, not partial: the tool did its job,
    // the declarations still disagree.
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        f.read("page.html"),
        "<!-- SPDX-License-Identifier: MIT -->\n<!-- SPDX-License-Identifier: Apache-2.0 -->\n<p>hi</p>\n"
    );
}

/// Invalid configuration fails before any write: an unknown comment-style
/// alias is a usage error (exit 2) and `apply` changes nothing.
#[test]
fn unknown_comment_style_alias_fails_before_writes() {
    let f = Fixture::new();
    f.config(
        "[default]\nlicense=\"MIT\"\n[[comment_style]]\next=\"rs\"\nstyle=\"no-such-style\"\n",
    )
    .write("a.rs", "fn a(){}\n")
    .commit("init");

    let out = f
        .licet()
        .args(["apply", "--allow-dirty", "--files", "a.rs"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unknown comment style"), "{stderr}");
    assert_eq!(f.read("a.rs"), "fn a(){}\n", "no write on invalid config");
}

/// An empty `add:` copyright is a usage error, not an empty rendered line.
#[test]
fn empty_copyright_text_fails_before_writes() {
    let f = Fixture::new();
    f.config("[default]\nlicense=\"MIT\"\ncopyright=\"add:\"\n")
        .write("a.rs", "fn a(){}\n")
        .commit("init");

    let out = f
        .licet()
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(f.read("a.rs"), "fn a(){}\n", "no write on invalid config");
}
// REUSE-IgnoreEnd
