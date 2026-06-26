//! Actual-license detection: parse in-file SPDX header blocks, read `<file>.license`
//! sidecars, and read out-of-band `REUSE.toml`/`.reuse/dep5`. Any of these can satisfy
//! intent; how a `REUSE.toml` annotation combines with file-level info is governed by its
//! `precedence` field (`closest` default), per REUSE 3.3 (FR-003a, FR-025, data-model §5).
//!
//! A `.license` sidecar carries file-level info that the REUSE spec treats as "inside the
//! file" — it takes precedence over an in-file header, and lets a binary asset be covered
//! without ever byte-editing it.
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
    let (headers, header_source, encoding_ok) = match sidecar {
        Some(sc) => match std::str::from_utf8(sc) {
            Ok(t) => (parse_headers(t), ActualSource::Sidecar, true),
            Err(_) => parse_asset_head(head),
        },
        None => parse_asset_head(head),
    };

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
        encoding_ok,
    }
}

/// Decode and parse the asset head into headers, reporting the source as `Header` and
/// whether the bytes were valid UTF-8. A clean head, or one whose only error is a
/// truncated trailing multibyte sequence (read-boundary cut), is treated as readable.
fn parse_asset_head(head: &[u8]) -> (Vec<HeaderBlock>, ActualSource, bool) {
    let text = match std::str::from_utf8(head) {
        Ok(t) => t.to_string(),
        Err(e) if head.len() == HEAD_BYTES && e.error_len().is_none() => {
            std::str::from_utf8(&head[..e.valid_up_to()])
                .unwrap_or("")
                .to_string()
        }
        Err(_) => return (Vec::new(), ActualSource::Header, false),
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

/// Parse contiguous SPDX header blocks from text, tracking byte ranges and first-line
/// context (FR-008, FR-019).
fn parse_headers(text: &str) -> Vec<HeaderBlock> {
    let position_after = leading_position(text);
    let mut blocks: Vec<HeaderBlock> = Vec::new();
    let mut current: Option<HeaderBlock> = None;
    let mut offset = 0usize;

    for line in split_keep_offsets(text) {
        let (start, raw) = line;
        let end = start + raw.len();
        let content = strip_comment(raw);
        let lic = extract_tag(content, LICENSE_TAG);
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
        } else if let Some(block) = current.take() {
            blocks.push(block);
        }
        offset = end;
    }
    let _ = offset;
    if let Some(block) = current.take() {
        blocks.push(block);
    }
    blocks
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
        );
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].license_ids, vec!["MIT".to_string()]);
        assert_eq!(h[0].copyrights, vec!["2026 Acme".to_string()]);
    }

    #[test]
    fn parses_two_blocks() {
        let h = parse_headers(
            "// SPDX-License-Identifier: MIT\n\ncode\n\n// SPDX-License-Identifier: Apache-2.0\n",
        );
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn shebang_position() {
        let h = parse_headers("#!/bin/sh\n# SPDX-License-Identifier: MIT\n");
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
        let h = parse_headers("<!-- SPDX-License-Identifier: CC0-1.0 -->\n");
        assert_eq!(h[0].license_ids, vec!["CC0-1.0".to_string()]);
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
