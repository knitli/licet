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
//! For throughput (SC-006) only the file head is read and scanned (FR-037/§10).

use std::path::{Path, PathBuf};

use crate::domain::{
    ActualLicenseState, ActualSource, HeaderBlock, OobSource, OutOfBandEntry, PositionAfter,
    Precedence,
};
use crate::reuse::oob::OutOfBand;

/// Bytes of the file head scanned for headers. Headers live at the very top, so a few KB
/// is ample and keeps IO minimal.
const HEAD_BYTES: usize = 8 * 1024;

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

/// The `.license` sidecar path for an asset (`foo.png` → `foo.png.license`).
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".license");
    PathBuf::from(p)
}

/// Read up to [`HEAD_BYTES`] from a file.
pub fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Read the `<abs_path>.license` sidecar head, if a sidecar exists.
pub fn read_sidecar(abs_path: &Path) -> Option<Vec<u8>> {
    read_head(&sidecar_path(abs_path)).ok()
}

/// Detect the actual license state for `rel_path`, given the file head, an optional
/// `.license` sidecar, and out-of-band data.
pub fn detect(
    rel_path: &Path,
    head: &[u8],
    sidecar: Option<&[u8]>,
    oob: &OutOfBand,
) -> ActualLicenseState {
    let out_of_band = oob.lookup(rel_path);

    // A `.license` sidecar supplies the file-level headers and renders the asset's own
    // bytes irrelevant (so a binary asset with a sidecar is fully readable). A malformed
    // (non-UTF8) sidecar is ignored in favor of the asset head.
    let (parsed, header_source, encoding_ok) = match sidecar {
        Some(sc) => match std::str::from_utf8(sc) {
            Ok(t) => (parse_headers(t), ActualSource::Sidecar, true),
            Err(_) => parse_asset_head(head),
        },
        None => parse_asset_head(head),
    };
    let ParsedFile {
        blocks: headers,
        snippet_licenses,
    } = parsed;

    // Gather copyrights from all sources (copyright is never erased — FR-009).
    let mut copyrights: Vec<String> = headers.iter().flat_map(|h| h.copyrights.clone()).collect();
    if let Some(o) = &out_of_band {
        copyrights.extend(o.copyrights.clone());
    }

    let header_licenses: Vec<String> = headers.iter().flat_map(|h| h.license_ids.clone()).collect();
    let (detected_license, detected_source) =
        resolve_primary(&header_licenses, header_source, out_of_band.as_ref());

    // A binary/non-UTF8 asset that is covered out-of-band (a REUSE.toml annotation with a
    // license) is not "unreadable" — its licensing is known without reading its bytes
    // (FR-025). Only a non-UTF8 asset with no coverage at all stays unreadable.
    let covered_oob = out_of_band.as_ref().is_some_and(|o| o.license.is_some());
    let encoding_ok = encoding_ok || covered_oob;

    ActualLicenseState {
        headers,
        out_of_band,
        detected_license,
        detected_source,
        detected_copyrights: copyrights,
        snippet_licenses,
        encoding_ok,
    }
}

/// Decode and parse the asset head into headers, reporting the source as `Header` and
/// whether the bytes were valid UTF-8. A clean head, or one whose only error is a
/// truncated trailing multibyte sequence (read-boundary cut), is treated as readable.
fn parse_asset_head(head: &[u8]) -> (ParsedFile, ActualSource, bool) {
    let text = match std::str::from_utf8(head) {
        Ok(t) => t.to_string(),
        Err(e) if head.len() == HEAD_BYTES && e.error_len().is_none() => {
            std::str::from_utf8(&head[..e.valid_up_to()])
                .unwrap_or("")
                .to_string()
        }
        Err(_) => return (ParsedFile::default(), ActualSource::Header, false),
    };
    (parse_headers(&text), ActualSource::Header, true)
}

/// Resolve `OobSource` → `ActualSource`.
fn oob_actual_source(o: &OutOfBandEntry) -> ActualSource {
    match o.source {
        OobSource::ReuseToml => ActualSource::ReuseToml,
        OobSource::Dep5 => ActualSource::Dep5,
    }
}

/// Pick the primary detected license + its source, honoring the annotation's precedence
/// (FR-003a). `closest`/`aggregate` let file-level info win as the primary; `override`
/// lets the annotation win. Mirrors [`candidate_licenses`].
fn resolve_primary(
    header_licenses: &[String],
    header_source: ActualSource,
    oob: Option<&OutOfBandEntry>,
) -> (Option<String>, Option<ActualSource>) {
    let header_first = header_licenses.first().cloned();
    let file_level = || match &header_first {
        Some(h) => (Some(h.clone()), Some(header_source)),
        None => (None, None),
    };
    match oob {
        None => file_level(),
        Some(o) => match o.precedence {
            Precedence::Override => match o.license.clone() {
                Some(l) => (Some(l), Some(oob_actual_source(o))),
                None => file_level(),
            },
            Precedence::Closest | Precedence::Aggregate => {
                if header_first.is_some() {
                    file_level()
                } else {
                    match o.license.clone() {
                        Some(l) => (Some(l), Some(oob_actual_source(o))),
                        None => (None, None),
                    }
                }
            }
        },
    }
}

/// Every SPDX license expression that may satisfy intent for a file, honoring the
/// out-of-band annotation's precedence (FR-003a). Both `classify` and `reconcile` gate
/// compliance on this, so precedence flows everywhere from one place.
pub fn candidate_licenses(state: &ActualLicenseState) -> Vec<String> {
    let header_licenses: Vec<String> = state
        .headers
        .iter()
        .flat_map(|h| h.license_ids.clone())
        .collect();
    let oob_lic = state.out_of_band.as_ref().and_then(|o| o.license.clone());
    match state.out_of_band.as_ref().map(|o| o.precedence) {
        None => header_licenses,
        Some(Precedence::Override) => match oob_lic {
            Some(l) => vec![l],
            None => header_licenses,
        },
        Some(Precedence::Closest) => {
            if header_licenses.is_empty() {
                oob_lic.into_iter().collect()
            } else {
                header_licenses
            }
        }
        Some(Precedence::Aggregate) => {
            let mut v = header_licenses;
            if let Some(l) = oob_lic
                && !v.contains(&l)
            {
                v.push(l);
            }
            v
        }
    }
}

/// In-file licensing parsed from text: file-level header blocks plus the licenses of any
/// SPDX snippets (collected for text inventory, never treated as the file's own license).
#[derive(Default)]
struct ParsedFile {
    blocks: Vec<HeaderBlock>,
    snippet_licenses: Vec<String>,
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
    let mut current: Option<HeaderBlock> = None;
    let mut in_ignore = false;
    let mut in_snippet = false;

    for (start, raw) in split_keep_offsets(text) {
        let end = start + raw.len();

        // Ignore blocks swallow everything — including snippet markers — until they close.
        if in_ignore {
            if raw.contains(IGNORE_END) {
                in_ignore = false;
            }
            flush_block(&mut current, &mut blocks);
            continue;
        }
        if raw.contains(IGNORE_START) {
            in_ignore = true;
            flush_block(&mut current, &mut blocks);
            continue;
        }
        // Snippet boundaries break any file-level block in progress.
        if raw.contains(SNIPPET_END) {
            in_snippet = false;
            flush_block(&mut current, &mut blocks);
            continue;
        }
        if raw.contains(SNIPPET_BEGIN) {
            in_snippet = true;
            flush_block(&mut current, &mut blocks);
            continue;
        }

        let content = strip_comment(raw);
        let lic = extract_tag(content, LICENSE_TAG);

        if in_snippet {
            // Snippet licensing is gathered for inventory only — never file-level.
            if let Some(l) = lic {
                snippet_licenses.push(l.trim().to_string());
            }
            continue;
        }

        let cpr = extract_tag(content, COPYRIGHT_TAG);
        if lic.is_some() || cpr.is_some() {
            let block = current.get_or_insert_with(|| HeaderBlock {
                byte_range: (start, end),
                license_ids: Vec::new(),
                copyrights: Vec::new(),
                position_after: if blocks.is_empty() {
                    position_after
                } else {
                    PositionAfter::FileStart
                },
            });
            block.byte_range.1 = end;
            if let Some(l) = lic {
                block.license_ids.push(l.trim().to_string());
            }
            if let Some(c) = cpr {
                block.copyrights.push(c.trim().to_string());
            }
        } else {
            flush_block(&mut current, &mut blocks);
        }
    }
    flush_block(&mut current, &mut blocks);
    ParsedFile {
        blocks,
        snippet_licenses,
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

/// Strip a leading comment marker (line prefix or block delimiters) from a line so the
/// SPDX tag is detectable regardless of comment syntax.
fn strip_comment(line: &str) -> &str {
    let t = line.trim_start_matches(['\u{feff}']);
    let t = t.trim_start();
    for marker in ["//", "#", ";", "--", "/*", "<!--", "*", "%", "\""] {
        if let Some(rest) = t.strip_prefix(marker) {
            return rest.trim_start();
        }
    }
    t
}

/// Extract the value following a tag on a line, trimming any trailing block terminators.
fn extract_tag<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let idx = content.find(tag)?;
    let mut val = content[idx + tag.len()..].trim();
    for term in ["-->", "*/"] {
        if let Some(stripped) = val.strip_suffix(term) {
            val = stripped.trim();
        }
    }
    Some(val)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        OutOfBandEntry {
            source: OobSource::ReuseToml,
            license: Some(license.to_string()),
            copyrights: vec![],
            precedence,
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
            copyrights: vec![],
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
}
