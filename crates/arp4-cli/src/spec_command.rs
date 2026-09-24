use super::*;

pub(crate) fn execute(command: SpecCommand, output: Output) -> Result<bool> {
    use arp4_cli::specifications as spec;
    match command {
        SpecCommand::Structure { root, command } => {
            let root = dunce::canonicalize(root)?;
            let mut result = arp4_cli::document_structure::execute(&root, command)?;
            result["ok"] = json!(true);
            // Read returns complete cell and image context; never truncate this protocol.
            println!("{}", serde_json::to_string(&result)?);
        }
        SpecCommand::Workflow { paths, command } => {
            let paths = paths.resolve()?;
            let reading = matches!(command, arp4_cli::workflow::WorkflowCommand::Read { .. });
            let publish = matches!(
                command,
                arp4_cli::workflow::WorkflowCommand::Init { .. }
                    | arp4_cli::workflow::WorkflowCommand::Advance
                    | arp4_cli::workflow::WorkflowCommand::Submit { .. }
                    | arp4_cli::workflow::WorkflowCommand::Update { .. }
                    | arp4_cli::workflow::WorkflowCommand::Run(_)
                    | arp4_cli::workflow::WorkflowCommand::Export
            );
            let configured_export = matches!(command, arp4_cli::workflow::WorkflowCommand::Export)
                && paths.output.is_some();
            // Status-shaped results list tasks; the reading protocol returns evidence.
            let status_view = !matches!(
                command,
                arp4_cli::workflow::WorkflowCommand::Read { .. }
                    | arp4_cli::workflow::WorkflowCommand::Inspect { .. }
                    | arp4_cli::workflow::WorkflowCommand::Handoff { .. }
                    | arp4_cli::workflow::WorkflowCommand::Receipt { .. }
                    | arp4_cli::workflow::WorkflowCommand::Next
                    | arp4_cli::workflow::WorkflowCommand::ValidateReply { .. }
            );
            let history = matches!(command, arp4_cli::workflow::WorkflowCommand::History);
            let mut result = if configured_export {
                let out = paths.output.as_ref().unwrap();
                ensure!(
                    arp4_cli::workflow::publish(&paths.root, out)?,
                    "formal export requires complete reviewed generation"
                );
                json!({"state":"exported","output":out})
            } else {
                arp4_cli::workflow::execute(&paths.root, command)?
            };
            if publish
                && !configured_export
                && let Some(out) = &paths.output
                && arp4_cli::workflow::publish(&paths.root, out)?
            {
                result["output"] = json!(out);
            }
            let ok = result["status"] != "blocked";
            if history {
                // One window over both lists; page.total is the longer one.
                let mut longest = json!({"total":0});
                for key in ["submissions", "runs"] {
                    let rows = result[key].as_array().cloned().unwrap_or_default();
                    let page = output.page(rows);
                    result[key] = page["items"].clone();
                    if page["page"]["total"].as_u64() >= longest["total"].as_u64() {
                        longest = page["page"].clone();
                    }
                }
                result["page"] = longest;
                output.emit(result, ok);
            } else if status_view {
                if let Some(tasks) = result.get("tasks").and_then(Value::as_array).cloned() {
                    let page = output.page(tasks);
                    result["tasks"] = page["items"].clone();
                    result["page"] = page["page"].clone();
                }
                output.emit(result, ok);
            } else {
                // Reading material uses the project format; control responses stay JSON.
                result["ok"] = json!(ok);
                if reading {
                    let format: arp4_cli::agent_format::AgentFormat =
                        serde_json::from_value(result["format"].clone())?;
                    println!("{}", format.encode(&result)?);
                } else {
                    println!("{}", serde_json::to_string(&result)?);
                }
            }
            if !ok {
                return Ok(false);
            }
        }
        SpecCommand::Semantic { root, command } => {
            use arp4_cli::semantic;
            let reading_root = match root {
                Some(root) => arp4_cli::project::open(&root)?,
                None => std::env::current_dir()?,
            };
            let format = arp4_cli::project::agent_read_format(&reading_root)?;
            match command {
                SemanticCommand::Changes { before, after, out } => {
                    let plan =
                        semantic::changes(&spec::load_input(&before)?, &spec::load_input(&after)?)?;
                    immutable(&out, &encoded(&plan))?;
                    output.emit(json!({"state":"saved","plan":out,"reextract":plan["reextract"],"removed":plan["removed"]}),true);
                }
                SemanticCommand::Packet {
                    input,
                    document,
                    out,
                } => {
                    let input = spec::load_input(&input)?;
                    let packet = semantic::packet(&input, &document)?;
                    let task = format.task(semantic::PROMPT, &packet)?;
                    immutable(&out, task.as_bytes())?;
                    output.emit(json!({"state":"saved","task":out,"packet":packet["packet"],"sources":packet["sources"]["rows"].as_array().unwrap().len(),"bytes":task.len()}), true);
                }
                SemanticCommand::Assemble {
                    input,
                    reply,
                    actor,
                    out,
                } => {
                    let report =
                        arp4_cli::semantic_operations::assemble(&input, &reply, &actor, &out)?;
                    output.emit(json!({"state":"assembled","out":out,"ready":report.ready,"summary":report.summary,"coverage":report.coverage}),true);
                }
                SemanticCommand::ReviewPacket {
                    source,
                    context_source,
                    sheet,
                    expanded,
                    input,
                    model,
                    catalog,
                    document,
                    out,
                } => {
                    let scope = (!source.is_empty()).then_some(semantic::ReviewScope {
                        sources: source,
                        context: context_source,
                    });
                    let packet = semantic::review_packet_selected(
                        &spec::load_input(&input)?,
                        &spec::load_model(&model)?,
                        &serde_json::from_value(read(&catalog, None)?)?,
                        document.as_deref(),
                        sheet.as_deref(),
                        scope.as_ref(),
                    )?;
                    let task = format.task(
                        semantic::REVIEW_PROMPT,
                        &if expanded {
                            packet.clone()
                        } else {
                            semantic::compact_review(&packet)
                        },
                    )?;
                    immutable(&out, task.as_bytes())?;
                    output.emit(json!({"state":"saved","task":out,"packet":packet["packet"],"bytes":task.len()}),true);
                }
                SemanticCommand::ReviewApply {
                    review_plan,
                    input,
                    model,
                    catalog,
                    review,
                    reviewer,
                    out,
                } => {
                    let report = arp4_cli::semantic_operations::apply_reviews(
                        &input,
                        &model,
                        &catalog,
                        &review,
                        &reviewer,
                        &out,
                        review_plan
                            .map(|p| {
                                Ok::<_, anyhow::Error>(serde_json::from_value(read(&p, None)?)?)
                            })
                            .transpose()?,
                    )?;
                    output.emit(json!({"state":"saved","ready":report.ready,"model":out,"summary":report.summary,"coverage":report.coverage}),true);
                    return Ok(true);
                }
                SemanticCommand::Finalize {
                    input,
                    model,
                    catalog,
                    plan,
                    out,
                    registry_out,
                    project,
                } => {
                    let registry_out = if registry_out {
                        let root = arp4_cli::project::discover(&std::env::current_dir()?)?;
                        Some(under(&root, ".arp/registry")?)
                    } else {
                        None
                    };
                    arp4_cli::semantic_operations::finalize(
                        &input,
                        &model,
                        &catalog,
                        &plan,
                        &out,
                        registry_out.as_deref(),
                        project,
                    )?;
                    output.emit(
                        json!({"state":"assigned","out":out,"registry":registry_out}),
                        true,
                    );
                }
            }
        }
        SpecCommand::Registry { root, command } => {
            let root = match root {
                Some(root) => arp4_cli::project::open(&root)?,
                None => arp4_cli::project::discover(&std::env::current_dir()?)?,
            };
            let canonical = under(&root, ".arp/registry")?;
            use arp4_cli::registry;
            match command {
                RegistryCommand::Init {
                    input,
                    model,
                    catalog,
                    project,
                } => {
                    let ledger = PathBuf::from(format!("{}.ids.json", model.display()));
                    let r = registry::import(
                        spec::load_input(&input)?,
                        spec::load_model(&model)?,
                        serde_json::from_value(read(&ledger, None)?)?,
                        serde_json::from_value(read(&catalog, None)?)?,
                        project,
                    )?;
                    registry::save(&r, &canonical)?;
                    output.emit(json!({"state":"initialized","root":canonical,"summary":registry::summary(&r)?}),true);
                }
                RegistryCommand::Preflight {
                    input,
                    model,
                    catalog,
                    project,
                } => {
                    let input = spec::load_input(&input)?;
                    let ledger_path = PathBuf::from(format!("{}.ids.json", model.display()));
                    let model = spec::load_model(&model)?;
                    let ledger: arp4_cli::specification_ids::Ledger =
                        serde_json::from_value(read(&ledger_path, None)?)?;
                    let catalog: registry::Catalog = serde_json::from_value(read(&catalog, None)?)?;
                    let report = registry::preflight(&input, &model, &ledger, &catalog, &project)?;
                    output.emit(report.clone(), report["ready"] == true);
                }
                RegistryCommand::Apply { change, input } => {
                    let r = registry::apply(
                        registry::load(&canonical)?,
                        serde_json::from_value(read(&change, None)?)?,
                        input
                            .iter()
                            .map(|p| spec::load_input(p))
                            .collect::<Result<Vec<_>>>()?,
                    )?;
                    registry::save(&r, &canonical)?;
                    output.emit(json!({"state":"revised","root":canonical,"summary":registry::summary(&r)?}),true);
                }
                RegistryCommand::Check => {
                    let r = registry::load(&canonical)?;
                    output.emit(
                        json!({"state":"valid","summary":registry::summary(&r)?}),
                        true,
                    );
                }
                RegistryCommand::Render => {
                    let out = under(&root, ".arp/cache/registry")?;
                    registry::render(&registry::load(&canonical)?, &out)?;
                    output.emit(json!({"state":"rendered","out":out}), true);
                }
            }
        }
        SpecCommand::AssignIds {
            input,
            model,
            plan,
            previous,
            out,
        } => {
            use arp4_cli::specification_ids;
            let input = spec::load_input(&input)?;
            let model = spec::load_model(&model)?;
            spec::assess(&input, &model)?;
            let sidecar = |p: &std::path::Path| PathBuf::from(format!("{}.ids.json", p.display()));
            let previous = previous
                .map(|path| -> Result<_> {
                    Ok((
                        spec::load_model(&path)?,
                        serde_json::from_value(read(&sidecar(&path), None)?)?,
                    ))
                })
                .transpose()?;
            let (model, ledger) = specification_ids::assign(
                model,
                serde_json::from_value(read(&plan, None)?)?,
                previous,
            )?;
            spec::assess(&input, &model)?;
            let ledger_path = sidecar(&out);
            let model_bytes = encoded(&serde_json::to_value(&model)?);
            let ledger_bytes = encoded(&serde_json::to_value(&ledger)?);
            for (path, bytes) in [(&out, &model_bytes), (&ledger_path, &ledger_bytes)] {
                ensure!(
                    !path.exists() || fs::read(path)? == *bytes,
                    "output already exists with different content: {}",
                    path.display()
                );
            }
            immutable(&ledger_path, &ledger_bytes)?;
            immutable(&out, &model_bytes)?;
            output.emit(json!({"state":"assigned","model":out,"ledger":ledger_path,"items":model.items.len()}), true);
        }
        SpecCommand::Sources {
            input,
            document,
            out,
        } => {
            immutable(
                &out,
                spec::source_view(&spec::load_input(&input)?, document.as_deref())?.as_bytes(),
            )?;
            output.emit(json!({"state":"saved","sources":out}), true);
        }
        SpecCommand::Schema { out } => {
            immutable(&out, &encoded(&spec::schema()))?;
            output.emit(json!({"state":"saved","schema":out}), true);
        }
        SpecCommand::Capture {
            extraction,
            document,
            root,
            out,
        } => {
            let mut input = spec::capture(&extraction)?;
            if !document.is_empty() {
                let root = dunce::canonicalize(root.context("--root required with --document")?)?;
                let store = arp4_cli::documents::Store::open(&root)?;
                let mut seen = std::collections::BTreeSet::new();
                for id in document {
                    ensure!(seen.insert(id.clone()), "duplicate structure document");
                    let inspected = store.inspect(&store.document(&id)?, true)?;
                    let structure = inspected
                        .interpretation
                        .context("document has no structure interpretation")?;
                    let mut matching = Vec::new();
                    for path in &extraction {
                        let value = read(path, Some("extraction"))?;
                        if value["document_id"] == id {
                            matching.push(value);
                        }
                    }
                    ensure!(
                        matching.len() == 1 && matching[0] == inspected.extraction,
                        "capture extraction differs from reviewed document"
                    );
                    arp4_cli::document_structure::attach(
                        &root,
                        &inspected.extraction,
                        &structure,
                        &mut input,
                    )?;
                }
            }
            spec::validate_structure_requirements(&input)?;
            let bytes = encoded(&serde_json::to_value(&input)?);
            immutable(&out, &bytes)?;
            output.emit(json!({"state":"captured","input":out,"input_hash":hash(&bytes),"sources":input.sources.len(),
                        "warnings":grouped_warnings(input.warnings.iter().map(String::as_str))}), true);
        }
        SpecCommand::Prompt { out } => {
            immutable(&out, spec::PROMPT.as_bytes())?;
            output.emit(json!({"state":"saved","prompt":out}), true);
        }
        SpecCommand::Check { input, model, out } => {
            let report = spec::assess(&spec::load_input(&input)?, &spec::load_model(&model)?)?;
            if let Some(path) = &out {
                immutable(path, &encoded(&serde_json::to_value(&report)?))?;
            }
            let mut result = if out.is_none() {
                output.page(report.issues)
            } else {
                json!({"report":out})
            };
            result["summary"] = json!(report.summary);
            result["coverage"] = report.coverage;
            result["assurance"] = report.assurance;
            result["state"] = json!(if report.ready { "ready" } else { "blocked" });
            output.emit(result, report.ready);
            return Ok(report.ready);
        }
        SpecCommand::Render {
            input,
            model,
            out,
            draft,
        } => {
            let report_path = arp4_cli::semantic_operations::render(&input, &model, &out, draft)?;
            output.emit(
                        json!({"state":if draft {"draft"} else {"rendered"},"output":out,"report":report_path}),
                        true,
                    );
        }
    }
    Ok(true)
}
