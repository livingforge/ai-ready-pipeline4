//! Persistent, provider-independent initial specification generation.
mod command;
mod confirmation;
pub use command::{WorkflowCommand, execute};
mod artifacts;
mod draft;
mod findings;
mod model;
mod planning;
mod references;
mod submission;
mod transitions;
mod update;
mod views;
pub use model::ReviewGranularity;
use model::{Stage, Task, TaskState, WorkflowState, WorkflowStatus};
mod extraction;
mod incremental;
mod interaction;
mod linking;
mod paging;
mod patch;
mod publication;
mod quality;
mod restructure;
mod runner;
mod store;
mod transport;
use crate::data::hash;
use anyhow::{Context, Result, ensure};
pub use publication::publish;
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use store::{Store, encode};
pub(crate) fn s(v: &Value) -> Result<&str> {
    v.as_str().context("expected string")
}
pub(crate) fn arr(v: &Value) -> Result<&Vec<Value>> {
    v.as_array().context("expected array")
}
pub(crate) fn obj(v: &Value) -> Result<&Map<String, Value>> {
    v.as_object().context("expected object")
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Value> {
    crate::data::parse(std::str::from_utf8(bytes)?, true)
}
pub(crate) fn read(path: &Path) -> Result<Value> {
    parse(&fs::read(path)?)
}
const MAX_ADAPTIVE_DOCUMENT_REVIEW_BYTES: usize = 96 * 1024;
const MAX_ADAPTIVE_DOCUMENT_REVIEW_SOURCES: usize = 512;
const MAX_REFERENCE_BYTES: usize = 1 << 20;
const ESCALATION_GUIDANCE: &str = "Check each concern against the original source. If the extraction is wrong, report a finding that names the items and the concrete correction. If the extraction is faithful and the diagnostic is a validator limitation, report no finding for it: it stays listed for a human decision, and formal export remains unavailable. Never change source meaning to satisfy a diagnostic. The concerns listed here are already recorded: do not report them again unless you find a different defect; a repeated finding with the same action and items is folded into the existing record. A needs_decision concern includes a human-directed finding or a declined repair with its reasons; it is neither resolved nor missing. Improvements do not require repair.";
const REFERENCE_GUIDANCE: &str = "Reference material (read --pointer references) is supporting context supplied by the project, not an extraction source: it may settle a cross-document conflict or explain a term. Cite it as ref:<path> in a finding message, reason or open issue. Never use it as evidence or a quote source, and never change source meaning to match it.";

struct Workflow {
    store: Store,
    state: WorkflowState,
    // Process-local only: never trust a persisted validation receipt after restart.
    validation_cache: std::cell::RefCell<BTreeMap<String, Value>>,
}
impl Workflow {
    fn clear_task_failure(&mut self, i: usize) {
        let task = &mut self.state.tasks[i];
        task.error = None;
        task.diagnostics = Value::Null;
        task.failure_class = None;
        task.rejected_reply = None;
        task.draft = None;
    }
    fn save(&mut self) -> Result<()> {
        self.register_transport_refs()?;
        let mut value = serde_json::to_value(&self.state)?;
        self.store.save(&mut value)?;
        self.state.previous = value["previous"].clone();
        Ok(())
    }
    fn index(&self, id: &str) -> Result<usize> {
        let normalized = id.to_ascii_lowercase();
        let id = normalized.as_str();
        let tasks = &self.state.tasks;
        if let Some(i) = tasks.iter().position(|t| t.id == id) {
            return Ok(i);
        }
        ensure!(
            id.len() >= 8 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "task requires a full ID or unique hexadecimal prefix of at least 8 characters"
        );
        let mut matches = tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id.starts_with(id));
        let (i, _) = matches.next().context("unknown task")?;
        ensure!(
            matches.next().is_none(),
            "ambiguous task prefix; use a longer ID from status or next"
        );
        Ok(i)
    }
    fn short_id(&self, task: &Task) -> String {
        self.short_ref(task.id.as_str())
    }
    /// Shortest prefix (at least 8 characters) unique among the current tasks.
    fn short_ref(&self, id: &str) -> String {
        for length in 8..id.len() {
            let prefix = &id[..length];
            if self
                .state
                .tasks
                .iter()
                .filter(|t| t.id.starts_with(prefix))
                .count()
                == 1
            {
                return prefix.to_owned();
            }
        }
        id.to_owned()
    }
    fn find(&self, id: &str) -> Result<&Task> {
        Ok(&self.state.tasks[self.index(id)?])
    }
    fn artifact(&self, digest: &Option<String>) -> Result<Value> {
        self.store
            .json(digest.as_deref().context("task artifact required")?)
    }
    fn task(&self, stage: Stage, text: &[u8], mut meta: Value) -> Result<Task> {
        if stage == Stage::Repair {
            let scope = self.store.json(s(&meta["scope"])?)?;
            let complex = arr(&scope["items"])?.iter().any(|i| {
                i["condition"]["basis"] == "composed"
                    || i["evidence"].as_array().is_some_and(|e| e.len() > 1)
            });
            meta["effort_hint"] = json!(if complex { "high" } else { "medium" });
        }
        meta["stage"] = json!(stage);
        meta["task"] = json!(self.store.put(text)?);
        meta["id"] = json!(hash(&encode(&meta)));
        meta["state"] = json!(TaskState::Pending);
        if stage == Stage::Extract
            && let Some(reply) = self.state.extraction_cache.get(s(&meta["id"])?)
        {
            meta["reply"] = reply.clone();
            meta["state"] = json!(TaskState::Complete);
        }
        Ok(serde_json::from_value(meta)?)
    }
    fn queue(&mut self, tasks: Vec<Task>) -> Result<Value> {
        self.state.tasks = tasks;
        self.state.status = WorkflowStatus::AwaitingReplies;
        self.state.blocked = json!([]);
        self.save()?;
        Ok(self.status())
    }
    fn block(&mut self, reasons: Value) -> Result<Value> {
        self.state.status = WorkflowStatus::Blocked;
        self.state.blocked = reasons;
        self.save()?;
        Ok(self.status())
    }
    fn advance(&mut self) -> Result<Value> {
        if self.state.status == WorkflowStatus::Complete {
            self.export()?;
            return Ok(self.status());
        }
        if self.state.status == WorkflowStatus::NeedsDecision {
            return Ok(self.status());
        }
        let result = match self.advance_inner() {
            Ok(v) => Ok(v),
            Err(error) => {
                self.block(json!([{"code":"validation_failed","message":format!("{error:#}")}]))
            }
        };
        self.write_unresolved()?;
        result
    }
    fn advance_inner(&mut self) -> Result<Value> {
        if !self.consume()? {
            return Ok(self.status());
        }
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let (context, tasks) = self.prepare_extraction(root)?;
        if !tasks.is_empty() {
            return self.queue(tasks);
        }
        if let Some(task) = self.linking_task(&context.typed_input)? {
            return self.queue(vec![task]);
        }
        let report = self.assemble_bundle(root, &context)?;
        let stalled = self.quality_progress(&report["issues"])?;
        self.prune_unresolved(&context.bases)?;
        let findings = json!(self.findings());
        self.state.deferred = json!([]);
        let open: Vec<Value> = arr(&report["issues"])?
            .iter()
            .filter(|issue| issue["code"] != "missing_semantic_audit")
            .cloned()
            .collect();
        let unresolved = arr(&findings)?.iter().any(|f| f["action"] != "improvement")
            || !open.is_empty()
            || !self.unresolved().is_empty();
        if unresolved && stalled && !self.escalated() {
            self.escalate("stalled_diagnostics");
        }
        if unresolved && !self.escalated() {
            let tasks = self.plan_repairs(&context, &report, &findings)?;
            if !tasks.is_empty() {
                return self.queue(tasks);
            }
        }
        let reviews = self.plan_reviews(root, &context, &open, &findings, unresolved)?;
        if !reviews.tasks.is_empty() {
            return self.queue(reviews.tasks);
        }
        if reviews.unreviewed {
            return self.needs_decision(
                root,
                &context.input_path,
                &root.join("assembled/model.json"),
                false,
            );
        }
        self.publish_reviewed(root, &context.input_path, &reviews.accepted, unresolved)
    }
}
/// Identity of a review finding across rounds: the same action on the same
/// items is the same finding even when a new reviewer words it differently.
pub(super) fn finding_signature(finding: &Value) -> String {
    let mut items: Vec<&str> = finding["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    items.sort_unstable();
    if items.is_empty() {
        json!([finding["action"], finding["message"]]).to_string()
    } else {
        json!([finding["action"], items]).to_string()
    }
}
/// Identity of a machine diagnostic on one item.
pub(super) fn diagnostic_signature(code: &Value, id: &str) -> String {
    json!([code, id]).to_string()
}
/// Whether a finding is already recorded: as a whole, or item by item, since a
/// declined repair records the finding per key it was asked to change.
fn finding_known(finding: &Value, known: &BTreeSet<String>) -> bool {
    if known.contains(&finding_signature(finding)) {
        return true;
    }
    let items: Vec<&str> = finding["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    !items.is_empty()
        && items.iter().all(|id| {
            known.contains(&finding_signature(
                &json!({"action":finding["action"],"items":[id]}),
            ))
        })
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}
