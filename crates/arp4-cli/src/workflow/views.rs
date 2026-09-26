use super::*;

impl Workflow {
    /// Agent-facing task row: the short reference, progress fields and structured
    /// diagnostics. Artifact digests and the escaped error text stay in state.json.
    pub(super) fn task_view(&self, task: &Task) -> Value {
        let mut view = json!({"task_ref":self.short_id(task),"stage":task.stage,"state":task.state,
            "document":task.document,"partition":task.partition,"failure_class":task.failure_class,
            "retry_reason":task.retry_reason,"diagnostics":task.diagnostics});
        view.as_object_mut()
            .unwrap()
            .retain(|_, value| !value.is_null());
        view
    }
    pub(super) fn status(&self) -> Result<Value> {
        let tasks: Vec<_> = self
            .state
            .tasks
            .iter()
            .map(|task| self.task_view(task))
            .collect();
        let rounds_remaining = self.state.max_rounds.saturating_sub(self.state.round);
        let escalation = if self.state.escalation["reason"].is_null() {
            Value::Null
        } else {
            json!({"reason":self.state.escalation["reason"]})
        };
        let quality = self.quality_view()?;
        let mut status = json!({"status":self.state.status,"round":self.state.round,"repair_rounds":self.state.repair_rounds,"max_rounds":self.state.max_rounds,"rounds_remaining":rounds_remaining,"review_granularity":self.state.review_granularity,"tasks":tasks,"blocked":self.state.blocked,
            "next_actions":self.next_actions(),"quality":quality,"provenance":self.provenance_summary(),"run_id":self.run_id(),"notices":self.notices(),"exports":self.state.exports,"escalation":escalation,"drafts":self.state.drafts.as_object().map(|d| d.keys().map(|name| format!("{}/draft/{name}", self.working_dir())).collect::<Vec<_>>()),"runs":self.state.runs.len(),
            "concerns":self.concerns(None, quality["diagnostics"].as_array().map(Vec::as_slice).unwrap_or_default())?,
            "references":{"count":self.state.references.as_object().map_or(0, |r|r.len()),"details_command":"read --task <task_ref> --pointer references"}});
        // Absent means empty here; tasks and next_actions stay visible even when empty.
        status["review_plan"] = json!(self.state.review_plan);
        status.as_object_mut().unwrap().retain(|key, value| {
            ["status", "tasks", "next_actions"].contains(&key.as_str())
                || !(value.is_null()
                    || value.as_array().is_some_and(Vec::is_empty)
                    || value.as_object().is_some_and(Map::is_empty))
        });
        Ok(status)
    }
    /// Progress and decisions remain visible; detailed diagnostics are read by
    /// the assigned agent, accounting and review configuration only on demand.
    pub(super) fn status_summary(&self) -> Result<Value> {
        let mut status = self.status()?;
        let fields = status.as_object_mut().unwrap();
        for key in ["quality", "provenance", "review_plan", "references", "runs"] {
            fields.remove(key);
        }
        for task in fields.get_mut("tasks").unwrap().as_array_mut().unwrap() {
            let task = task.as_object_mut().unwrap();
            let has_diagnostics = task.remove("diagnostics").is_some();
            if has_diagnostics {
                task.insert("has_diagnostics".into(), json!(true));
            }
        }
        fields.insert("details_command".into(), json!("status"));
        Ok(status)
    }
    /// The run identifier is the working directory name under .arp/work/workflow.
    pub(super) fn run_id(&self) -> String {
        self.store
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    /// Repository-relative working directory; --root plus --run-id reproduce it.
    pub(super) fn working_dir(&self) -> String {
        format!(".arp/work/workflow/{}", self.run_id())
    }
    pub(super) fn notices(&self) -> Value {
        crate::data::grouped_warnings(
            self.state
                .notices
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
    }
}
