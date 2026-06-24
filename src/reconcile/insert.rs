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

    // Encoding / XML declaration on the (new) first line.
    let rest = &content[offset..];
    let first_line_end = line_len(rest.as_bytes());
    let first_line = &rest[..first_line_end];
    if first_line.starts_with("<?xml")
        || first_line.contains("coding:")
        || first_line.contains("coding=")
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
        assert!(out
            .lines()
            .nth(1)
            .unwrap()
            .contains("SPDX-License-Identifier"));
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
        assert!(out
            .lines()
            .nth(1)
            .unwrap()
            .contains("SPDX-License-Identifier"));
    }
}
