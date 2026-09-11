//! Lookup resolution for out-of-band REUSE metadata: per-path coverage with
//! root-first hierarchy, `override` barriers, and `closest` fallbacks,
//! mirroring the reference REUSE tool (FR-003a).

use super::*;

use crate::domain::{MetadataOrigin, OobSource, OutOfBandEntry};

impl OutOfBand {
    /// Out-of-band coverage for a repo-relative path, if any.
    ///
    /// Each document contributes exclusively its last matching table; tables
    /// are consulted root-first and consultation stops after the rootmost
    /// `override` table (mirroring the reference tool). The returned entry
    /// carries unconditional (`aggregate`/barrier) values, per-field
    /// `closest` fallbacks, and every contributing table's provenance.
    pub fn lookup(&self, rel_path: &Path) -> Option<OutOfBandEntry> {
        let rel_str = rel_path.to_string_lossy().replace('\\', "/");
        // Per-document last match, root-first, stopping after an override.
        let mut consulted: Vec<(&ReuseDoc, &OobTable)> = Vec::new();
        for doc in &self.docs {
            let Some(remainder) = under_base(&rel_str, &doc.base) else {
                continue;
            };
            if let Some(table) = doc
                .tables
                .iter()
                .rev()
                .find(|t| matches_any(&t.matchers, remainder))
            {
                let is_override = table.precedence == Precedence::Override;
                consulted.push((doc, table));
                if is_override {
                    break;
                }
            }
        }
        // dep5 paragraphs aggregate; the last matching paragraph wins (as in
        // the reference tool), contributing alongside any REUSE tables.
        let dep5_hit = self
            .dep5
            .iter()
            .rev()
            .find(|p| matches_any(&p.matchers, &rel_str));
        if consulted.is_empty() && dep5_hit.is_none() {
            return None;
        }
        let barrier = consulted
            .iter()
            .any(|(_, t)| t.precedence == Precedence::Override);
        let mut licenses: Vec<String> = Vec::new();
        let mut copyrights: Vec<String> = Vec::new();
        let mut origins: Vec<MetadataOrigin> = Vec::new();
        let mut primary_source = OobSource::ReuseToml;
        let mut has_aggregate = false;
        // Unconditional contributors: the barrier table first (if any), then
        // every consulted `aggregate` table shallowest-first, then dep5 —
        // matching the reference tool's override-before-aggregate order.
        let mut unconditional: Vec<(&ReuseDoc, &OobTable)> = Vec::new();
        if barrier {
            let (doc, table) = consulted
                .last()
                .expect("barrier implies a consulted override table");
            debug_assert_eq!(table.precedence, Precedence::Override);
            unconditional.push((*doc, *table));
        }
        for (doc, table) in &consulted {
            if table.precedence == Precedence::Aggregate {
                has_aggregate = true;
                unconditional.push((*doc, *table));
            }
        }
        for (doc, table) in unconditional {
            push_unique(&mut licenses, table.licenses.iter().cloned());
            push_unique(&mut copyrights, table.copyrights.iter().cloned());
            origins.push(MetadataOrigin {
                metadata_path: doc.rel.clone(),
                table_index: table.index,
                precedence: table.precedence,
                licenses: table.licenses.clone(),
                copyrights: table.copyrights.clone(),
            });
        }
        if let Some(para) = dep5_hit {
            has_aggregate = true;
            if origins.is_empty() {
                primary_source = OobSource::Dep5;
            }
            push_unique(&mut licenses, para.licenses.iter().cloned());
            push_unique(&mut copyrights, para.copyrights.iter().cloned());
            origins.push(MetadataOrigin {
                metadata_path: PathBuf::from(".reuse/dep5"),
                table_index: para.index,
                precedence: Precedence::Aggregate,
                licenses: para.licenses.clone(),
                copyrights: para.copyrights.clone(),
            });
        }
        // Per-field nearest-outward `closest` fallback (deepest consulted
        // table wins each field independently).
        let mut fallback_licenses: Vec<String> = Vec::new();
        let mut fallback_copyrights: Vec<String> = Vec::new();
        let mut fallback_lic_origin: Option<MetadataOrigin> = None;
        let mut fallback_cpr_origin: Option<MetadataOrigin> = None;
        for (doc, table) in consulted.iter().rev() {
            if table.precedence != Precedence::Closest {
                continue;
            }
            if fallback_licenses.is_empty() && !table.licenses.is_empty() {
                fallback_licenses = table.licenses.clone();
                fallback_lic_origin = Some(MetadataOrigin {
                    metadata_path: doc.rel.clone(),
                    table_index: table.index,
                    precedence: table.precedence,
                    licenses: table.licenses.clone(),
                    copyrights: table.copyrights.clone(),
                });
            }
            if fallback_copyrights.is_empty() && !table.copyrights.is_empty() {
                fallback_copyrights = table.copyrights.clone();
                fallback_cpr_origin = Some(MetadataOrigin {
                    metadata_path: doc.rel.clone(),
                    table_index: table.index,
                    precedence: table.precedence,
                    licenses: table.licenses.clone(),
                    copyrights: table.copyrights.clone(),
                });
            }
        }
        if fallback_lic_origin
            .as_ref()
            .is_some_and(|o| Some(o) != fallback_cpr_origin.as_ref())
        {
            origins.push(fallback_lic_origin.expect("checked"));
        }
        if let Some(o) = fallback_cpr_origin {
            origins.push(o);
        }
        if licenses.is_empty()
            && copyrights.is_empty()
            && fallback_licenses.is_empty()
            && fallback_copyrights.is_empty()
        {
            // Tables matched but none carries any licensing information — an
            // override barrier still suppresses the file (reference behavior),
            // otherwise there is no coverage at all.
            if !barrier {
                return None;
            }
        }
        // Origins read shallowest-document first (dep5 is root-level).
        origins.sort_by_key(|o| (o.metadata_path.clone(), o.table_index));
        let precedence = if barrier {
            Precedence::Override
        } else if has_aggregate {
            Precedence::Aggregate
        } else {
            Precedence::Closest
        };
        Some(OutOfBandEntry {
            source: primary_source,
            licenses,
            copyrights,
            fallback_licenses,
            fallback_copyrights,
            suppresses_file: barrier,
            precedence,
            origins,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty() && self.dep5.is_empty()
    }

    /// Whether `.reuse/dep5` exists in the loaded snapshot (used to refuse
    /// writes that would create a mutually-exclusive `REUSE.toml` next to it).
    pub fn has_dep5(&self) -> bool {
        self.dep5_present
    }
}

fn push_unique(target: &mut Vec<String>, iter: impl Iterator<Item = String>) {
    for item in iter {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}
