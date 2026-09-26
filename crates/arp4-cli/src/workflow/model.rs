//! Persisted workflow control state. Agent-authored payloads remain JSON.
use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Extract,
    Link,
    Repair,
    Restructure,
    Review,
    GlobalReview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Complete,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    New,
    AwaitingReplies,
    Blocked,
    Complete,
    NeedsDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ReviewGranularity {
    Adaptive,
    Document,
    Sheet,
}

impl Stage {
    pub fn is_review(self) -> bool {
        matches!(self, Self::Review | Self::GlobalReview)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Task {
    pub id: String,
    pub stage: Stage,
    pub state: TaskState,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub bases: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub diagnostics: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected_reply: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkflowState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_plan: Option<crate::specifications::ReviewPlan>,
    pub version: u32,
    pub engine: String,
    pub status: WorkflowStatus,
    pub tasks: Vec<Task>,
    pub round: u64,
    pub max_rounds: u64,
    pub review_granularity: ReviewGranularity,
    pub extract_max_chars: usize,
    pub extract_max_sources: usize,
    pub extract_max_bytes: usize,
    pub binary_hash: String,
    pub input: String,
    pub regions: String,
    pub replies: Value,
    pub packets: Value,
    pub reviews: Value,
    pub runs: Vec<Value>,
    pub repair_rounds: std::collections::BTreeMap<String, u64>,
    pub modules: Value,
    pub submissions: Vec<Value>,
    pub extraction_cache: Value,
    pub transport_refs: std::collections::BTreeMap<String, String>,
    pub transport_namespace: String,
    /// Task inputs whose identities are already in `transport_refs`. Artifacts
    /// never change, so each is read once rather than on every save.
    pub transport_scanned: std::collections::BTreeSet<String>,
    /// The `modules` each live reply artifact declares, by digest. Artifacts never
    /// change, so the vocabulary reads each reply once rather than every reply on
    /// every submission. Filled on first use; `save` keeps the live replies only.
    pub reply_modules: std::cell::RefCell<std::collections::BTreeMap<String, Value>>,
    pub blocked: Value,
    pub bundle: Value,
    pub exports: Value,
    pub deferred: Value,
    pub notices: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub unresolved: Value,
    pub references: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub quality: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub incremental: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub findings: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub escalation: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub drafts: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub reviewed_replies: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub review_findings: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub previous: Value,
}

impl WorkflowState {
    /// Decode the persisted control structure before any operation mutates it.
    /// JSON remains only for schemas, diagnostics and artifact collections whose
    /// contents are inspected by the semantic validators.
    pub fn decode(value: Value) -> Result<Self> {
        let state: Self = serde_json::from_value(value).context("invalid workflow state")?;
        let references = serde_json::to_value(&state.transport_refs)?;
        ensure!(
            crate::semantic_contract::errors(
                &crate::semantic_contract::workflow_reference_schema()["$defs"]["namespace"],
                &serde_json::json!(state.transport_namespace)
            )?
            .is_empty(),
            "invalid transport namespace"
        );
        ensure!(
            crate::semantic_contract::errors(
                &crate::semantic_contract::workflow_reference_schema(),
                &references
            )?
            .is_empty(),
            "invalid transport reference table"
        );
        ensure!(
            state
                .transport_refs
                .values()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == state.transport_refs.len()
                && (1..=state.transport_refs.len()).all(|i| state
                    .transport_refs
                    .contains_key(&format!("r{}.{i}", state.transport_namespace))),
            "invalid transport reference sequence"
        );
        ensure!(
            state.max_rounds > 0 && state.extract_max_bytes > 0,
            "invalid workflow limits"
        );
        for (name, value) in [
            ("replies", &state.replies),
            ("packets", &state.packets),
            ("reviews", &state.reviews),
            ("extraction_cache", &state.extraction_cache),
            ("bundle", &state.bundle),
            ("exports", &state.exports),
            ("references", &state.references),
            ("modules", &state.modules),
        ] {
            let values = value
                .as_object()
                .with_context(|| format!("workflow {name} must be an object"))?;
            ensure!(
                values.values().all(Value::is_string),
                "workflow {name} values must be strings"
            );
        }
        ensure!(
            state
                .reply_modules
                .borrow()
                .values()
                .all(|modules| modules.is_object() || modules.is_null()),
            "workflow reply_modules values must be objects or null"
        );
        for (name, value) in [
            ("blocked", &state.blocked),
            ("deferred", &state.deferred),
            ("notices", &state.notices),
        ] {
            ensure!(value.is_array(), "workflow {name} must be an array");
        }
        ensure!(
            state.unresolved.is_null() || state.unresolved.is_string(),
            "workflow unresolved must be an object reference when present"
        );
        ensure!(
            state.quality["diagnostics"].is_null() || state.quality["diagnostics"].is_string(),
            "workflow quality diagnostics must be an object reference"
        );
        for (name, value) in [
            ("findings", &state.findings),
            ("review_findings", &state.review_findings),
        ] {
            ensure!(
                value.is_null() || value.is_array(),
                "workflow {name} must be an array when present"
            );
        }
        for (name, value) in [
            ("quality", &state.quality),
            ("incremental", &state.incremental),
            ("escalation", &state.escalation),
            ("drafts", &state.drafts),
            ("reviewed_replies", &state.reviewed_replies),
        ] {
            ensure!(
                value.is_null() || value.is_object(),
                "workflow {name} must be an object when present"
            );
        }
        Ok(state)
    }
}

#[cfg(test)]
impl WorkflowState {
    pub fn test_state() -> Self {
        serde_json::from_value(serde_json::json!({"version":1,"engine":"arp4-rust","status":"new","tasks":[],"round":0,"max_rounds":8,"review_granularity":"adaptive","extract_max_chars":0,"extract_max_sources":0,"extract_max_bytes":98304,"binary_hash":"test","input":"test","regions":"test","replies":{},"packets":{},"reviews":{},"runs":[],"repair_rounds":{},"modules":{},"submissions":[],"extraction_cache":{},"transport_refs":{},"transport_namespace":"0000000000000000000000000","transport_scanned":[],"reply_modules":{},"blocked":[],"bundle":{},"exports":{},"deferred":[],"notices":[],"references":{}})).unwrap()
    }
}
