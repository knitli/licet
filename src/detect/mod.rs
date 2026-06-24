//! Actual-license detection: parse in-file SPDX header blocks and read out-of-band
//! `REUSE.toml`/`.reuse/dep5`; either source satisfies intent, with out-of-band
//! authoritative on disagreement (FR-003a, FR-025, data-model §5).
//!
//! For throughput (SC-006) only the file head is read and scanned (FR-037/§10).

use std::path::Path;

use crate::domain::{ActualLicenseState, ActualSource, HeaderBlock, OobSource, PositionAfter};
use crate::reuse::oob::OutOfBand;

/// Bytes of the file head scanned for headers. Headers live at the very top, so a few KB
/// is ample and keeps IO minimal.
const HEAD_BYTES: usize = 8 * 1024;

const LICENSE_TAG: &str = "SPDX-License-Identifier:";
const COPYRIGHT_TAG: &str = "SPDX-FileCopyrightText:";

/// Read up to [`HEAD_BYTES`] from a file.
pub fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

/// Detect the actual license state for `rel_path`, given the file head and out-of-band data.
pub fn detect(rel_path: &Path, head: &[u8], oob: &OutOfBand) -> ActualLicenseState {
    // UTF-8 validity: a clean head, or a head whose only error is a truncated trailing
    // multibyte sequence (boundary cut), is treated as readable.
    let text = match std::str::from_utf8(head) {
        Ok(t) => t.to_string(),
        Err(e) if head.len() == HEAD_BYTES && e.error_len().is_none() => {
            // Truncated at the read boundary — decode the valid prefix.
            std::str::from_utf8(&head[..e.valid_up_to()])
                .unwrap_or("")
                .to_string()
        }
        Err(_) => {
            return ActualLicenseState {
                encoding_ok: false,
                ..Default::default()
            };
        }
    };

    let headers = parse_headers(&text);
    let out_of_band = oob.lookup(rel_path);

    // Gather copyrights from all sources.
    let mut copyrights: Vec<String> = headers.iter().flat_map(|h| h.copyrights.clone()).collect();
    if let Some(o) = &out_of_band {
        copyrights.extend(o.copyrights.clone());
    }

    // Determine the detected license and its source, with out-of-band authoritative on
    // disagreement (FR-003a).
    let header_license = headers.iter().flat_map(|h| h.license_ids.clone()).next();
    let (detected_license, detected_source) = match (&out_of_band, &header_license) {
        (Some(o), _) if o.license.is_some() => {
            let src = match o.source {
                OobSource::ReuseToml => ActualSource::ReuseToml,
                OobSource::Dep5 => ActualSource::Dep5,
            };
            (o.license.clone(), Some(src))
        }
        (_, Some(h)) => (Some(h.clone()), Some(ActualSource::Header)),
        _ => (None, None),
    };

    ActualLicenseState {
        headers,
        out_of_band,
        detected_license,
        detected_source,
        detected_copyrights: copyrights,
        encoding_ok: true,
    }
}

/// Every distinct SPDX license expression detected across header blocks and out-of-band.
pub fn candidate_licenses(state: &ActualLicenseState) -> Vec<String> {
    let mut out: Vec<String> = state
        .headers
        .iter()
        .flat_map(|h| h.license_ids.clone())
        .collect();
    if let Some(o) = &state.out_of_band {
        if let Some(l) = &o.license {
            out.push(l.clone());
        }
    }
    out
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
        let st = detect(&PathBuf::from("x.txt"), &bytes, &oob);
        assert!(!st.encoding_ok);
    }

    #[test]
    fn block_comment_header() {
        let h = parse_headers("<!-- SPDX-License-Identifier: CC0-1.0 -->\n");
        assert_eq!(h[0].license_ids, vec!["CC0-1.0".to_string()]);
    }
}
