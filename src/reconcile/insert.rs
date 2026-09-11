//! First-line-aware header insertion: skip shebang, encoding/XML decl, and BOM, inserting
//! the header immediately after them (FR-019, research §6). Line endings preserved (FR-026).

/// The dominant newline convention of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// Detect the file's newline convention (CRLF if the first newline is preceded by CR).
pub fn detect_line_ending(content: &str) -> LineEnding {
    match content.find('\n') {
        Some(i) if i > 0 && content.as_bytes()[i - 1] == b'\r' => LineEnding::Crlf,
        _ => LineEnding::Lf,
    }
}

/// Compute the byte offset at which a new header should be inserted, skipping a leading
/// BOM, shebang, and/or encoding/XML declaration.
pub fn insertion_offset(content: &str) -> usize {
    let bytes = content.as_bytes();
    let mut offset = 0usize;

    // BOM.
    if content.starts_with('\u{feff}') {
        offset += '\u{feff}'.len_utf8();
    }

    // Shebang must remain the first line.
    let rest = &content[offset..];
    if rest.starts_with("#!") {
        offset += line_len(&bytes[offset..]);
    }

    // A `<? ... ?>` processing instruction (XML declaration, PHP open tag)
    // yields only through its close, so a same-line body stays after the
    // header; other encoding declarations take the whole first line.
    let rest = &content[offset..];
    let first_line_end = line_len(rest.as_bytes());
    let first_line = &rest[..first_line_end];
    if first_line.starts_with("<?") {
        match first_line.find("?>") {
            Some(i) => offset += i + "?>".len(),
            None => offset += first_line_end,
        }
    } else if first_line.contains("coding:") || first_line.contains("coding=") {
        offset += first_line_end;
    }

    // Language-specific first lines that must stay first (FR-019): the PHP
    // open tag, the Cabal project header, and TeX/BibTeX magic comments — the
    // same positions `leading_position` recognizes for detection.
    let rest = &content[offset..];
    let first_line_end = line_len(rest.as_bytes());
    let first_line = &rest[..first_line_end];
    if first_line.starts_with("<?")
        || first_line.starts_with("cabal-version:")
        || first_line.starts_with("% !TEX")
        || first_line.starts_with("%TEX")
        || first_line.starts_with("% !BIB")
        || first_line.starts_with("%!BIB")
    {
        offset += first_line_end;
    }

    offset
}

/// Length in bytes of the next line including its terminator.
fn line_len(bytes: &[u8]) -> usize {
    match bytes.iter().position(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => bytes.len(),
    }
}

/// Insert `header` (LF-terminated lines) at the computed offset, adapting line endings and
/// ensuring blank-line separation from following content.
pub fn insert_header(content: &str, header: &str) -> String {
    let ending = detect_line_ending(content);
    let offset = insertion_offset(content);

    // Adapt the header's line endings to the file's convention.
    let header = if ending == LineEnding::Crlf {
        header.replace('\n', "\r\n")
    } else {
        header.to_string()
    };

    let (prefix, suffix) = content.split_at(offset);
    let mut out = String::with_capacity(content.len() + header.len() + 2);
    out.push_str(prefix);
    // Ensure the prefix ended with a newline (when non-empty) before the header.
    if !prefix.is_empty() && !prefix.ends_with('\n') {
        out.push_str(ending.as_str());
    }
    out.push_str(&header);
    // Blank line between header and existing body, if body is non-empty.
    if !suffix.is_empty() && !suffix.starts_with('\n') && !suffix.starts_with('\r') {
        out.push_str(ending.as_str());
    }
    out.push_str(suffix);
    out
}

// REUSE-IgnoreStart — SPDX tags in the tests below are fixtures, not this file's licensing.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_at_top_plain() {
        let out = insert_header("code\n", "// SPDX-License-Identifier: MIT\n");
        assert!(out.starts_with("// SPDX-License-Identifier: MIT\n"));
        assert!(out.contains("code"));
    }

    #[test]
    fn inserts_after_shebang() {
        let out = insert_header("#!/bin/sh\ncode\n", "# SPDX-License-Identifier: MIT\n");
        assert!(out.starts_with("#!/bin/sh\n"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("SPDX-License-Identifier")
        );
    }

    #[test]
    fn preserves_crlf() {
        let out = insert_header("code\r\nmore\r\n", "// SPDX-License-Identifier: MIT\n");
        assert!(out.contains("// SPDX-License-Identifier: MIT\r\n"));
    }

    #[test]
    fn inserts_after_xml_decl() {
        let out = insert_header(
            "<?xml version=\"1.0\"?>\n<root/>\n",
            "<!-- SPDX-License-Identifier: MIT -->\n",
        );
        assert!(out.starts_with("<?xml"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("SPDX-License-Identifier")
        );
    }

    #[test]
    fn inserts_after_php_tag() {
        let out = insert_header("<?php\necho 'hi';\n", "// SPDX-License-Identifier: MIT\n");
        assert!(out.starts_with("<?php\n"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("SPDX-License-Identifier")
        );
    }

    #[test]
    fn inserts_after_cabal_version() {
        let out = insert_header(
            "cabal-version: 3.0\nname: demo\n",
            "-- SPDX-License-Identifier: MIT\n",
        );
        assert!(out.starts_with("cabal-version: 3.0\n"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("SPDX-License-Identifier")
        );
    }

    #[test]
    fn inserts_after_tex_magic() {
        let out = insert_header(
            "% !TEX program = xelatex\n\\documentclass{article}\n",
            "% SPDX-License-Identifier: MIT\n",
        );
        assert!(out.starts_with("% !TEX program = xelatex\n"));
        assert!(
            out.lines()
                .nth(1)
                .unwrap()
                .contains("SPDX-License-Identifier")
        );
    }

    #[test]
    fn inserts_after_xml_close_before_same_line_body() {
        let out = insert_header(
            "<?xml version=\"1.0\"?><root/>\n",
            "<!-- SPDX-License-Identifier: MIT -->\n",
        );
        assert_eq!(
            out,
            "<?xml version=\"1.0\"?>\n<!-- SPDX-License-Identifier: MIT -->\n\n<root/>\n"
        );
    }

    #[test]
    fn inserts_after_shebang_and_encoding_decl() {
        let out = insert_header(
            "#!/usr/bin/env python\n# coding: utf-8\nprint('hi')\n",
            "# SPDX-License-Identifier: MIT\n",
        );
        assert_eq!(
            out,
            "#!/usr/bin/env python\n# coding: utf-8\n# SPDX-License-Identifier: MIT\n\nprint('hi')\n"
        );
    }

    #[test]
    fn bom_then_shebang_stay_contiguous() {
        let out = insert_header(
            "\u{feff}#!/bin/sh\necho hi\n",
            "# SPDX-License-Identifier: MIT\n",
        );
        assert_eq!(
            out,
            "\u{feff}#!/bin/sh\n# SPDX-License-Identifier: MIT\n\necho hi\n"
        );
    }

    #[test]
    fn no_trailing_newline_gets_terminated_body() {
        let out = insert_header("code", "// SPDX-License-Identifier: MIT\n");
        assert_eq!(out, "// SPDX-License-Identifier: MIT\n\ncode");
    }

    #[test]
    fn unicode_body_survives_insertion() {
        let out = insert_header(
            "fn héllo(){} // héllo\n",
            "// SPDX-License-Identifier: MIT\n",
        );
        assert_eq!(
            out,
            "// SPDX-License-Identifier: MIT\n\nfn héllo(){} // héllo\n"
        );
    }

    #[test]
    fn crlf_shebang_keeps_crlf() {
        let out = insert_header("#!/bin/sh\r\ncode\r\n", "# SPDX-License-Identifier: MIT\n");
        assert_eq!(
            out,
            "#!/bin/sh\r\n# SPDX-License-Identifier: MIT\r\n\r\ncode\r\n"
        );
    }

    #[test]
    fn multiline_block_header_inserts_verbatim() {
        let out = insert_header("code\n", "/*\n * SPDX-License-Identifier: MIT\n */\n");
        assert_eq!(out, "/*\n * SPDX-License-Identifier: MIT\n */\n\ncode\n");
    }
}
// REUSE-IgnoreEnd
