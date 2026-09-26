//! Read-only interactive views. Agents never need to decode prompt strings or objects.
use super::*;

/// Normalize transport wrapping only; never repair JSON or alter source strings.
pub(super) fn parse_reply(bytes: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(bytes)?
        .trim_start_matches('\u{feff}')
        .trim();
    let text = if let Some(body) = text
        .strip_prefix("```json\r\n")
        .or_else(|| text.strip_prefix("```json\n"))
        .or_else(|| text.strip_prefix("```\r\n"))
        .or_else(|| text.strip_prefix("```\n"))
    {
        body.strip_suffix("```")
            .context("unclosed JSON code fence")?
            .trim()
    } else {
        text
    };
    parse(text.as_bytes())
}

pub(super) fn reply_input(path: Option<&Path>, inline: Option<&str>) -> Result<Value> {
    match (path, inline) {
        (Some(path), None) => super::command::read_reply(path),
        (None, Some(text)) => parse_reply(text.as_bytes()),
        _ => anyhow::bail!("provide exactly one of --reply or --json"),
    }
}

const SECTIONS: [&str; 11] = [
    "instructions",
    "packet",
    "context",
    "scope",
    "reply_schema",
    "module_vocabulary",
    "references",
    "previous_error",
    "previous_reply",
    "draft",
    "reply_template",
];

impl Workflow {
    pub(super) fn recorded_task_id(&self, id: &str) -> Result<String> {
        let normalized = id.to_ascii_lowercase();
        let id = normalized.as_str();
        ensure!(
            id.len() >= 8 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "task requires a full ID or unique hexadecimal prefix of at least 8 characters"
        );
        let ids: BTreeSet<&str> = self
            .state
            .tasks
            .iter()
            .map(|t| t.id.as_str())
            .chain(
                self.state
                    .submissions
                    .iter()
                    .filter_map(|s| s["task"].as_str()),
            )
            .filter(|known| known.starts_with(id))
            .collect();
        ensure!(!ids.is_empty(), "unknown task");
        ensure!(ids.len() == 1, "ambiguous task prefix; use a longer ID");
        Ok((*ids.first().unwrap()).to_owned())
    }

    pub(super) fn inspect_task(&self, id: &str) -> Result<Value> {
        let id = self.recorded_task_id(id)?;
        if let Some(task) = self.state.tasks.iter().find(|task| task.id == id) {
            return self.brief(task);
        }
        let entry = self
            .state
            .submissions
            .iter()
            .rev()
            .find(|entry| entry["task"] == id)
            .unwrap();
        Ok(
            json!({"task_id":id,"stage":entry["stage"],"document":entry["document"],
            "state":"retired","submission_accepted":entry["accepted"],
            "reason":entry["reason"],"details_command":"history",
            "meaning":"This task was submitted and is no longer active. Accepted records describe submission acceptance, not current publication approval."}),
        )
    }

    pub(super) fn handoff(&self, task: &Task, binary: &Path) -> Result<Value> {
        ensure!(
            matches!(task.state, TaskState::Pending | TaskState::Failed),
            "only pending or failed tasks can be assigned"
        );
        let repository = self
            .store
            .root
            .ancestors()
            .nth(4)
            .context("missing repository root")?;
        let mut assignment = self.summary(task);
        assignment["task_id"] = json!(task.id);
        assignment["executable"] = json!(binary);
        assignment["working_directory"] = json!(std::env::current_dir()?);
        assignment["workflow_arguments"] = json!([
            "spec",
            "workflow",
            "--root",
            repository,
            "--run-id",
            self.run_id()
        ]);
        assignment["claimed"] = json!(false);
        assignment["agent"] = json!(if task.stage.is_review() {
            "arp4-reviewer"
        } else {
            "arp4-worker"
        });
        assignment["instructions"] = json!(
            "Follow the named custom agent definition. Use the supplied executable, working_directory and workflow_arguments; start with read --task <task_ref> --max-bytes 12000. On success, no report (only '完了' if required); on interruption, briefly flag it and report only blockers absent from CLI records."
        );
        Ok(assignment)
    }

    pub(super) fn receipt(&self, id: &str) -> Result<Value> {
        let id = self.recorded_task_id(id)?;
        let task = self.state.tasks.iter().find(|t| t.id == id);
        let latest = self
            .state
            .submissions
            .iter()
            .rev()
            .find(|s| s["task"] == id);
        Ok(json!({
            "task_id":id,
            "stage":task.map(|t| json!(t.stage)).or_else(|| latest.map(|s| s["stage"].clone())),
            "state":task.map(|t| json!(t.state)).unwrap_or(json!("retired")),
            "submission_accepted":latest.map(|s| &s["accepted"]),
            "submission":self.state.submissions.iter().filter(|s| s["task"] == id).count(),
            "actor":latest.map(|s| &s["actor"]),
            "origin":latest.map(|s| &s["origin"]),
            "workflow_status":self.state.status,
            "meaning":"Latest recorded submission only; null means not submitted. Acceptance does not establish semantic correctness or publication approval."
        }))
    }

    /// What `next` returns: enough to pick the task and start reading. The usage
    /// guidance is repeated only by `inspect`, once per task.
    pub(super) fn summary(&self, task: &Task) -> Value {
        let task_ref = self.short_id(task);
        let mut summary = json!({"task_ref":task_ref,"stage":task.stage,"state":task.state,
            "read":{"command":"read","task":task_ref},
            "independent_review_required":task.stage.is_review()});
        // Cross-document stages (link, global review) have no document.
        if !task.document.is_none() {
            summary["document"] = json!(task.document);
        }
        summary
    }

    pub(super) fn brief(&self, task: &Task) -> Result<Value> {
        let mut brief = self.summary(task);
        brief["task_id"] = json!(task.id);
        brief["working_root"] = json!(self.working_dir());
        brief["configuration"] = json!(
            "Keep the same repository --root, --run-id and working directory for every command, including delegated agents. Configuration is .arp/config.yml."
        );
        brief["sections"] = json!(SECTIONS.map(|key| format!("/{key}")));
        let data = self.interactive_data(task)?;
        let format = crate::project::agent_read_format(&self.store.root)?;
        brief["agent_read_format"] = json!(format);
        brief["section_bytes"] = json!(
            SECTIONS
                .iter()
                .map(|key| Ok((format!("/{key}"), format.encode(&data[key])?.len() + 1)))
                .collect::<Result<BTreeMap<_, _>>>()?
        );
        brief["cli_usage"] = json!(
            "Start with read --task ID --max-bytes 12000 without --pointer and read output directly. Append page.next_command.bash or .powershell to the same executable and workflow root/run-id prefix until page.complete. Do not create display-processing files/scripts or slice output. --max-bytes sets the response byte budget in the configured TOON or JSON format (4096..48000, default 48000); A larger budget may be selected at offset 0 only after verifying that this host displays it completely; choose a smaller budget if the host truncates or spills output. A revision mismatch requires restarting at offset 0. A truncated or spilled response is not a completed read; restart with a smaller budget. Entries contain actual values; pointers label locations, not retrieval instructions. Array chunks carry zero-based array_offset and array_total; long strings carry string_offset and string_total_bytes in UTF-8 bytes. Do not explore child pointers or truncate reading material. Optional --pointer selects a subtree for targeted inspection (omit the leading slash in Git Bash). No --full. Lock contention waits up to 30 seconds inside the CLI."
        );
        brief["workflow"] = json!(
            "Read all pages of read --task ID directly in the configured TOON (default) or JSON format; do not open internal JSON objects instead: instructions, packet, context, scope, reply_schema, module_vocabulary, references, previous_error, previous_reply, draft and reply_template are included. Source rows use sources.columns order and sources.tables metadata. Submit semantic JSON with validate-reply/submit --reply FILE or --reply - (UTF-8 stdin). After rejection read draft and diagnostics; submit only changed fields using draft.revision and draft.schema. The CLI merges and revalidates the whole reply. Use submit --reason-file FILE for UTF-8 reasons containing quotes, JSON fragments or newlines; --reason is for short inline text and cannot be combined with --reason-file. No helper scripts or manual merging. A template is incomplete, not a completed review."
        );
        Ok(brief)
    }

    pub(super) fn interactive_data(&self, task: &Task) -> Result<Value> {
        let mut packet = if task.packet.is_some() {
            self.artifact(&task.packet)?
        } else {
            Value::Null
        };
        let mut context = if task.context.is_some() {
            self.artifact(&task.context)?
        } else {
            Value::Null
        };
        let stage = task.stage;
        if stage.is_review() {
            packet = confirmation::delivery(&packet, &context["confirmation"])?;
            // Rationale/provenance is stored by the CLI, not re-read as if it
            // were fresh audit work. Source lists explain the inherited scope.
            if let Some(carried) = context
                .pointer_mut("/confirmation/carried_audits")
                .and_then(Value::as_array_mut)
            {
                for audit in carried {
                    audit.as_object_mut().unwrap().remove("rationale");
                }
            }
        }
        // Persisted packets remain complete for validation and context expansion.
        // Each piece of evidence has one owner in the agent-facing view.
        if stage == Stage::Repair {
            packet = json!({"packet":packet["packet"],"document":packet["document"]});
        }
        if stage == Stage::Link {
            context = Value::Null;
        }
        let scope = if task.scope.is_some() {
            self.artifact(&task.scope)?
        } else {
            Value::Null
        };
        let instructions = match stage {
            Stage::Extract => format!(
                "{}\nWhen scope is present, extract scope.sources only; scope.context is context, not a target; sources.tables merges and structure cover only their cells and the headers they name, and omitted_cells counts element cells left out. If context is insufficient return only packet, document, request_tables (sources.tables indexes), reason. Keep local keys; the CLI namespaces partitions. In modules, define new slugs only (or use an empty object); the CLI fills registered names for referenced slugs from module_vocabulary. Explicit conflicting names and undefined slugs are rejected.",
                crate::semantic::PROMPT
            ),
            Stage::Link => linking::instructions().to_owned(),
            Stage::Repair => patch::instructions(),
            Stage::Restructure => format!(
                "{}\n{}",
                restructure::instructions(),
                crate::semantic::PROMPT
            ),
            Stage::Review | Stage::GlobalReview => crate::semantic::REVIEW_PROMPT.to_owned(),
        };
        let instructions = if task.incremental == Some(true) {
            format!(
                "{instructions}\nThis is incremental extraction: read context.previous_items and context.reserved_keys. Re-extract the entire owned scope from current sources, preserving keys only for retained claims. Previous items are not authoritative facts. Do not reuse reserved keys. Omit relationships for the linking stage; retained items are merged by the CLI."
            )
        } else {
            instructions
        };
        let references = self.references()?;
        let instructions = if references.is_empty() {
            instructions
        } else {
            format!("{instructions}\n{REFERENCE_GUIDANCE}")
        };
        Ok(
            json!({"instructions":format!("{instructions}\nIdentity fields use workflow-local references. Copy them unchanged from this task; never calculate hashes or reuse references from another run."),"packet":self.compact_payload(packet)?,"context":self.compact_payload(context)?,"scope":self.compact_payload(scope)?,
            "reply_schema":crate::semantic_contract::workflow_reply_schema(self.reply_schema(task)),"module_vocabulary":self.module_vocabulary()?,"references":references,
            "previous_error":if task.diagnostics.is_null() { json!(task.error) } else { task.diagnostics.clone() },"previous_reply":if task.rejected_reply.is_some() {self.compact_payload(self.artifact(&task.rejected_reply)?)?} else {Value::Null},
            "draft":self.draft_info(task)?,"reply_template":self.compact_payload(self.template(task)?)?}),
        )
    }

    /// The unfilled reply: fields the CLI already knows carry their values, work
    /// fields are empty. An empty array or object here is never a valid answer.
    fn template(&self, task: &Task) -> Result<Value> {
        let artifact = |digest: &Option<String>| -> Result<Value> {
            if digest.is_some() {
                self.artifact(digest)
            } else {
                Ok(Value::Null)
            }
        };
        Ok(match task.stage {
            Stage::Extract => {
                let packet = artifact(&task.packet)?;
                json!({"packet":packet["packet"],"document":packet["document"],"modules":{},"items":[],"exclusions":[],"open_issues":[]})
            }
            Stage::Link => {
                json!({"packet":artifact(&task.packet)?["packet"],"links":[],"open_issues":[]})
            }
            Stage::Repair => json!({"base":artifact(&task.scope)?["base"],"changes":[]}),
            Stage::Restructure => {
                json!({"bases":artifact(&task.context)?["bases"],"edits":[],"mapping":[]})
            }
            Stage::Review | Stage::GlobalReview => {
                let packet = artifact(&task.packet)?;
                let mut reply = json!({"packet":packet["packet"],"document":packet["document"],"audits":[],"findings":[]});
                for key in ["sheet", "scope"] {
                    if let Some(value) = packet.get(key) {
                        reply[key] = value.clone();
                    }
                }
                reply
            }
        })
    }

    /// Fill the CLI-known fields (packet fingerprint, document, review sheet/scope,
    /// repair base, restructure bases) a reply left out, so agents need not copy
    /// references. Supplied references are resolved before validation; work fields such as
    /// items or links are never defaulted.
    pub(super) fn complete_reply(&self, task: &Task, reply: Value) -> Result<Value> {
        let reply = self.expand_draft(task, reply)?;
        let reply = self.expand_transport_reply(task, reply)?;
        let Value::Object(mut fields) = reply else {
            return Ok(reply);
        };
        for (key, value) in obj(&self.template(task)?)? {
            let empty = value.as_array().is_some_and(Vec::is_empty)
                || value.as_object().is_some_and(Map::is_empty);
            if !empty && !value.is_null() && !fields.contains_key(key) {
                fields.insert(key.clone(), value.clone());
            }
        }
        // Registered names are mechanical metadata. Preserve explicit definitions
        // so conflicting names still reach validation; never guess a new module.
        if task.stage == Stage::Extract {
            let vocabulary = self.module_vocabulary()?;
            let referenced: Vec<String> = fields
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| item["classification"]["module"].as_str().map(str::to_owned))
                .collect();
            if let Some(modules) = fields.get_mut("modules").and_then(Value::as_object_mut) {
                for slug in referenced {
                    if let Some(name) = vocabulary.get(&slug) {
                        modules.entry(slug).or_insert_with(|| name.clone());
                    }
                }
            }
        }
        Ok(Value::Object(fields))
    }

    pub(super) fn read_section(
        &self,
        task: &Task,
        pointer: &str,
        offset: usize,
        limit: usize,
        max_bytes: usize,
        revision: Option<&str>,
    ) -> Result<Value> {
        // Slash-free section names survive Git Bash's native argument conversion.
        let normalized = if pointer.is_empty() || pointer.starts_with('/') {
            pointer.to_owned()
        } else {
            format!("/{pointer}")
        };
        let pointer = normalized.as_str();
        let options = super::paging::ReadOptions {
            format: crate::project::agent_read_format(&self.store.root)?,
            pointer,
            offset,
            limit,
            max_bytes,
            revision,
        };
        super::paging::check_options(limit, max_bytes)?;
        let task_ref = self.short_id(task);
        let cache = super::read_cache::ReadCache::new(
            &self.store.root,
            &task.id,
            self.read_key(task, &options)?,
        );
        if let Some(index) = cache.load() {
            let mut missing = false;
            let page = super::paging::select(
                &task_ref,
                &index.view,
                |range| cache.fetch(&index, range).inspect_err(|_| missing = true),
                &options,
            );
            // A layout another reader is replacing is laid out again.
            if !missing {
                return page;
            }
        }
        let data = self.interactive_data(task)?;
        let layout = super::paging::layout(&data, &SECTIONS, &options)?;
        let open: BTreeSet<&str> = self
            .state
            .tasks
            .iter()
            .filter(|t| t.state != TaskState::Complete || t.id == task.id)
            .map(|t| t.id.as_str())
            .collect();
        // An unwritable cache only costs the next page a new layout.
        let _ = cache
            .store(&layout)
            .and_then(|()| super::read_cache::prune(&self.store.root, &open));
        super::paging::select(
            &task_ref,
            &layout.view,
            |range| Ok(layout.fragments[range].to_vec()),
            &options,
        )
    }

    /// Everything the task view is built from (see `interactive_data`), the page
    /// shape, and this executable, whose prompts and schemas are part of the view.
    fn read_key(&self, task: &Task, options: &super::paging::ReadOptions<'_>) -> Result<String> {
        let completed: Vec<_> = self
            .state
            .tasks
            .iter()
            .filter(|t| t.stage == Stage::Extract && t.state == TaskState::Complete)
            .map(|t| &t.reply)
            .collect();
        let executable = std::env::current_exe()?.metadata()?;
        let modified = executable
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string();
        Ok(hash(&encode(&json!([
            task,
            self.state.modules,
            self.state.replies,
            completed,
            self.state.references,
            self.state.transport_refs,
            options.pointer,
            options.max_bytes,
            options.format,
            env!("CARGO_PKG_VERSION"),
            executable.len(),
            modified,
        ]))))
    }
}
