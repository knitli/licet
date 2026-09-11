//! Batch writing for out-of-band REUSE metadata: routing annotation requests
//! to their winning documents and appending exact-path stanzas that reuse
//! their document'"'"'s own newline convention (FR-015).

use super::*;

/// Outcome of [`write_annotation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationWrite {
    /// The file's effective `REUSE.toml` license already matched; nothing written.
    Unchanged,
    /// A new exact-path annotation was appended — covering a previously
    /// uncovered file, overriding a broader glob entry, or superseding an
    /// older exact-path entry. Being last, it wins per REUSE 3.3.
    Appended,
}

impl AnnotationWrite {
    /// Whether the file was actually rewritten.
    pub fn modified(self) -> bool {
        !matches!(self, AnnotationWrite::Unchanged)
    }
}

/// One exact-path annotation the caller wants covered by `REUSE.toml`.
pub struct AnnotationRequest<'a> {
    pub rel_path: &'a str,
    pub license: &'a str,
    pub copyrights: &'a [String],
}

/// Where one annotation patch belongs: the document that wins for the path,
/// the path relative to that document's base, and whether the stanza needs
/// `override` precedence to govern there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationDestination {
    /// Repo-relative document path (`REUSE.toml`, `sub/REUSE.toml`).
    pub doc_rel: PathBuf,
    /// Request path relative to the document's base.
    pub base_path: String,
    /// True when only an `override` stanza governs at the destination: under
    /// an `override` barrier, or when an `aggregate` table contributes
    /// unconditionally (a default-`closest` stanza would neither break the
    /// barrier nor silence the aggregate).
    pub use_override: bool,
}

/// Route an annotation to its winning document (FR-003a).
///
/// The walk mirrors [`OutOfBand::lookup`] over the already-loaded hierarchy:
/// - no coverage, or `closest` governing → the nearest consulted document
///   (the project root when nothing consults), default precedence — nearest
///   wins the per-field `closest` fallback;
/// - `override` or `aggregate` governing → the project root with `override`
///   precedence — root is consulted first, so the appended stanza breaks
///   before every deeper table and supersedes root tables by last match.
pub fn annotation_destination(oob: &OutOfBand, rel_path: &str) -> AnnotationDestination {
    let mut consulted: Vec<(usize, Precedence)> = Vec::new();
    for (di, doc) in oob.docs.iter().enumerate() {
        let Some(remainder) = under_base(rel_path, &doc.base) else {
            continue;
        };
        if let Some(table) = doc
            .tables
            .iter()
            .rev()
            .find(|t| matches_any(&t.matchers, remainder))
        {
            let is_override = table.precedence == Precedence::Override;
            consulted.push((di, table.precedence));
            if is_override {
                break;
            }
        }
    }
    let governing = if consulted.iter().any(|(_, p)| *p == Precedence::Override) {
        Some(Precedence::Override)
    } else if consulted.iter().any(|(_, p)| *p == Precedence::Aggregate) {
        Some(Precedence::Aggregate)
    } else if consulted.is_empty() {
        None
    } else {
        Some(Precedence::Closest)
    };
    match governing {
        Some(Precedence::Override) | Some(Precedence::Aggregate) => AnnotationDestination {
            doc_rel: PathBuf::from("REUSE.toml"),
            base_path: rel_path.to_string(),
            use_override: true,
        },
        Some(Precedence::Closest) => {
            let (di, _) = consulted.last().copied().expect("consulted non-empty");
            let doc = &oob.docs[di];
            AnnotationDestination {
                doc_rel: doc.rel.clone(),
                base_path: under_base(rel_path, &doc.base)
                    .unwrap_or(rel_path)
                    .to_string(),
                use_override: false,
            }
        }
        None => AnnotationDestination {
            doc_rel: PathBuf::from("REUSE.toml"),
            base_path: rel_path.to_string(),
            use_override: false,
        },
    }
}

/// Ensure a `REUSE.toml` annotation covers `rel_path` with `license` (FR-015, FR-003a).
///
/// Single-request form of [`write_annotations`]; `oob` is the scan's loaded
/// hierarchy, used for coverage and destination routing.
pub fn write_annotation(
    root: &Path,
    rel_path: &str,
    license: &str,
    copyrights: &[String],
    oob: &OutOfBand,
) -> Result<AnnotationWrite, crate::reuse::WriteError> {
    let outcomes = write_annotations(
        root,
        &[AnnotationRequest {
            rel_path,
            license,
            copyrights,
        }],
        oob,
    )?;
    Ok(outcomes
        .per_request
        .into_iter()
        .next()
        .unwrap_or(AnnotationWrite::Unchanged))
}

/// One patched metadata document: the before/after bytes plus which caller
/// requests its stanzas carry, so the caller files one write record per
/// document with the covered assets attached.
#[derive(Debug, Clone)]
pub struct DocPatch {
    pub doc_rel: PathBuf,
    pub before: Option<Vec<u8>>,
    pub after: Vec<u8>,
    pub requests: Vec<usize>,
}

/// Outcome of [`write_annotations`]: per-request dispositions plus the
/// document patches actually committed (one per touched document).
#[derive(Debug, Clone)]
pub struct AnnotationBatchOutcome {
    pub per_request: Vec<AnnotationWrite>,
    pub patches: Vec<DocPatch>,
}

/// Ensure `REUSE.toml` annotations cover every request (FR-015, FR-003a).
///
/// `oob` is the scan's loaded hierarchy: coverage and destination routing read
/// it, so decisions match what detection sees. Each request whose effective
/// license *and* copyrights already match is left untouched
/// ([`AnnotationWrite::Unchanged`]); every other request gains one appended
/// exact-path stanza ([`AnnotationWrite::Appended`]) in its winning document
/// ([`annotation_destination`]). Creates the project root document with a
/// `version = 1` header if absent; never creates deeper documents.
///
/// The patch is strictly append-only: existing documents are never reparsed
/// with a lossy line scan and never reformatted, so comments, ordering, and
/// unrelated stanzas survive byte-for-byte, and a repeated run converges.
/// Requests group by destination document, so each document is read, patched,
/// and written exactly once. Every patched document is re-parsed — and every
/// appended coverage verified — before anything is committed. Copyrights
/// serialize as one string/list field, never repeated keys; exact paths escape
/// REUSE glob metacharacters so odd filenames cannot become new patterns;
/// appended stanzas reuse their document's own newline convention.
///
/// An unreadable or non-UTF-8 document is an error, never silently replaced
/// with fresh content (F04).
pub fn write_annotations(
    root: &Path,
    requests: &[AnnotationRequest<'_>],
    oob: &OutOfBand,
) -> Result<AnnotationBatchOutcome, crate::reuse::WriteError> {
    use crate::reuse::{WriteError, atomic_write, read_expected_for_write};
    use std::collections::BTreeMap;

    let mut per_request = vec![AnnotationWrite::Unchanged; requests.len()];
    // Requests needing a stanza, grouped by destination document so each
    // document is read, patched, and written exactly once.
    let mut pending: BTreeMap<PathBuf, Vec<(usize, AnnotationDestination)>> = BTreeMap::new();
    for (i, req) in requests.iter().enumerate() {
        let entry = oob.lookup(Path::new(req.rel_path));
        let covered = entry.as_ref().and_then(|e| {
            e.license()
                .or_else(|| crate::domain::combine_licenses(&e.fallback_licenses))
        });
        let copyrights_match = entry.as_ref().is_some_and(|e| {
            norm_set(effective_copyrights(e)) == norm_set(req.copyrights.to_vec())
        });
        if covered.as_deref() == Some(req.license) && copyrights_match {
            continue;
        }
        let dest = annotation_destination(oob, req.rel_path);
        pending
            .entry(dest.doc_rel.clone())
            .or_default()
            .push((i, dest));
    }
    if pending.is_empty() {
        return Ok(AnnotationBatchOutcome {
            per_request,
            patches: Vec::new(),
        });
    }

    let mut patches: Vec<DocPatch> = Vec::new();

    for (doc_rel, jobs) in &pending {
        let existing_bytes =
            read_expected_for_write(&root.join(doc_rel)).map_err(WriteError::for_read_failure)?;
        let existing = match &existing_bytes {
            Some(bytes) => std::str::from_utf8(bytes).map_err(|e| {
                WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("existing {} is not valid UTF-8: {e}", doc_rel.display()),
                ))
            })?,
            // Only the project root document may be created; a missing deeper
            // document means the hierarchy moved under us — refuse, don't invent.
            None if doc_rel == Path::new("REUSE.toml") => "",
            None => {
                return Err(WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "annotated document {} disappeared during the scan",
                        doc_rel.display()
                    ),
                )));
            }
        };

        let nl = if existing.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut out = existing.to_owned();
        if out.is_empty() {
            out.push_str("version = 1");
            out.push_str(nl);
            out.push_str(nl);
        } else if !out.ends_with('\n') {
            out.push_str(nl);
        }
        let (stanzas, appended) = render_stanzas(jobs, requests, nl);
        out.push_str(&stanzas);
        for i in appended {
            per_request[i] = AnnotationWrite::Appended;
        }

        // Re-parse the complete proposed document before committing, and
        // verify every appended coverage through the same lookup detection
        // uses (this also validates the escaping and list serialization).
        let mut verify = OutOfBand::default();
        verify.parse_reuse_toml(doc_rel, &out).map_err(|e| {
            WriteError::for_read_failure(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("proposed {} failed to re-parse: {e}", doc_rel.display()),
            ))
        })?;
        for (i, _) in jobs {
            let req = &requests[*i];
            // Verify through the document's own base, exactly as detection
            // will read it back.
            let got = verify.lookup(Path::new(req.rel_path)).and_then(|e| {
                e.license()
                    .or_else(|| crate::domain::combine_licenses(&e.fallback_licenses))
            });
            if got.as_deref() != Some(req.license) {
                return Err(WriteError::for_read_failure(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "proposed {} does not cover {} with {}",
                        doc_rel.display(),
                        req.rel_path,
                        req.license
                    ),
                )));
            }
        }

        atomic_write(root, doc_rel, existing_bytes.as_deref(), out.as_bytes())?;
        patches.push(DocPatch {
            doc_rel: doc_rel.clone(),
            before: existing_bytes.clone(),
            after: out.into_bytes(),
            requests: jobs.iter().map(|(i, _)| *i).collect(),
        });
    }
    Ok(AnnotationBatchOutcome {
        per_request,
        patches,
    })
}

/// Serialize one document's appended stanzas: exact paths with REUSE glob
/// metacharacters escaped, copyrights as absent/single/list fields, and the
/// document's own newline convention. Pure rendering — returns the stanza
/// text plus the covered request indices (callers mark them `Appended` only
/// once the whole document verifies), so TOML shape and escaping are
/// directly unit-testable.
pub(crate) fn render_stanzas(
    jobs: &[(usize, AnnotationDestination)],
    requests: &[AnnotationRequest<'_>],
    nl: &str,
) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut appended = Vec::with_capacity(jobs.len());
    for (i, dest) in jobs {
        let req = &requests[*i];
        out.push_str("[[annotations]]");
        out.push_str(nl);
        out.push_str(&format!(
            "path = {}",
            toml_string(&escape_reuse_path(&dest.base_path))
        ));
        out.push_str(nl);
        if dest.use_override {
            out.push_str("precedence = \"override\"");
            out.push_str(nl);
        }
        match req.copyrights.len() {
            0 => {}
            1 => {
                out.push_str(&format!(
                    "SPDX-FileCopyrightText = {}",
                    toml_string(&req.copyrights[0])
                ));
                out.push_str(nl);
            }
            _ => {
                let list = req
                    .copyrights
                    .iter()
                    .map(|s| toml_string(s))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("SPDX-FileCopyrightText = [{list}]"));
                out.push_str(nl);
            }
        }
        out.push_str(&format!(
            "SPDX-License-Identifier = {}",
            toml_string(req.license)
        ));
        out.push_str(nl);
        out.push_str(nl);
        appended.push(*i);
    }
    (out, appended)
}

/// Effective OOB copyrights for a lookup entry, mirroring the license logic:
/// unconditional contributors first, else the `closest` fallback.
fn effective_copyrights(entry: &crate::domain::OutOfBandEntry) -> Vec<String> {
    if entry.copyrights.is_empty() {
        entry.fallback_copyrights.clone()
    } else {
        entry.copyrights.clone()
    }
}

/// Order-insensitive comparison form for notice lists.
fn norm_set(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

/// Escape the REUSE glob metacharacters in an exact path so the emitted stanza
/// cannot act as a pattern: a file named `a*b.png` must not start covering
/// its siblings. Only `\` and `*` are escaped: both engines treat `\`-asterisk
/// and `\\`-backslash as escapes, while `?`, brackets, and braces are literal
/// in both grammars and must stay bare (this crate's matcher and the reference
/// tool's pathspec grammar agree on all of this).
fn escape_reuse_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '\\' | '*') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Quote a value for emission into a `REUSE.toml` stanza (write-side escaping only).
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
