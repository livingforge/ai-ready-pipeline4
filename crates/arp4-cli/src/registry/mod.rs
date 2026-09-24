//! Current canonical records; Git owns previous versions.
mod validation;
use validation::conflicts;
pub use validation::{fingerprint, validate};
mod persistence;
use persistence::refresh;
pub use persistence::{load, save};
mod mutation;
pub use mutation::{apply, import, preflight};
mod render;
pub use render::{render, summary};

use crate::{
    data::{encoded, hash, read},
    specification_ids::Ledger,
    specifications::{self as spec, AtomicValue, Condition, Span},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Requirement,
    Specification,
    Observation,
    Estimate,
    Reference,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Proposed,
    Approved,
    Retired,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub snapshot: String,
    pub span: Span,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub actor: String,
    pub reason: String,
    pub revision: u64,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub module: String,
    pub status: Status,
    pub subject: String,
    pub property: String,
    pub condition: Condition,
    pub value: AtomicValue,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    pub acceptance: Vec<String>,
    pub requirements: Vec<String>,
    pub related: Vec<String>,
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    pub approval: Option<Approval>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    pub category: Category,
    pub module: String,
    #[serde(default)]
    pub requirements: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub actor: String,
    pub reason: String,
    pub modules: BTreeMap<String, String>,
    pub open_issues: Vec<String>,
    /// Explicit issue scope; an empty document set denotes a project-wide issue.
    pub open_issue_documents: BTreeMap<String, BTreeSet<String>>,
    pub entries: BTreeMap<String, Classification>,
}

impl Catalog {
    pub fn validate_issue_documents(&self, input: &spec::Input) -> Result<()> {
        let issues: BTreeSet<_> = self.open_issues.iter().collect();
        ensure!(
            issues.len() == self.open_issues.len()
                && issues == self.open_issue_documents.keys().collect(),
            "each open issue requires an explicit document scope"
        );
        for (issue, documents) in &self.open_issue_documents {
            nonempty(issue)?;
            ensure!(
                documents
                    .iter()
                    .all(|doc| input.revisions.contains_key(doc)),
                "unknown open issue document"
            );
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRecord {
    pub revision: u64,
    pub parent: Option<String>,
    pub actor: String,
    pub reason: String,
    pub changed: Vec<String>,
    pub impacted: Vec<String>,
    pub approved: Vec<String>,
    pub aliases: BTreeMap<String, String>,
    pub resolved_issues: BTreeMap<String, String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub project: String,
    pub revision: u64,
    pub parent: Option<String>,
    pub modules: BTreeMap<String, String>,
    pub open_issues: Vec<String>,
    pub records: BTreeMap<String, String>,
    pub evidence: BTreeSet<String>,
    pub archives: BTreeMap<String, String>,
    pub issued: BTreeSet<String>,
    pub last_change: ChangeRecord,
}
pub struct Registry {
    pub manifest: Manifest,
    pub entries: BTreeMap<String, Entry>,
    pub inputs: BTreeMap<String, Value>,
    pub archives: BTreeMap<String, Value>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approve {
    pub id: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub base_hash: String,
    pub actor: String,
    pub reason: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub retire: Vec<String>,
    #[serde(default)]
    pub approve: Vec<Approve>,
    #[serde(default)]
    pub modules: BTreeMap<String, String>,
    /// Keys are exact existing issue descriptions, values are documented resolutions.
    #[serde(default)]
    pub resolve_issues: BTreeMap<String, String>,
    #[serde(default)]
    pub add_issues: Vec<String>,
}
fn nonempty(s: &str) -> Result<()> {
    ensure!(!s.trim().is_empty(), "empty required text");
    Ok(())
}
fn slug(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
fn valid_id(s: &str) -> bool {
    s.split_once('-').is_some_and(|(p, n)| {
        ["REQ", "SPEC", "OBS", "EST", "REF"].contains(&p)
            && n.len() >= 6
            && n.bytes().all(|c| c.is_ascii_digit())
            && n.parse::<u64>().is_ok_and(|n| n > 0)
    })
}
fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(encoded(&serde_json::to_value(v)?))
}
