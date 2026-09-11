// REUSE-IgnoreStart — SPDX tags in the tests below are fixtures, not this file's licensing.
use super::*;
use std::path::PathBuf;

use crate::domain::OobSource;

fn load_doc(text: &str) -> OutOfBand {
    let mut oob = OutOfBand::default();
    oob.parse_reuse_toml(Path::new("REUSE.toml"), text).unwrap();
    oob
}

#[test]
fn parses_reuse_toml_glob() {
    // A `closest` table is fallback only: its values land in the fallback
    // fields, leaving the unconditional ones empty for detection to fill
    // from file-level info first.
    let oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"assets/**\"\n\
             SPDX-License-Identifier = \"CC0-1.0\"\nSPDX-FileCopyrightText = \"2026 Acme\"\n",
    );
    let e = oob.lookup(&PathBuf::from("assets/logo.png")).unwrap();
    assert!(e.licenses.is_empty());
    assert_eq!(e.fallback_licenses, vec!["CC0-1.0".to_string()]);
    assert_eq!(e.fallback_copyrights, vec!["2026 Acme".to_string()]);
    assert_eq!(e.precedence, Precedence::Closest);
    assert!(!e.suppresses_file);
}

#[test]
fn license_array_stays_together() {
    // Multiple expressions in one table apply together (aggregate here so
    // they are unconditional); the presentation form AND-combines them.
    let oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"a.bin\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = [\"MIT\", \"Apache-2.0\"]\n",
    );
    let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
    assert_eq!(
        e.licenses,
        vec!["MIT".to_string(), "Apache-2.0".to_string()]
    );
    assert_eq!(e.license().as_deref(), Some("MIT AND Apache-2.0"));
}

#[test]
fn parses_dep5() {
    let mut oob = OutOfBand::default();
    oob.parse_dep5("Files: img/*\nCopyright: 2026 Acme\nLicense: MIT\n")
        .unwrap();
    let e = oob.lookup(&PathBuf::from("img/x.jpg")).unwrap();
    assert_eq!(e.licenses, vec!["MIT".to_string()]);
    assert_eq!(e.source, OobSource::Dep5);
    assert_eq!(e.precedence, Precedence::Aggregate);
}

#[test]
fn dep5_tolerates_cr_terminated_input() {
    // dep5 files in the wild use CRLF or bare-CR endings; every value
    // path trims, so carriage returns never leak into parsed values.
    let mut oob = OutOfBand::default();
    oob.parse_dep5("Files: a.bin\r\nLicense: MIT\r").unwrap();
    let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
    assert_eq!(e.licenses, vec!["MIT".to_string()]);
}

#[test]
fn dep5_star_crosses_directories() {
    // Unlike REUSE.toml, dep5 `*` matches across `/` (reference behavior).
    let mut oob = OutOfBand::default();
    oob.parse_dep5("Files: *.bin\nLicense: MIT\n").unwrap();
    assert!(oob.lookup(&PathBuf::from("sub/b.bin")).is_some());
}

#[test]
fn dep5_continuation_lines_unfold() {
    let mut oob = OutOfBand::default();
    oob.parse_dep5(
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
             \n\
             Files: a.bin\n sub/b.bin\n\
             Copyright: 2026 A\n 2026 B\n .\n\
             License: MIT\n",
    )
    .unwrap();
    let e = oob.lookup(&PathBuf::from("sub/b.bin")).unwrap();
    assert_eq!(e.licenses, vec!["MIT".to_string()]);
    assert_eq!(
        e.copyrights,
        vec!["2026 A".to_string(), "2026 B".to_string()]
    );
}

#[test]
fn unfold_dep5_paragraphs_joins_continuations() {
    // Raw unfolding: continuation lines join with a space, a lone `.` is
    // blank, field names match ASCII-case-insensitively, unknown fields
    // neither set state nor leak content, and repeated `Files:` accumulate.
    let paras = super::parse::unfold_dep5_paragraphs(
        "Format: https://example.test/spec\n\
         \n\
         FILES: a.bin\n sub/b.bin\n\
         copyright: 2026 A\n 2026 B\n .\n\
         Upstream-Name: ignored\n continued-ignored\n\
         License: MIT\n OR Apache-2.0\n\
         \n\
         Files: c.bin\n\
         Copyright: 2027 C\n",
    );
    assert_eq!(paras.len(), 3);
    assert_eq!(paras[0].files, "");
    assert_eq!(paras[1].files, "a.bin sub/b.bin");
    assert_eq!(
        paras[1].copyright,
        vec!["2026 A".to_string(), "2026 B".to_string()]
    );
    assert_eq!(paras[1].license, "MIT OR Apache-2.0");
    assert_eq!(paras[2].files, "c.bin");
    assert_eq!(paras[2].copyright, vec!["2027 C".to_string()]);
    assert_eq!(paras[2].license, "");
}

#[test]
fn dep5_last_paragraph_wins() {
    // Two paragraphs covering the same file: the last one governs, exactly
    // as the reference tool reports.
    let mut oob = OutOfBand::default();
    oob.parse_dep5(
            "Files: a.bin\nCopyright: 2026 A\nLicense: MIT\n\nFiles: a.bin\nCopyright: 2026 B\nLicense: Apache-2.0\n",
        )
        .unwrap();
    let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
    assert_eq!(e.licenses, vec!["Apache-2.0".to_string()]);
    assert_eq!(e.copyrights, vec!["2026 B".to_string()]);
    assert_eq!(e.origins.len(), 1);
    assert_eq!(e.origins[0].table_index, 1);
}

#[test]
fn dep5_field_names_are_case_insensitive() {
    let mut oob = OutOfBand::default();
    oob.parse_dep5("files: a.bin\ncopyright: 2026 A\nlicense: MIT\n")
        .unwrap();
    let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
    assert_eq!(e.licenses, vec!["MIT".to_string()]);
}

#[test]
fn no_match_is_none() {
    let oob = OutOfBand::default();
    assert!(oob.lookup(&PathBuf::from("x")).is_none());
}

#[test]
fn malformed_documents_fail_with_location() {
    for (name, text) in [
        (
            "bad-toml",
            "version = 1\n[[annotations]]\npath = \nSPDX-License-Identifier = \"MIT\"\n",
        ),
        ("version-2", "version = 2\n[[annotations]]\npath = \"a\"\n"),
        (
            "missing-path",
            "version = 1\n[[annotations]]\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "empty-path",
            "version = 1\n[[annotations]]\npath = []\nSPDX-License-Identifier = \"MIT\"\n",
        ),
        (
            "bad-precedence",
            "version = 1\n[[annotations]]\npath = \"a\"\nprecedence = \"bogus\"\n",
        ),
        (
            "capital-precedence",
            "version = 1\n[[annotations]]\npath = \"a\"\nprecedence = \"Closest\"\n",
        ),
        (
            "bad-expression",
            "version = 1\n[[annotations]]\npath = \"a\"\nSPDX-License-Identifier = \"NOT-A-LICENSE\"\n",
        ),
    ] {
        let mut oob = OutOfBand::default();
        let err = oob
            .parse_reuse_toml(Path::new("REUSE.toml"), text)
            .expect_err(&format!("{name} must fail"));
        let msg = err.to_string();
        assert!(
            msg.contains("REUSE.toml"),
            "{name}: error names the document: {msg}"
        );
    }
    let mut oob = OutOfBand::default();
    let err = oob
        .parse_dep5("Files: a.bin\nLicense: NOT-A-LICENSE\n")
        .expect_err("bad dep5 License must fail");
    assert!(err.to_string().contains(".reuse/dep5"), "{err}");
}

#[test]
fn missing_version_fails() {
    let mut oob = OutOfBand::default();
    let err = oob
        .parse_reuse_toml(Path::new("REUSE.toml"), "[[annotations]]\npath = \"a\"\n")
        .expect_err("missing version must fail");
    assert!(err.to_string().contains("REUSE.toml"), "{err}");
}

#[test]
fn unknown_keys_are_ignored() {
    // The REUSE schema explicitly permits extension keys.
    let oob = load_doc(
        "version = 1\n[tool.extra]\nnote = 1\n[[annotations]]\npath = \"a\"\n\
             SPDX-License-Identifier = \"MIT\"\nSPDX-FileComment = \"hi\"\n",
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("a")).unwrap().fallback_licenses,
        vec!["MIT".to_string()]
    );
}

#[test]
fn nested_documents_resolve_root_first() {
    let mut oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nSPDX-License-Identifier = \"MIT\"\n\
             SPDX-FileCopyrightText = \"2026 Root\"\n",
    );
    oob.parse_reuse_toml(
        Path::new("sub/REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"f.rs\"\nSPDX-License-Identifier = \"Apache-2.0\"\n",
    )
    .unwrap();
    // Per-field nearest-outward fallback: the child's license wins, but
    // the child says nothing about copyright so the root still supplies it.
    let e = oob.lookup(&PathBuf::from("sub/f.rs")).unwrap();
    assert_eq!(e.fallback_licenses, vec!["Apache-2.0".to_string()]);
    assert_eq!(e.fallback_copyrights, vec!["2026 Root".to_string()]);
    assert_eq!(e.origins.len(), 2);
    assert_eq!(
        e.origins[0].metadata_path,
        PathBuf::from("REUSE.toml"),
        "origins read shallowest-document first"
    );
    assert_eq!(e.origins[1].metadata_path, PathBuf::from("sub/REUSE.toml"));
}

#[test]
fn nested_patterns_cannot_escape_their_directory() {
    let mut oob = OutOfBand::default();
    oob.parse_reuse_toml(
        Path::new("sub/REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"**\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    assert!(oob.lookup(&PathBuf::from("sub/a.rs")).is_some());
    assert!(
        oob.lookup(&PathBuf::from("other.rs")).is_none(),
        "a nested document never covers its parent"
    );
    assert!(
        oob.lookup(&PathBuf::from("REUSE.toml")).is_none(),
        "metadata documents are not covered by nested globs"
    );
}

#[test]
fn override_barrier_suppresses_deeper_tables() {
    let mut oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"sub/f.rs\"\nprecedence = \"override\"\n\
             SPDX-License-Identifier = \"MIT\"\nSPDX-FileCopyrightText = \"2026 Root\"\n",
    );
    oob.parse_reuse_toml(
        Path::new("sub/REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"f.rs\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"Apache-2.0\"\n",
    )
    .unwrap();
    let e = oob.lookup(&PathBuf::from("sub/f.rs")).unwrap();
    assert!(e.suppresses_file);
    assert_eq!(e.precedence, Precedence::Override);
    assert_eq!(e.licenses, vec!["MIT".to_string()]);
    assert!(
        !e.licenses.contains(&"Apache-2.0".to_string()),
        "the deeper aggregate is suppressed by the rootmost override"
    );
}

#[test]
fn empty_override_still_suppresses() {
    // A field-less override table contributes nothing but still
    // suppresses file-level info (reference behavior).
    let oob =
        load_doc("version = 1\n[[annotations]]\npath = \"a.bin\"\nprecedence = \"override\"\n");
    let e = oob.lookup(&PathBuf::from("a.bin")).unwrap();
    assert!(e.suppresses_file);
    assert!(e.licenses.is_empty() && e.copyrights.is_empty());
}

#[test]
fn reuse_pattern_grammar() {
    // `*` never crosses `/`; `**` does; `?[]{} ` are literal; `\` escapes.
    let oob = load_doc(
        "version = 1\n\
             [[annotations]]\npath = \"*.png\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"MIT\"\n\
             [[annotations]]\npath = \"a?.png\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"Apache-2.0\"\n\
             [[annotations]]\npath = \"deep/**\"\nprecedence = \"aggregate\"\nSPDX-License-Identifier = \"CC0-1.0\"\n",
    );
    // Last match wins within the document.
    assert_eq!(
        oob.lookup(&PathBuf::from("a?.png")).unwrap().licenses,
        vec!["Apache-2.0".to_string()],
        "`?` is literal: the exact file matches the literal pattern"
    );
    // `*.png` must not have matched `a?.png` via wildcard, or last-match
    // would still hold — check a file only `*` can match instead.
    assert!(
        oob.lookup(&PathBuf::from("sub/a.png")).is_none(),
        "`*` must not cross `/`"
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("deep/nest/a.png"))
            .unwrap()
            .licenses,
        vec!["CC0-1.0".to_string()],
        "`**` crosses directories"
    );
}

#[test]
fn reuse_escapes_match_verbatim() {
    let mut oob = OutOfBand::default();
    // TOML `"star\\\\*.bin"` is the pattern `star\*.bin`: a literal star.
    oob.parse_reuse_toml(
        Path::new("REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"star\\\\*.bin\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    assert!(oob.lookup(&PathBuf::from("star*.bin")).is_some());
    assert!(oob.lookup(&PathBuf::from("starX.bin")).is_none());
}

#[test]
fn braces_and_brackets_are_literal() {
    let oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"a[0].{png,bin}\"\nprecedence = \"aggregate\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    );
    assert!(oob.lookup(&PathBuf::from("a[0].{png,bin}")).is_some());
    assert!(oob.lookup(&PathBuf::from("a0.png")).is_none());
}

#[test]
fn coexistence_with_dep5_fails() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("REUSE.toml"),
        "version = 1\n[[annotations]]\npath = \"a\"\nSPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join(".reuse")).unwrap();
    std::fs::write(
        dir.path().join(".reuse/dep5"),
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n",
    )
    .unwrap();
    let err = OutOfBand::load(dir.path()).expect_err("coexistence must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("REUSE.toml") && msg.contains(".reuse/dep5"),
        "{msg}"
    );
}

/// The scan's hierarchy as the write probe sees it.
fn load_root(root: &std::path::Path) -> OutOfBand {
    OutOfBand::load(root).unwrap()
}

#[test]
fn write_annotation_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cprs = vec!["2026 Acme".to_string()];
    assert_eq!(
        write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let first = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    // Second write of the same path with the same license is a no-op.
    assert_eq!(
        write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
        AnnotationWrite::Unchanged
    );
    let second = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.matches("path = \"logo.png\"").count(), 1);
    assert!(first.starts_with("version = 1"));
}

#[test]
fn write_annotation_appends_superseding_stanza_for_same_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cprs = vec!["2026 Acme".to_string()];
    write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap();
    // A new intent for the same exact path appends a superseding stanza;
    // the old one is left byte-for-byte (last match wins per REUSE 3.3).
    assert_eq!(
        write_annotation(root, "logo.png", "CC-BY-4.0", &cprs, &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert_eq!(text.matches("path = \"logo.png\"").count(), 2);
    assert!(text.contains("SPDX-License-Identifier = \"CC0-1.0\""));
    assert!(text.contains("SPDX-License-Identifier = \"CC-BY-4.0\""));

    // The appended stanza wins through lookup (a `closest` table surfaces
    // as the fallback until file-level info exists) …
    let mut oob = OutOfBand::default();
    oob.parse_reuse_toml(Path::new("REUSE.toml"), &text)
        .unwrap();
    assert_eq!(
        oob.lookup(&PathBuf::from("logo.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC-BY-4.0".to_string()]
    );
    // … and the rerun converges to a no-op.
    assert_eq!(
        write_annotation(root, "logo.png", "CC-BY-4.0", &cprs, &load_root(root)).unwrap(),
        AnnotationWrite::Unchanged
    );
}

#[test]
fn write_annotation_refuses_malformed_document() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("REUSE.toml"), "[[annotations\npath = \n").unwrap();
    let before = std::fs::read(root.join("REUSE.toml")).unwrap();
    // The probe hierarchy is empty (the scan would already have failed on
    // this document); the re-parse gate still refuses to append to it.
    let err = write_annotation(root, "logo.png", "CC0-1.0", &[], &OutOfBand::default())
        .expect_err("malformed REUSE.toml must fail closed");
    assert!(err.to_string().contains("re-parse"), "{err}");
    // The broken document is left untouched — nothing appended.
    assert_eq!(std::fs::read(root.join("REUSE.toml")).unwrap(), before);
}

#[test]
fn write_annotation_appends_override_for_glob() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("REUSE.toml"),
        "version = 1\n\n[[annotations]]\npath = \"*.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    // A glob covers the file; we cannot edit it without affecting siblings, so we
    // append a more-specific exact-path block. Last match wins.
    assert_eq!(
        write_annotation(root, "logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    let mut oob = OutOfBand::default();
    oob.parse_reuse_toml(Path::new("REUSE.toml"), &text)
        .unwrap();
    // Both tables are `closest` fallbacks without file-level info, so the
    // last match governs each file.
    assert_eq!(
        oob.lookup(&PathBuf::from("logo.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC-BY-4.0".to_string()],
        "exact override must win over the glob"
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("other.png"))
            .unwrap()
            .fallback_licenses,
        vec!["MIT".to_string()],
        "the glob still governs its other files"
    );
}

#[test]
fn write_annotations_batch_many_files_single_doc_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let empty: Vec<String> = vec![];
    let acme = vec!["2026 Acme".to_string()];
    let outcome = write_annotations(
        root,
        &[
            AnnotationRequest {
                rel_path: "a.png",
                license: "CC0-1.0",
                copyrights: &empty,
            },
            AnnotationRequest {
                rel_path: "b.png",
                license: "MIT",
                copyrights: &acme,
            },
        ],
        &load_root(root),
    )
    .unwrap();
    assert_eq!(
        outcome.per_request,
        vec![AnnotationWrite::Appended, AnnotationWrite::Appended]
    );
    // One document touched, one patch carrying both requests.
    assert_eq!(outcome.patches.len(), 1);
    assert_eq!(outcome.patches[0].doc_rel, PathBuf::from("REUSE.toml"));
    assert_eq!(outcome.patches[0].requests, vec![0, 1]);
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert_eq!(text.matches("[[annotations]]").count(), 2);
    // Both resolve, and a rerun over the same batch converges entirely.
    let oob = load_root(root);
    assert_eq!(
        oob.lookup(&PathBuf::from("a.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC0-1.0".to_string()]
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("b.png"))
            .unwrap()
            .fallback_licenses,
        vec!["MIT".to_string()]
    );
    let rerun = write_annotations(
        root,
        &[
            AnnotationRequest {
                rel_path: "a.png",
                license: "CC0-1.0",
                copyrights: &empty,
            },
            AnnotationRequest {
                rel_path: "b.png",
                license: "MIT",
                copyrights: &acme,
            },
        ],
        &oob,
    )
    .unwrap();
    assert_eq!(
        rerun.per_request,
        vec![AnnotationWrite::Unchanged, AnnotationWrite::Unchanged]
    );
    assert!(rerun.patches.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("REUSE.toml")).unwrap(),
        text
    );
}

#[test]
fn multiple_copyrights_serialize_as_list() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cprs = vec!["2026 Acme".to_string(), "2027 Acme".to_string()];
    assert_eq!(
        write_annotation(root, "logo.png", "CC0-1.0", &cprs, &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    // One string/list field, never repeated keys (repeated keys are
    // invalid TOML and would fail the re-parse gate).
    assert_eq!(text.matches("SPDX-FileCopyrightText").count(), 1, "{text}");
    assert!(
        text.contains("SPDX-FileCopyrightText = [\"2026 Acme\", \"2027 Acme\"]"),
        "{text}"
    );
    let oob = load_root(root);
    assert_eq!(
        oob.lookup(&PathBuf::from("logo.png"))
            .unwrap()
            .fallback_copyrights,
        cprs
    );
}

#[test]
fn render_stanzas_covers_copyright_shapes() {
    // Pure stanza rendering: absent/single/list copyright fields, the
    // `override` stanza flag, glob-metachar escaping, and the document's
    // newline convention — plus exactly which requests each stanza covers.
    use super::write::{AnnotationDestination, AnnotationRequest, render_stanzas};
    let cprs = vec!["2026 Acme".to_string(), "2027 Acme".to_string()];
    let requests = vec![
        AnnotationRequest {
            rel_path: "a.png",
            license: "MIT",
            copyrights: &[],
        },
        AnnotationRequest {
            rel_path: "sub/b.png",
            license: "Apache-2.0",
            copyrights: &cprs[..1],
        },
        AnnotationRequest {
            rel_path: "a*b.png",
            license: "CC0-1.0",
            copyrights: &cprs,
        },
    ];
    let dest = |i: usize, base: &str, use_override: bool| {
        (
            i,
            AnnotationDestination {
                doc_rel: PathBuf::from("REUSE.toml"),
                base_path: base.to_string(),
                use_override,
            },
        )
    };
    let jobs = vec![
        dest(0, "a.png", false),
        dest(1, "b.png", true),
        dest(2, "a*b.png", false),
    ];
    let (text, appended) = render_stanzas(&jobs, &requests, "\r\n");
    assert_eq!(appended, vec![0, 1, 2]);
    assert_eq!(
        text,
        "[[annotations]]\r\n\
         path = \"a.png\"\r\n\
         SPDX-License-Identifier = \"MIT\"\r\n\
         \r\n\
         [[annotations]]\r\n\
         path = \"b.png\"\r\n\
         precedence = \"override\"\r\n\
         SPDX-FileCopyrightText = \"2026 Acme\"\r\n\
         SPDX-License-Identifier = \"Apache-2.0\"\r\n\
         \r\n\
         [[annotations]]\r\n\
         path = \"a\\\\*b.png\"\r\n\
         SPDX-FileCopyrightText = [\"2026 Acme\", \"2027 Acme\"]\r\n\
         SPDX-License-Identifier = \"CC0-1.0\"\r\n\
         \r\n"
    );
}

#[test]
fn glob_metachar_path_is_escaped_to_literal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(
        write_annotation(root, "a*b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    // Two layers of escaping: TOML decodes `\\` to `\`, leaving the
    // REUSE-level `\`-asterisk escape the matcher (and the reference
    // tool) reads as a literal star.
    assert!(text.contains("path = \"a\\\\*b.png\""), "{text}");
    let oob = load_root(root);
    assert!(oob.lookup(&PathBuf::from("a*b.png")).is_some());
    assert!(
        oob.lookup(&PathBuf::from("aXb.png")).is_none(),
        "escaped path must not act as a glob"
    );
}

#[test]
fn crlf_document_keeps_crlf_on_append() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("REUSE.toml"),
        "version = 1\r\n\r\n[[annotations]]\r\npath = \"a.png\"\r\n\
             SPDX-License-Identifier = \"MIT\"\r\n",
    )
    .unwrap();
    assert_eq!(
        write_annotation(root, "b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert!(!text.contains('\n') || text.contains("\r\n"), "{text:?}");
    assert!(
        text.contains("[[annotations]]\r\npath = \"b.png\"\r\n"),
        "{text:?}"
    );
    // Still parses: the appended stanza reuses the document convention.
    load_root(root)
        .lookup(&PathBuf::from("b.png"))
        .expect("b.png covered");
}

#[test]
fn two_holder_broad_annotation_gets_exact_exception() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = "version = 1\n\n[[annotations]]\npath = [\"a.png\", \"b.png\"]\n\
             SPDX-FileCopyrightText = [\"2026 Acme\", \"2027 Acme\"]\n\
             SPDX-License-Identifier = \"MIT\"\n";
    std::fs::write(root.join("REUSE.toml"), before).unwrap();
    // Narrowing a.png must not rewrite the shared stanza: append an exact
    // exception carrying both holders.
    assert_eq!(
        write_annotation(
            root,
            "a.png",
            "CC-BY-4.0",
            &["2026 Acme".to_string(), "2027 Acme".to_string()],
            &load_root(root),
        )
        .unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert!(
        text.starts_with(before),
        "shared stanza byte-identical: {text}"
    );
    let oob = load_root(root);
    assert_eq!(
        oob.lookup(&PathBuf::from("a.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC-BY-4.0".to_string()]
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("a.png"))
            .unwrap()
            .fallback_copyrights,
        vec!["2026 Acme".to_string(), "2027 Acme".to_string()]
    );
    // The sibling still rides the broad annotation untouched.
    assert_eq!(
        oob.lookup(&PathBuf::from("b.png"))
            .unwrap()
            .fallback_licenses,
        vec!["MIT".to_string()]
    );
}

#[test]
fn matching_license_missing_copyright_appends() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("REUSE.toml"),
        "version = 1\n\n[[annotations]]\npath = \"a.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    // Same license but a new requested notice is a real change, not Unchanged.
    assert_eq!(
        write_annotation(
            root,
            "a.png",
            "MIT",
            &["2026 Acme".to_string()],
            &load_root(root),
        )
        .unwrap(),
        AnnotationWrite::Appended
    );
    let oob = load_root(root);
    assert_eq!(
        oob.lookup(&PathBuf::from("a.png"))
            .unwrap()
            .fallback_copyrights,
        vec!["2026 Acme".to_string()]
    );
}

#[test]
fn unknown_keys_comments_and_literal_path_survive_append() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = "# Hand-authored header comment.\nversion = 1\ncustom = 42\n\n\
             [[annotations]]\npath = 'a.png'\n# inline comment\nunknown-key = true\n\
             SPDX-License-Identifier = \"MIT\"\n";
    std::fs::write(root.join("REUSE.toml"), before).unwrap();
    assert_eq!(
        write_annotation(root, "b.png", "CC0-1.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let text = std::fs::read_to_string(root.join("REUSE.toml")).unwrap();
    assert!(
        text.starts_with(before),
        "hand-authored content byte-identical: {text}"
    );
}

#[test]
fn subdir_closest_routes_to_subdir_doc_with_base_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(
        root.join("sub/REUSE.toml"),
        "version = 1\n\n[[annotations]]\npath = \"*.png\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    // The subdir document wins the `closest` fallback for its tree, so the
    // exact exception lands there with a base-relative path — a root
    // stanza would lose to the nearer document.
    let dest = annotation_destination(&load_root(root), "sub/logo.png");
    assert_eq!(dest.doc_rel, PathBuf::from("sub/REUSE.toml"));
    assert_eq!(dest.base_path, "logo.png");
    assert!(!dest.use_override);
    assert_eq!(
        write_annotation(root, "sub/logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    assert!(
        !root.join("REUSE.toml").exists(),
        "nothing appended at the root"
    );
    let text = std::fs::read_to_string(root.join("sub/REUSE.toml")).unwrap();
    assert!(text.contains("path = \"logo.png\""), "{text}");
    let oob = load_root(root);
    assert_eq!(
        oob.lookup(&PathBuf::from("sub/logo.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC-BY-4.0".to_string()]
    );
}

#[test]
fn subdir_override_barrier_gets_root_override_stanza() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(
        root.join("sub/REUSE.toml"),
        "version = 1\n\n[[annotations]]\npath = \"logo.png\"\nprecedence = \"override\"\n\
             SPDX-License-Identifier = \"MIT\"\n",
    )
    .unwrap();
    // A nearer `override` barrier can only be superseded from the root:
    // root is consulted first, so a root override stanza breaks before the
    // subdir document is ever read.
    let dest = annotation_destination(&load_root(root), "sub/logo.png");
    assert_eq!(dest.doc_rel, PathBuf::from("REUSE.toml"));
    assert!(dest.use_override);
    assert_eq!(
        write_annotation(root, "sub/logo.png", "CC-BY-4.0", &[], &load_root(root)).unwrap(),
        AnnotationWrite::Appended
    );
    let oob = load_root(root);
    let entry = oob.lookup(&PathBuf::from("sub/logo.png")).unwrap();
    assert_eq!(entry.license().as_deref(), Some("CC-BY-4.0"));
    assert!(entry.suppresses_file);
}

#[test]
fn lookup_is_last_match() {
    let oob = load_doc(
        "version = 1\n[[annotations]]\npath = \"*.png\"\nSPDX-License-Identifier = \"MIT\"\n\
             [[annotations]]\npath = \"logo.png\"\nSPDX-License-Identifier = \"CC-BY-4.0\"\n",
    );
    assert_eq!(
        oob.lookup(&PathBuf::from("logo.png"))
            .unwrap()
            .fallback_licenses,
        vec!["CC-BY-4.0".to_string()]
    );
}
// REUSE-IgnoreEnd
