//! Small, source-bound semantic replies; deterministic expansion into the full model.
pub(crate) use assembly::expand_item;
pub(crate) use packet::{sources, span};
mod packet;
use packet::relevant_warnings;
use packet::source_rows;
pub use packet::{changes, compact_review, packet, packet_fingerprint};
mod assembly;
pub use assembly::{Reply, assemble};
mod review_packet;
pub use review_packet::{ReviewScope, review_packet, review_packet_scoped, review_packet_selected};
mod review;
pub use review::{AuditGroup, Review, ReviewAction, ReviewFinding, apply_reviews};

use crate::{
    data::{encoded, hash},
    registry::{Catalog, Classification},
    specifications::{self as spec, Input, Model, Source},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const PROMPT: &str = include_str!("../../../../surface/prompts/semantic.md");
pub const REVIEW_PROMPT: &str = include_str!("../../../../surface/prompts/semantic-review.md");

pub fn remap_catalog(catalog: &mut Catalog, aliases: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        catalog.entries.keys().collect::<BTreeSet<_>>() == aliases.keys().collect(),
        "catalog and identity keys differ"
    );
    let resolve = |key: &str| {
        aliases
            .get(key)
            .cloned()
            .context("unknown catalog relationship")
    };
    let mut entries = BTreeMap::new();
    for (key, mut entry) in std::mem::take(&mut catalog.entries) {
        entry.requirements = entry
            .requirements
            .iter()
            .map(|s| resolve(s))
            .collect::<Result<_>>()?;
        entry.related = entry
            .related
            .iter()
            .map(|s| resolve(s))
            .collect::<Result<_>>()?;
        entries.insert(resolve(&key)?, entry);
    }
    catalog.entries = entries;
    Ok(())
}
