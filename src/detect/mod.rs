//! Actual-license detection: parse in-file SPDX header blocks, read `<file>.license`
//! sidecars, and read out-of-band `REUSE.toml`/`.reuse/dep5`. Any of these can satisfy
//! intent; how a `REUSE.toml` annotation combines with file-level info is governed by its
//! `precedence` field (`closest` default), per REUSE 3.3 (FR-003a, FR-025, data-model §5).
//!
//! A `.license` sidecar carries file-level info that the REUSE spec treats as "inside the
//! file" — it takes precedence over an in-file header, and lets a binary asset be covered
//! without ever byte-editing it.
//!
//! REUSE ignore blocks (`REUSE-IgnoreStart`/`REUSE-IgnoreEnd`) and SPDX snippets
//! (`SPDX-SnippetBegin`/`SPDX-SnippetEnd`) are honored when parsing: ignored regions are
//! dropped, and snippet licenses are collected separately from the file's own (FR-030).
//!
//! Complete file bytes are scanned (no head cutoff — F12): a header anywhere counts,
//! invalid UTF-8 anywhere makes the file unreadable, and rejected license values are
//! kept with their line for diagnosis instead of being dropped silently.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aho_corasick::AhoCorasick;

use crate::domain::{
    ActualLicenseState, ActualSource, HeaderBlock, InvalidLicenseValue, OobSource, OutOfBandEntry,
    PositionAfter, Precedence,
};
use crate::reuse::oob::OutOfBand;
use crate::spdx;

const LICENSE_TAG: &str = "SPDX-License-Identifier:";
const COPYRIGHT_TAG: &str = "SPDX-FileCopyrightText:";

/// REUSE ignore-block markers: licensing information between them is *not* the file's own
/// (e.g. example/output text) and is skipped entirely (REUSE 3.3 §"Ignore block").
const IGNORE_START: &str = "REUSE-IgnoreStart";
const IGNORE_END: &str = "REUSE-IgnoreEnd";

/// SPDX snippet markers: licensing between them describes a *snippet*, not the file, so it
/// never affects the file's drift class (REUSE 3.3 §"In-line Snippet comments", SPDX
/// Annex H). Recognizing snippets requires the same region-skipping as ignore blocks.
const SNIPPET_BEGIN: &str = "SPDX-SnippetBegin";
const SNIPPET_END: &str = "SPDX-SnippetEnd";

/// All control markers scanned per line, in pattern-ID order. One Aho-Corasick automaton
/// finds every marker (and both tags) in a single pass, replacing six per-line substring
/// scans. Indices here are the `pattern()` IDs matched on in [`parse_headers`].
const MARKERS: [&str; 6] = [
    IGNORE_START,  // 0
    IGNORE_END,    // 1
    SNIPPET_BEGIN, // 2
    SNIPPET_END,   // 3
    LICENSE_TAG,   // 4
    COPYRIGHT_TAG, // 5
];

/// The shared automaton, built once and reused across every file/line (cheap to share
/// across the rayon-parallel scan since it is read-only after construction).
fn marker_automaton() -> &'static AhoCorasick {
    static AC: OnceLock<AhoCorasick> = OnceLock::new();
    AC.get_or_init(|| AhoCorasick::new(MARKERS).expect("static marker patterns are valid"))
}

/// The `.license` sidecar path for an asset (`foo.png` → `foo.png.license`).
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".license");
    PathBuf::from(p)
}

/// Detect the actual license state for `rel_path`, given the complete file bytes,
/// an optional `.license` sidecar, and out-of-band data.
///
/// The whole content is scanned (no head cutoff): a header anywhere counts, and
/// invalid UTF-8 anywhere (not just in the head) makes the file unreadable.
/// Rejected license values are kept with their line for diagnosis, never dropped.
pub fn detect(
    rel_path: &Path,
    full: &[u8],
    sidecar: Option<&[u8]>,
    oob: &OutOfBand,
) -> ActualLicenseState {
    let mut out_of_band = oob.lookup(rel_path);

    // A `.license` sidecar supplies the file-level headers and renders the asset's own
    // bytes irrelevant (so a binary asset with a sidecar is fully readable). A malformed
    // (non-UTF8) sidecar falls back to the asset bytes; task 4 diagnoses sidecar
    // encoding as an in-band error instead of this silent fallback.
    let (parsed, header_source, encoding_ok) = match sidecar {
        Some(sc) => match std::str::from_utf8(sc) {
            Ok(t) => (parse_headers(t), ActualSource::Sidecar, true),
            Err(_) => parse_asset_bytes(full),
        },
        None => parse_asset_bytes(full),
    };
    let ParsedFile {
        blocks: headers,
        snippet_licenses,
        snippet_copyrights,
        invalid_license_values,
    } = parsed;

    // An `override` barrier suppresses file/sidecar info entirely: the raw
    // notices stay in `headers` (for preservation and surgical edits) but are
    // excluded from every effective value (REUSE 3.3, reference behavior).
    let suppressed = out_of_band.as_ref().is_some_and(|o| o.suppresses_file);
    let file_licenses: Vec<String> = if suppressed {
        Vec::new()
    } else {
        headers.iter().flat_map(|h| h.license_ids.clone()).collect()
    };
    let file_copyrights: Vec<String> = if suppressed {
        Vec::new()
    } else {
        headers.iter().flat_map(|h| h.copyrights.clone()).collect()
    };

    // Effective values: file-level info plus unconditional (`aggregate` and
    // barrier) OOB contributions, plus the per-field `closest` fallback only
    // where the file carries nothing for that field (reference `reuse_info_of`:
    // a file with exactly one field still takes the other's fallback).
    let mut eff_licenses = file_licenses.clone();
    let mut eff_copyrights = file_copyrights.clone();
    if let Some(o) = &out_of_band {
        push_unique(&mut eff_licenses, &o.licenses);
        push_unique(&mut eff_copyrights, &o.copyrights);
        if file_licenses.is_empty() {
            push_unique(&mut eff_licenses, &o.fallback_licenses);
        }
        if file_copyrights.is_empty() {
            push_unique(&mut eff_copyrights, &o.fallback_copyrights);
        }
    }
    let (detected_license, detected_source) =
        resolve_primary(&file_licenses, header_source, out_of_band.as_ref());

    // Retain only provenance that actually contributed: unconditional tables
    // always, fallback tables only for a field the file left empty.
    if let Some(o) = out_of_band.as_mut() {
        let used_lic = file_licenses.is_empty() && !o.fallback_licenses.is_empty();
        let used_cpr = file_copyrights.is_empty() && !o.fallback_copyrights.is_empty();
        let (fb_lic, fb_cpr) = (o.fallback_licenses.clone(), o.fallback_copyrights.clone());
        o.origins.retain(|origin| {
            if origin.precedence != Precedence::Closest {
                return true;
            }
            (used_lic && !fb_lic.is_empty() && origin.licenses == fb_lic)
                || (used_cpr && !fb_cpr.is_empty() && origin.copyrights == fb_cpr)
        });
    }

    // A binary/non-UTF8 asset that is covered out-of-band (metadata with a
    // license) is not "unreadable" — its licensing is known without reading
    // its bytes (FR-025). Only a non-UTF8 asset with no coverage at all stays
    // unreadable.
    let covered_oob = out_of_band
        .as_ref()
        .is_some_and(|o| !o.licenses.is_empty() || !o.fallback_licenses.is_empty());
    let encoding_ok = encoding_ok || covered_oob;

    ActualLicenseState {
        headers,
        out_of_band,
        detected_license,
        detected_source,
        detected_copyrights: eff_copyrights,
        snippet_licenses,
        snippet_copyrights,
        encoding_ok,
        invalid_license_values,
    }
}

/// Append entries of `src` that `target` does not already hold.
fn push_unique(target: &mut Vec<String>, src: &[String]) {
    for item in src {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

/// Decode and parse complete asset bytes into headers, reporting the source as
/// `Header` and whether the bytes were valid UTF-8 anywhere in the file.
fn parse_asset_bytes(full: &[u8]) -> (ParsedFile, ActualSource, bool) {
    let text = match std::str::from_utf8(full) {
        Ok(t) => t,
        Err(_) => return (ParsedFile::default(), ActualSource::Header, false),
    };
    (parse_headers(text), ActualSource::Header, true)
}

/// Resolve `OobSource` → `ActualSource`.
fn oob_actual_source(o: &OutOfBandEntry) -> ActualSource {
    match o.source {
        OobSource::ReuseToml => ActualSource::ReuseToml,
        OobSource::Dep5 => ActualSource::Dep5,
    }
}

/// Pick the primary detected license + its source. `header_licenses` is
/// already suppression-aware (empty under an `override` barrier), so the
/// first file-level license wins when present and OOB licenses — unconditional
/// first, then the `closest` fallback — only fill a gap. Mirrors
/// [`candidate_licenses`].
fn resolve_primary(
    header_licenses: &[String],
    header_source: ActualSource,
    oob: Option<&OutOfBandEntry>,
) -> (Option<String>, Option<ActualSource>) {
    if let Some(first) = header_licenses.first() {
        return (Some(first.clone()), Some(header_source));
    }
    match oob {
        None => (None, None),
        Some(o) => match o.licenses.first().or(o.fallback_licenses.first()) {
            Some(l) => (Some(l.clone()), Some(oob_actual_source(o))),
            None => (None, None),
        },
    }
}

/// Every SPDX license expression that may satisfy intent for a file: effective
/// file-level licenses plus unconditional OOB contributions, plus the
/// `closest` fallback only when the file carries no license of its own
/// (FR-003a). Both `classify` and `reconcile` gate compliance on this, so
/// precedence flows everywhere from one place.
pub fn candidate_licenses(state: &ActualLicenseState) -> Vec<String> {
    let suppressed = state
        .out_of_band
        .as_ref()
        .is_some_and(|o| o.suppresses_file);
    let mut v: Vec<String> = if suppressed {
        Vec::new()
    } else {
        state
            .headers
            .iter()
            .flat_map(|h| h.license_ids.clone())
            .collect()
    };
    // The fallback fills a file-level gap even when unconditional OOB values
    // exist (the reference tool reports both in that case).
    let file_empty = v.is_empty();
    if let Some(o) = state.out_of_band.as_ref() {
        push_unique(&mut v, &o.licenses);
        if file_empty {
            push_unique(&mut v, &o.fallback_licenses);
        }
    }
    v
}

/// In-file licensing parsed from text: file-level header blocks plus the licenses of any
/// SPDX snippets (collected for text inventory, never treated as the file's own license),
/// plus every rejected license value with its line for diagnosis.
#[derive(Default)]
struct ParsedFile {
    blocks: Vec<HeaderBlock>,
    snippet_licenses: Vec<String>,
    snippet_copyrights: Vec<String>,
    invalid_license_values: Vec<InvalidLicenseValue>,
}

/// Push and clear the in-progress file-level header block, if any.
fn flush_block(current: &mut Option<HeaderBlock>, blocks: &mut Vec<HeaderBlock>) {
    if let Some(b) = current.take() {
        blocks.push(b);
    }
}

/// Parse contiguous SPDX header blocks from text, tracking byte ranges and first-line
/// context (FR-008, FR-019), honoring REUSE ignore blocks and SPDX snippets (FR-030).
///
/// `REUSE-IgnoreStart`..`REUSE-IgnoreEnd` regions are dropped wholesale (an unclosed
/// `IgnoreStart` suppresses to end of input). `SPDX-SnippetBegin`..`SPDX-SnippetEnd`
/// regions describe a snippet, not the file: their `SPDX-License-Identifier`s are gathered
/// into `snippet_licenses` (so their texts still count for `LICENSES/`) but never become
/// file-level header blocks. Ignore takes precedence over snippet markers.
fn parse_headers(text: &str) -> ParsedFile {
    let position_after = leading_position(text);
    let mut blocks: Vec<HeaderBlock> = Vec::new();
    let mut snippet_licenses: Vec<String> = Vec::new();
    let mut snippet_copyrights: Vec<String> = Vec::new();
    let mut invalid_license_values: Vec<InvalidLicenseValue> = Vec::new();
    let mut current: Option<HeaderBlock> = None;
    let mut in_ignore = false;
    let mut in_snippet = false;

    for (line_no, (start, raw)) in split_keep_offsets(text).into_iter().enumerate() {
        let line = line_no + 1;
        let end = start + raw.len();

        // Single pass over the line: which control markers are present, and where each
        // tag's value begins. Detection is comment-syntax-agnostic — the automaton finds
        // the tag anywhere on the line, so no leading-marker stripping is needed (mirrors
        // the reference REUSE tool).
        let (mut ig_start, mut ig_end, mut sn_begin, mut sn_end) = (false, false, false, false);
        let mut lic_at: Option<usize> = None;
        let mut cpr_at: Option<usize> = None;
        for m in marker_automaton().find_iter(raw) {
            match m.pattern().as_usize() {
                0 => ig_start = true,
                1 => ig_end = true,
                2 => sn_begin = true,
                3 => sn_end = true,
                4 if lic_at.is_none() => lic_at = Some(m.end()),
                5 if cpr_at.is_none() => cpr_at = Some(m.end()),
                _ => {}
            }
        }

        // Ignore blocks swallow everything — including snippet markers — until they close.
        if in_ignore {
            if ig_end {
                in_ignore = false;
            }
            flush_block(&mut current, &mut blocks);
            continue;
        }
        if ig_start {
            in_ignore = true;
            flush_block(&mut current, &mut blocks);
            continue;
        }
        // Snippet boundaries break any file-level block in progress.
        if sn_end {
            in_snippet = false;
            flush_block(&mut current, &mut blocks);
            continue;
        }
        if sn_begin {
            in_snippet = true;
            flush_block(&mut current, &mut blocks);
            continue;
        }

        // Accept the value only when it is a well-formed SPDX expression. The tag is matched
        // anywhere on the line (comment-syntax-agnostic), so without this guard any prose or
        // code that merely follows the marker — e.g. the tag appearing inside a source
        // string literal — would be captured verbatim as the file's license (FR-005).
        // Rejected values are diagnosed with line + value, never dropped silently.
        // The trimmed value stays a subslice of the line, so its exact byte span
        // travels with it for span-based reconciliation (FR-007).
        let raw_lic = lic_at.map(|i| {
            let seg = &raw[i..];
            let val = trim_value(seg).trim();
            let off = val.as_ptr() as usize - seg.as_ptr() as usize;
            (
                val.to_string(),
                (start + i + off, start + i + off + val.len()),
            )
        });
        let lic = match raw_lic {
            Some((value, span)) => match spdx::validate_expression(&value) {
                Ok(()) => Some((value, span)),
                Err(reason) => {
                    invalid_license_values.push(InvalidLicenseValue {
                        line,
                        value,
                        reason,
                    });
                    None
                }
            },
            None => None,
        };

        if in_snippet {
            // Snippet notices are gathered for inventory and REUSE validation —
            // never file-level policy (decision 5). The reference tool flattens
            // snippet notices into the file's info, so lint counts them too.
            if let Some((l, _)) = lic {
                snippet_licenses.push(l);
            }
            if let Some(i) = cpr_at {
                let val = trim_value(&raw[i..]).trim();
                if !val.is_empty() {
                    snippet_copyrights.push(val.to_string());
                }
            }
            continue;
        }

        let cpr = cpr_at.map(|i| {
            let seg = &raw[i..];
            let val = trim_value(seg).trim();
            let off = val.as_ptr() as usize - seg.as_ptr() as usize;
            (
                val.to_string(),
                (start + i + off, start + i + off + val.len()),
            )
        });
        if lic.is_some() || cpr.is_some() {
            let block = current.get_or_insert_with(|| HeaderBlock {
                byte_range: (start, end),
                license_ids: Vec::new(),
                license_spans: Vec::new(),
                copyrights: Vec::new(),
                copyright_spans: Vec::new(),
                position_after: if blocks.is_empty() {
                    position_after
                } else {
                    PositionAfter::FileStart
                },
            });
            block.byte_range.1 = end;
            if let Some((l, span)) = lic {
                block.license_ids.push(l);
                block.license_spans.push(span);
            }
            if let Some((c, span)) = cpr {
                block.copyrights.push(c);
                block.copyright_spans.push(span);
            }
        } else {
            flush_block(&mut current, &mut blocks);
        }
    }
    flush_block(&mut current, &mut blocks);
    ParsedFile {
        blocks,
        snippet_licenses,
        snippet_copyrights,
        invalid_license_values,
    }
}

/// Determine the first-line context for safe insertion (FR-019).
fn leading_position(text: &str) -> PositionAfter {
    if text.starts_with('\u{feff}') {
        return PositionAfter::Bom;
    }
    let first = text.lines().next().unwrap_or("");
    if first.starts_with("#!") {
        return PositionAfter::Shebang;
    }
    if first.starts_with("<?xml") || first.contains("coding:") || first.contains("coding=") {
        return PositionAfter::EncodingDecl;
    }
    if first.starts_with("<?php") {
        return PositionAfter::PhpTag;
    }
    if first.starts_with("cabal-version:") {
        return PositionAfter::HaskellCabal;
    }
    if first.starts_with("% !BIB") || first.starts_with("%!BIB") {
        return PositionAfter::BibTex;
    }
    if first.starts_with("% !TEX") || first.starts_with("%TEX") {
        return PositionAfter::Tex;
    }
    PositionAfter::FileStart
}

/// Split text into `(byte_offset, line_without_terminator)` pairs.
fn split_keep_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let mut line_end = i;
            if line_end > start && bytes[line_end - 1] == b'\r' {
                line_end -= 1;
            }
            out.push((start, &text[start..line_end]));
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push((start, &text[start..]));
    }
    out
}

/// Trim a tag value (the slice just past the tag): surrounding whitespace plus any trailing
/// block-comment closer. The closer set spans every block style in the comment registry so
/// detection stays comment-syntax-agnostic; only one closer can end a line, so we stop after
/// the first match. Ordered longest-first to avoid a shorter closer masking a longer one.
fn trim_value(rest: &str) -> &str {
    const CLOSERS: [&str; 12] = [
        "--}}", "--%>", "--#>", "-->", "*/", "*)", "*#", "'/", "-/", ":)", "#}", "}",
    ];
    let mut val = rest.trim();
    for term in CLOSERS {
        if let Some(stripped) = val.strip_suffix(term) {
            val = stripped.trim();
            break;
        }
    }
    val
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn license_spans_cover_exact_value_bytes() {
        // A tag with a code prefix still validates when the value itself is
        // clean; each value keeps its own exact span so reconciliation can
        // locate it — and refuse it — without reparsing the line. (A tag
        // inside a string literal carries trailing quote junk and is rejected
        // by expression validation instead.)
        let text = "/* SPDX-License-Identifier: MIT */\nFOO=SPDX-License-Identifier: Apache-2.0\n";
        let h = parse_headers(text).blocks;
        // Adjacent tag lines merge.
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].license_ids, vec!["MIT", "Apache-2.0"]);
        assert_eq!(h[0].license_spans.len(), 2);
        for (span, expect) in h[0].license_spans.iter().zip(["MIT", "Apache-2.0"]) {
            assert_eq!(&text[span.0..span.1], expect);
        }
    }

    #[test]
    fn parses_single_header() {
        let h = parse_headers(
            "// SPDX-License-Identifier: MIT\n// SPDX-FileCopyrightText: 2026 Acme\n\ncode\n",
        )
        .blocks;
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].license_ids, vec!["MIT".to_string()]);
        assert_eq!(h[0].copyrights, vec!["2026 Acme".to_string()]);
    }

    #[test]
    fn parses_two_blocks() {
        let h = parse_headers(
            "// SPDX-License-Identifier: MIT\n\ncode\n\n// SPDX-License-Identifier: Apache-2.0\n",
        )
        .blocks;
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn shebang_position() {
        let h = parse_headers("#!/bin/sh\n# SPDX-License-Identifier: MIT\n").blocks;
        assert_eq!(h[0].position_after, PositionAfter::Shebang);
    }

    #[test]
    fn php_tag_position() {
        let h = parse_headers("<?php\n// SPDX-License-Identifier: MIT\n").blocks;
        assert_eq!(h[0].position_after, PositionAfter::PhpTag);
    }

    #[test]
    fn haskell_cabal_position() {
        let h = parse_headers("cabal-version: 2.4\n-- SPDX-License-Identifier: MIT\n").blocks;
        assert_eq!(h[0].position_after, PositionAfter::HaskellCabal);
    }

    #[test]
    fn tex_position() {
        let h = parse_headers("% !TEX\n% SPDX-License-Identifier: MIT\n").blocks;
        assert_eq!(h[0].position_after, PositionAfter::Tex);
    }

    #[test]
    fn detects_non_utf8_as_unreadable() {
        let oob = OutOfBand::default();
        let bytes = [0xff, 0xfe, 0x00, 0x41]; // UTF-16-ish, invalid UTF-8 at byte 0
        let st = detect(&PathBuf::from("x.txt"), &bytes, None, &oob);
        assert!(!st.encoding_ok);
    }

    #[test]
    fn binary_with_sidecar_is_readable_and_detected() {
        let oob = OutOfBand::default();
        let bytes = [0xff, 0xfe, 0x00, 0x41]; // binary asset
        let sidecar = b"SPDX-License-Identifier: CC-BY-4.0\nSPDX-FileCopyrightText: 2026 Acme\n";
        let st = detect(&PathBuf::from("logo.png"), &bytes, Some(sidecar), &oob);
        assert!(st.encoding_ok, "sidecar makes a binary asset readable");
        assert_eq!(st.detected_license.as_deref(), Some("CC-BY-4.0"));
        assert_eq!(st.detected_source, Some(ActualSource::Sidecar));
        assert!(
            st.detected_copyrights
                .iter()
                .any(|c| c.contains("2026 Acme"))
        );
    }

    #[test]
    fn block_comment_header() {
        let h = parse_headers("<!-- SPDX-License-Identifier: CC0-1.0 -->\n").blocks;
        assert_eq!(h[0].license_ids, vec!["CC0-1.0".to_string()]);
    }

    #[test]
    fn non_spdx_tag_value_is_not_captured_as_license() {
        // The tag is matched anywhere on a line, so a tag appearing inside a source-string
        // literal (e.g. this project's own test fixtures) trails non-SPDX junk after the id.
        // Such a value must not be accepted as the file's license (would otherwise poison
        // `init`'s generated config — regression for the unescaped-TOML crash).
        let parsed =
            parse_headers("    f.write(\"a.py\", \"# SPDX-License-Identifier: MIT\\nx=1\\n\")\n");
        assert!(
            parsed.blocks.iter().all(|b| b.license_ids.is_empty()),
            "malformed SPDX expression must not be detected: {:?}",
            parsed.blocks
        );
        // A clean id on its own line is still detected.
        let ok = parse_headers("# SPDX-License-Identifier: MIT\n").blocks;
        assert_eq!(ok[0].license_ids, vec!["MIT".to_string()]);
    }

    #[test]
    fn block_closers_across_styles_are_trimmed() {
        // One representative per registry block style; the trailing closer must be stripped
        // so the bare SPDX id remains.
        let cases = [
            ("/* SPDX-License-Identifier: MIT */\n", "MIT"), // C/CSS, ML/AppleScript share *)
            ("(* SPDX-License-Identifier: MIT *)\n", "MIT"), // ML/OCaml
            ("{# SPDX-License-Identifier: MIT #}\n", "MIT"), // Jinja
            ("/- SPDX-License-Identifier: MIT -/\n", "MIT"), // Lean
            ("{{-- SPDX-License-Identifier: MIT --}}\n", "MIT"), // Blade/Handlebars
            ("<%-- SPDX-License-Identifier: MIT --%>\n", "MIT"), // ASPX
            ("<#-- SPDX-License-Identifier: MIT --#>\n", "MIT"), // FreeMarker
            ("/' SPDX-License-Identifier: MIT '/\n", "MIT"), // PlantUML
            ("(: SPDX-License-Identifier: MIT :)\n", "MIT"), // XQuery
            ("#* SPDX-License-Identifier: MIT *#\n", "MIT"), // Velocity
            ("{ SPDX-License-Identifier: MIT }\n", "MIT"),   // Pascal/BibTeX
        ];
        for (input, want) in cases {
            let h = parse_headers(input).blocks;
            assert_eq!(
                h[0].license_ids,
                vec![want.to_string()],
                "closer not trimmed for input {input:?}"
            );
        }
    }

    #[test]
    fn ignore_block_suppresses_tags() {
        // The real header is MIT; the bracketed GPL line is example output, not licensing.
        let p = parse_headers(
            "// SPDX-License-Identifier: MIT\n\
             // REUSE-IgnoreStart\n\
             // SPDX-License-Identifier: GPL-3.0-or-later\n\
             // REUSE-IgnoreEnd\n",
        );
        let licenses: Vec<_> = p
            .blocks
            .iter()
            .flat_map(|b| b.license_ids.clone())
            .collect();
        assert_eq!(licenses, vec!["MIT".to_string()]);
    }

    #[test]
    fn unclosed_ignore_suppresses_to_eof() {
        let p =
            parse_headers("// REUSE-IgnoreStart\n// SPDX-License-Identifier: MIT\n// more stuff\n");
        assert!(
            p.blocks.is_empty(),
            "everything after IgnoreStart is dropped"
        );
    }

    #[test]
    fn snippet_license_is_not_file_level() {
        // The file is MIT; the snippet is GPL. Drift must see only MIT, but the snippet
        // license is still collected for LICENSES/ inventory.
        let p = parse_headers(
            "// SPDX-License-Identifier: MIT\n\
             // SPDX-FileCopyrightText: 2026 Acme\n\n\
             code\n\n\
             // SPDX-SnippetBegin\n\
             // SPDX-SnippetCopyrightText: 2022 Jane Doe\n\
             // SPDX-License-Identifier: GPL-3.0-or-later\n\
             snippet()\n\
             // SPDX-SnippetEnd\n",
        );
        let file_licenses: Vec<_> = p
            .blocks
            .iter()
            .flat_map(|b| b.license_ids.clone())
            .collect();
        assert_eq!(file_licenses, vec!["MIT".to_string()]);
        assert_eq!(p.snippet_licenses, vec!["GPL-3.0-or-later".to_string()]);
        // Snippet copyright is not folded into the file's copyrights.
        let file_copyrights: Vec<_> = p.blocks.iter().flat_map(|b| b.copyrights.clone()).collect();
        assert_eq!(file_copyrights, vec!["2026 Acme".to_string()]);
    }

    #[test]
    fn ignore_outranks_snippet_markers() {
        // A snippet opened inside an ignore block is never recognized.
        let p = parse_headers(
            "// SPDX-License-Identifier: MIT\n\
             // REUSE-IgnoreStart\n\
             // SPDX-SnippetBegin\n\
             // SPDX-License-Identifier: GPL-3.0-or-later\n\
             // SPDX-SnippetEnd\n\
             // REUSE-IgnoreEnd\n",
        );
        let file_licenses: Vec<_> = p
            .blocks
            .iter()
            .flat_map(|b| b.license_ids.clone())
            .collect();
        assert_eq!(file_licenses, vec!["MIT".to_string()]);
        assert!(
            p.snippet_licenses.is_empty(),
            "snippet inside ignore is dropped"
        );
    }

    fn oob_with(license: &str, precedence: Precedence) -> OutOfBandEntry {
        // A `closest` entry carries its value as the fallback (used only when
        // the file has no license); other precedences contribute unconditionally,
        // and `override` additionally suppresses file-level info.
        let (licenses, fallback_licenses) = match precedence {
            Precedence::Closest => (Vec::new(), vec![license.to_string()]),
            _ => (vec![license.to_string()], Vec::new()),
        };
        OutOfBandEntry {
            source: OobSource::ReuseToml,
            licenses,
            copyrights: vec![],
            fallback_licenses,
            fallback_copyrights: vec![],
            suppresses_file: precedence == Precedence::Override,
            precedence,
            origins: vec![],
        }
    }

    fn state_with(headers: Vec<HeaderBlock>, oob: Option<OutOfBandEntry>) -> ActualLicenseState {
        ActualLicenseState {
            headers,
            out_of_band: oob,
            encoding_ok: true,
            ..Default::default()
        }
    }

    fn header(license: &str) -> HeaderBlock {
        HeaderBlock {
            byte_range: (0, 0),
            license_ids: vec![license.to_string()],
            license_spans: vec![(0, 0)],
            copyrights: vec![],
            copyright_spans: vec![],
            position_after: PositionAfter::FileStart,
        }
    }

    #[test]
    fn precedence_closest_lets_header_win() {
        // header=MIT, REUSE.toml=Apache, default (closest) → MIT is the candidate.
        let st = state_with(
            vec![header("MIT")],
            Some(oob_with("Apache-2.0", Precedence::Closest)),
        );
        assert_eq!(candidate_licenses(&st), vec!["MIT".to_string()]);
    }

    #[test]
    fn precedence_override_lets_oob_win() {
        let st = state_with(
            vec![header("MIT")],
            Some(oob_with("Apache-2.0", Precedence::Override)),
        );
        assert_eq!(candidate_licenses(&st), vec!["Apache-2.0".to_string()]);
    }

    #[test]
    fn precedence_aggregate_keeps_both() {
        let st = state_with(
            vec![header("MIT")],
            Some(oob_with("Apache-2.0", Precedence::Aggregate)),
        );
        let cands = candidate_licenses(&st);
        assert!(cands.contains(&"MIT".to_string()));
        assert!(cands.contains(&"Apache-2.0".to_string()));
    }

    #[test]
    fn closest_falls_back_to_oob_when_no_header() {
        let st = state_with(vec![], Some(oob_with("CC0-1.0", Precedence::Closest)));
        assert_eq!(candidate_licenses(&st), vec!["CC0-1.0".to_string()]);
    }

    #[test]
    fn multiple_blocks_reported_individually() {
        // Two header blocks: both kept with their own ids; the first wins primary.
        let text =
            "// SPDX-License-Identifier: MIT\n\ncode\n\n// SPDX-License-Identifier: Apache-2.0\n";
        let oob = OutOfBand::default();
        let st = detect(&PathBuf::from("a.rs"), text.as_bytes(), None, &oob);
        assert_eq!(st.headers.len(), 2);
        assert_eq!(st.headers[0].license_ids, vec!["MIT".to_string()]);
        assert_eq!(st.headers[1].license_ids, vec!["Apache-2.0".to_string()]);
        assert_eq!(st.detected_license.as_deref(), Some("MIT"));
        assert!(st.invalid_license_values.is_empty());
    }

    #[test]
    fn crlf_headers_keep_correct_byte_ranges() {
        // CRLF line endings: ranges must slice the exact source lines.
        let text = "// SPDX-License-Identifier: MIT\r\n// SPDX-FileCopyrightText: 2026 Acme\r\n\r\ncode\r\n";
        let p = parse_headers(text);
        assert_eq!(p.blocks.len(), 1);
        let (start, end) = p.blocks[0].byte_range;
        assert_eq!(
            &text[start..end],
            "// SPDX-License-Identifier: MIT\r\n// SPDX-FileCopyrightText: 2026 Acme"
        );
        assert_eq!(p.blocks[0].license_ids, vec!["MIT".to_string()]);
        assert_eq!(p.blocks[0].copyrights, vec!["2026 Acme".to_string()]);
    }

    #[test]
    fn invalid_values_carry_line_and_reason() {
        let text = "// SPDX-License-Identifier: MIT\n// SPDX-License-Identifier: Bogus-1.0\ncode\n// SPDX-License-Identifier: Also Bad\n";
        let p = parse_headers(text);
        assert_eq!(p.blocks.len(), 1);
        assert_eq!(p.blocks[0].license_ids, vec!["MIT".to_string()]);
        assert_eq!(p.invalid_license_values.len(), 2);
        assert_eq!(p.invalid_license_values[0].line, 2);
        assert_eq!(p.invalid_license_values[0].value, "Bogus-1.0");
        assert!(!p.invalid_license_values[0].reason.is_empty());
        assert_eq!(p.invalid_license_values[1].line, 4);
    }

    #[test]
    fn copyright_only_block_has_no_ids_but_keeps_copyright() {
        let text = "// SPDX-FileCopyrightText: 2026 Acme\n";
        let oob = OutOfBand::default();
        let st = detect(&PathBuf::from("a.rs"), text.as_bytes(), None, &oob);
        assert_eq!(st.detected_license, None);
        assert!(
            st.detected_copyrights
                .iter()
                .any(|c| c.contains("2026 Acme"))
        );
        assert!(
            st.invalid_license_values.is_empty(),
            "no license tag, no diagnosis"
        );
    }
}
