use super::*;
use clap::Subcommand;

pub(super) fn read_reply(path: &Path) -> Result<Value> {
    if path == Path::new("-") {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes)?;
        interaction::parse_reply(&bytes)
    } else {
        interaction::parse_reply(&fs::read(path)?)
    }
}

fn reason_input(reason: Option<String>, file: Option<PathBuf>, default: &str) -> Result<String> {
    let reason = if let Some(path) = file {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("cannot read UTF-8 reason file: {}", path.display()))?;
        text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
    } else {
        reason.unwrap_or_else(|| default.to_owned())
    };
    ensure!(!reason.trim().is_empty(), "reason must not be blank");
    Ok(reason)
}

#[derive(Subcommand)]
pub enum WorkflowCommand {
    Init {
        /// JSON review plan with source IDs omitted from review and explicit reasons.
        #[arg(long)]
        review_plan: Option<PathBuf>,
        #[arg(long)]
        input: PathBuf,
        #[arg(long, num_args=1..)]
        reply: Vec<PathBuf>,
        #[arg(long)]
        regions: Option<PathBuf>,
        /// Maximum repair/restructure waves for the entire run; then confirm and emit a draft if issues remain.
        #[arg(long, default_value_t = 2)]
        max_rounds: u64,
        /// Review unit selection. Adaptive batches small documents and splits large ones by sheet.
        #[arg(long, value_enum, default_value_t = ReviewGranularity::Adaptive)]
        review_granularity: ReviewGranularity,
        /// Shared module slug/name vocabulary (JSON object).
        #[arg(long)]
        modules: Option<PathBuf>,
        /// Optional source-text character cap; zero disables it.
        #[arg(long, default_value_t = 0)]
        extract_max_chars: usize,
        /// Optional source count cap; zero disables it.
        #[arg(long, default_value_t = 0)]
        extract_max_sources: usize,
        /// UTF-8 task size, including instructions, schema and context.
        #[arg(long, default_value_t = 98304)]
        extract_max_bytes: usize,
        /// Reference text files (Markdown, plain text): supporting context for
        /// review and repair, readable with read --pointer references. Never a source.
        #[arg(long, num_args=1..)]
        reference: Vec<PathBuf>,
    },
    Status {
        /// Omit detailed diagnostics and accounting from the orchestration view.
        #[arg(long)]
        summary: bool,
    },
    /// Read the next pending task without advancing state or invoking a model.
    Next,
    /// Prepare a compact worker assignment without reading task evidence or claiming it.
    Handoff {
        #[arg(long)]
        task: String,
    },
    /// Confirm the latest recorded submission, including tasks retired after advance.
    Receipt {
        #[arg(long)]
        task: String,
    },
    /// Read the complete task, automatically traversing nested data in bounded pages.
    /// Start without --pointer. Continue with page.next_command for your shell,
    /// keeping the same executable and workflow root/run-id prefix until page.complete.
    Read {
        #[arg(long)]
        task: String,
        #[arg(long, default_value = "")]
        pointer: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Optional fragment count cap.
        #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u32).range(1..=10000))]
        limit: u32,
        /// Maximum reading response bytes (configured TOON or JSON). Lower for host truncation.
        #[arg(long, default_value_t = 48000, value_parser = clap::value_parser!(u32).range(4096..=48000))]
        max_bytes: u32,
        /// Copy page.revision when continuing; changed content requires restarting at offset 0.
        #[arg(long)]
        revision: Option<String>,
    },
    /// Check a complete reply or draft correction without saving artifacts or history.
    ValidateReply {
        #[arg(long)]
        task: String,
        #[arg(long, required_unless_present = "json", conflicts_with = "json")]
        reply: Option<PathBuf>,
        /// Inline JSON; use --reply - for large UTF-8 replies.
        #[arg(long)]
        json: Option<String>,
    },
    /// Inspect provider calls and external intervention history.
    History,
    /// Attach measured deltas to one external submission, including a retired task.
    /// Existing measurements may be repeated identically, but never overwritten or added twice.
    RecordUsage {
        #[arg(long)]
        task: String,
        /// One-based submission number within the task, as shown by history/receipt.
        #[arg(long)]
        submission: usize,
        /// Same measured usage JSON as submit --usage-file; no agent lifetime totals.
        #[arg(long)]
        usage_file: PathBuf,
    },
    Advance,
    Inspect {
        #[arg(long)]
        task: String,
    },
    /// Submit a complete reply or a revision-bound draft correction; validate the whole result.
    Submit {
        #[arg(long)]
        task: String,
        #[arg(long, required_unless_present = "json", conflicts_with = "json")]
        reply: Option<PathBuf>,
        #[arg(long)]
        json: Option<String>,
        #[arg(long, default_value = "external")]
        actor: String,
        #[arg(long)]
        model: Option<String>,
        /// Short inline reason; defaults to "Externally supplied reply" when omitted.
        #[arg(long, conflicts_with = "reason_file")]
        reason: Option<String>,
        /// Read the reason verbatim from a UTF-8 text file (optional BOM).
        #[arg(long)]
        reason_file: Option<PathBuf>,
        #[arg(long, default_value = "external", value_parser = ["external", "interactive-agent"])]
        origin: String,
        /// JSON with measured usage and reported_cost_usd; never estimates.
        #[arg(long)]
        usage_file: Option<PathBuf>,
    },
    Update {
        #[arg(long)]
        input: Option<PathBuf>,
        #[arg(long,num_args=1..)]
        reply: Vec<PathBuf>,
        #[arg(long)]
        regions: Option<PathBuf>,
        /// Replace the reference text files of the run.
        #[arg(long, num_args=1..)]
        reference: Vec<PathBuf>,
    },
    Run(runner::RunOptions),
    Recover {
        #[arg(long)]
        task: String,
    },
    Retry {
        #[arg(long)]
        task: String,
        /// Short inline reason; use --reason-file for quotes or multiline text.
        #[arg(
            long,
            required_unless_present = "reason_file",
            conflicts_with = "reason_file"
        )]
        reason: Option<String>,
        /// Read the reason verbatim from a UTF-8 text file (optional BOM).
        #[arg(long)]
        reason_file: Option<PathBuf>,
    },
    Export,
    Compact,
}

pub fn execute(root: &Path, mut command: WorkflowCommand) -> Result<Value> {
    let shared = matches!(
        &command,
        WorkflowCommand::Status { .. }
            | WorkflowCommand::History
            | WorkflowCommand::Next
            | WorkflowCommand::Handoff { .. }
            | WorkflowCommand::Receipt { .. }
            | WorkflowCommand::Inspect { .. }
            | WorkflowCommand::Read { .. }
            | WorkflowCommand::ValidateReply { .. }
    );
    let store = Store::open_mode(root, shared)?;
    let binary = dunce::canonicalize(std::env::current_exe()?)?;
    if let WorkflowCommand::Init {
        review_plan,
        input,
        reply,
        regions,
        max_rounds,
        review_granularity,
        modules,
        extract_max_chars,
        extract_max_sources,
        extract_max_bytes,
        reference,
    } = command
    {
        ensure!(
            !store.managed("state.json")?.exists() && max_rounds > 0,
            "workflow exists or invalid round limit; keep the same root and use status, update --reply, submit, retry or recover instead of init"
        );
        ensure!(
            extract_max_bytes > 0,
            "positive extraction byte limit required"
        );
        let vocabulary = modules.map(|p| read(&p)).transpose()?.unwrap_or(json!({}));
        for (slug, name) in obj(&vocabulary)? {
            ensure!(
                !slug.is_empty()
                    && slug
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                    && !s(name)?.trim().is_empty(),
                "invalid module vocabulary"
            );
        }
        let mut replies = Map::new();
        for path in reply {
            let v = read(&path)?;
            ensure!(
                replies
                    .insert(s(&v["document"])?.to_owned(), json!(store.put_json(&v)?))
                    .is_none(),
                "duplicate document reply"
            );
        }
        let review_plan: Option<crate::specifications::ReviewPlan> = review_plan
            .map(|p| Ok::<_, anyhow::Error>(serde_json::from_value(read(&p)?)?))
            .transpose()?;
        if let Some(plan) = &review_plan {
            plan.validate(&crate::specifications::load_input(&input)?)?;
        }
        let state = WorkflowState {
            review_plan,
            version: 1,
            engine: "arp4-rust".into(),
            binary_hash: hash(&fs::read(&binary)?),
            input: store.put_json(&read(&input)?)?,
            regions: store
                .put_json(&regions.map(|p| read(&p)).transpose()?.unwrap_or(json!({})))?,
            status: WorkflowStatus::New,
            tasks: Vec::new(),
            round: 0,
            max_rounds,
            review_granularity,
            extract_max_chars,
            extract_max_sources,
            extract_max_bytes,
            replies: Value::Object(replies),
            modules: vocabulary,
            runs: Vec::new(),
            submissions: Vec::new(),
            repair_rounds: BTreeMap::new(),
            packets: json!({}),
            reviews: json!({}),
            extraction_cache: json!({}),
            transport_refs: BTreeMap::new(),
            transport_namespace: transport::namespace(),
            bundle: json!({}),
            exports: json!({}),
            references: json!({}),
            blocked: json!([]),
            deferred: json!([]),
            notices: json!([]),
            unresolved: json!([]),
            quality: Value::Null,
            incremental: Value::Null,
            findings: Value::Null,
            escalation: Value::Null,
            drafts: Value::Null,
            reviewed_replies: Value::Null,
            review_findings: Value::Null,
            previous: Value::Null,
        };
        let mut flow = Workflow {
            store,
            state,
            validation_cache: Default::default(),
        };
        flow.state.references = flow.load_references(&reference)?;
        flow.validate_replies(&flow.state.replies)?;
        let imported = obj(&flow.state.replies)?.clone();
        for (doc, digest) in imported {
            let reply = flow.store.json(s(&digest)?)?;
            flow.record_submission(
                &json!({"stage":"import","document":doc}),
                &reply,
                &json!({"actor":"external","reason":"Imported extraction","origin":"external"}),
                true,
                None,
            )?;
        }
        flow.save()?;
        let brief = flow.store.managed("AGENT_BRIEF.md")?;
        if !brief.exists() {
            crate::data::immutable(
                &brief,
                include_bytes!("../../../../docs/guides/AGENT_BRIEF.md"),
            )?;
        }
        return flow.advance();
    }
    let state = store.load()?;
    ensure!(
        state["version"] == 1 && state["engine"] == "arp4-rust",
        "unsupported workflow state; version 1 and engine arp4-rust required"
    );
    ensure!(
        matches!(
            command,
            WorkflowCommand::Status { .. }
                | WorkflowCommand::History
                | WorkflowCommand::Inspect { .. }
                | WorkflowCommand::Receipt { .. }
                | WorkflowCommand::Next
                | WorkflowCommand::Read { .. }
                | WorkflowCommand::Export
        ) || state["binary_hash"] == hash(&fs::read(&binary)?),
        "CLI binary changed; start a new workflow"
    );
    let mut flow = Workflow {
        store,
        state: WorkflowState::decode(state)?,
        validation_cache: Default::default(),
    };
    // Resolve once: recovery paths and history always use full IDs.
    match &mut command {
        WorkflowCommand::Read { task, .. }
        | WorkflowCommand::Handoff { task }
        | WorkflowCommand::ValidateReply { task, .. }
        | WorkflowCommand::Submit { task, .. }
        | WorkflowCommand::Retry { task, .. }
        | WorkflowCommand::Recover { task } => {
            *task = flow.find(task)?.id.clone();
        }
        _ => {}
    }
    match command {
        WorkflowCommand::Status { summary } => Ok(if summary {
            flow.status_summary()
        } else {
            flow.status()
        }),
        WorkflowCommand::History => Ok(flow.history()),
        WorkflowCommand::RecordUsage {
            task,
            submission,
            usage_file,
        } => {
            flow.record_usage(&task, submission, &read(&usage_file)?)?;
            Ok(flow.history())
        }
        WorkflowCommand::Advance => flow.advance(),
        WorkflowCommand::Next => {
            let mut result = json!({"status":flow.state.status,"next_actions":flow.next_actions()});
            result["next"] = match flow
                .state
                .tasks
                .iter()
                .find(|t| t.state == TaskState::Pending)
            {
                Some(task) => flow.summary(task),
                None => Value::Null,
            };
            Ok(result)
        }
        WorkflowCommand::Read {
            task,
            pointer,
            offset,
            limit,
            max_bytes,
            revision,
        } => flow.read_section(
            flow.find(&task)?,
            &pointer,
            offset,
            limit as usize,
            max_bytes as usize,
            revision.as_deref(),
        ),
        WorkflowCommand::ValidateReply { task, reply, json } => {
            flow.store.preview();
            let task = flow.find(&task)?;
            ensure!(
                task.state == TaskState::Pending || task.state == TaskState::Failed,
                "task already complete or running"
            );
            let reply = flow.complete_reply(
                task,
                interaction::reply_input(reply.as_deref(), json.as_deref())?,
            )?;
            flow.check_draft_identity(task, &reply)?;
            flow.validate(task, &reply)?;
            let mut result = json!({"valid":true,"task_ref":flow.short_id(task),"adopted":false});
            result["quality_check"] = flow.preview_quality(task, &reply)?;
            Ok(result)
        }
        WorkflowCommand::Inspect { task } => flow.inspect_task(&task),
        WorkflowCommand::Handoff { task } => flow.handoff(flow.find(&task)?, &binary),
        WorkflowCommand::Receipt { task } => flow.receipt(&task),
        WorkflowCommand::Submit {
            task,
            reply,
            json,
            actor,
            model,
            reason,
            reason_file,
            origin,
            usage_file,
        } => {
            let reason = reason_input(reason, reason_file, "Externally supplied reply")?;
            ensure!(
                !actor.trim().is_empty() && !reason.trim().is_empty(),
                "actor and reason required"
            );
            let usage = match usage_file {
                Some(path) => read(&path)?,
                None => Value::Null,
            };
            quality::validate_usage(&usage)?;
            // Keep the full ID: advance may retire this task and change short IDs.
            let submitted_id = flow.find(&task)?.id.clone();
            flow.submit(
                &task,
                &interaction::reply_input(reply.as_deref(), json.as_deref())?,
                &json!({"actor":actor,"model":model,"reason":reason,"origin":origin,
                    "usage":usage["usage"],"reported_cost_usd":usage["reported_cost_usd"]}),
            )?;
            let mut result = flow.advance()?;
            result["receipt"] = flow.receipt(&submitted_id)?;
            Ok(result)
        }
        WorkflowCommand::Update {
            input,
            reply,
            regions,
            reference,
        } => {
            flow.update(input, reply, regions, reference)?;
            flow.advance()
        }
        WorkflowCommand::Retry {
            task,
            reason,
            reason_file,
        } => {
            let reason = reason_input(reason, reason_file, "")?;
            let i = flow.index(&task)?;
            ensure!(
                flow.state.tasks[i].state == TaskState::Failed,
                "retry requires failed task"
            );
            flow.state.tasks[i].state = TaskState::Pending;
            flow.state.tasks[i].retry_reason = Some(reason.to_owned());
            flow.state.status = WorkflowStatus::AwaitingReplies;
            flow.state.blocked = json!([]);
            flow.save()?;
            Ok(flow.status())
        }
        WorkflowCommand::Recover { task } => {
            flow.recover(&task)?;
            flow.advance()
        }
        WorkflowCommand::Run(options) => flow.run(&options),
        WorkflowCommand::Export => flow.export(),
        WorkflowCommand::Compact => flow.compact(),
        WorkflowCommand::Init { .. } => unreachable!(),
    }
}
