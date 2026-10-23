mod response;
mod workspace;

use anyhow::{Context, Result, ensure};
use arp4_cli::{
    data::*,
    document_source::SlidePosition,
    documents::{
        Axis, Batch, EditKind, ImageRequest, Position, ShapeRequest, SheetEdit, SlideEdit,
        SlideEditKind, Store,
    },
};
use clap::Parser;
mod cli;
mod spec_command;
use cli::*;
use response::{Output, omit_hashes};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

struct Lock {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
fn run(cli: Cli) -> Result<bool> {
    let output = cli.output;
    ensure!(
        !output.full
            || !matches!(
                &cli.command,
                Command::Spec {
                    command: SpecCommand::Workflow { .. }
                }
            ),
        "workflow does not support --full; use read --task <task_ref>; continue with --offset <page.next_offset> until null"
    );
    if output.limit.is_some() || output.offset.is_some() {
        ensure!(
            matches!(
                &cli.command,
                Command::Spec {
                    command: SpecCommand::Check { out: None, .. }
                } | Command::Spec {
                    command: SpecCommand::Workflow {
                        command: arp4_cli::workflow::WorkflowCommand::Read { .. }
                            | arp4_cli::workflow::WorkflowCommand::Status { .. }
                            | arp4_cli::workflow::WorkflowCommand::History,
                        ..
                    }
                } | Command::Documents {
                    command: DocumentCommand::Schema { pointer: None, .. }
                        | DocumentCommand::Status { .. }
                        | DocumentCommand::Check { .. }
                        | DocumentCommand::Search { .. }
                        | DocumentCommand::SearchFormats { .. }
                        | DocumentCommand::Operations {
                            command: OperationCommand::List { .. }
                        }
                        | DocumentCommand::Diff { out: None, .. }
                        | DocumentCommand::Export { out: None, .. },
                    ..
                }
            ),
            "--limit/--offset require workflow read/status/history, spec check, schema without --pointer, check, status, search, search-formats, operations list, diff without --out, or export without --out"
        );
    }
    match cli.command {
        Command::Spec { command } => return spec_command::execute(command, output),
        Command::Doctor => {
            let mut report = arp4_cli::capabilities();
            report["cli"] = json!({"response_version":1,"format":"json","max_response_bytes":response::MAX_RESPONSE_BYTES,"default_limit":20,"max_limit":100});
            // The prose limitations duplicate the skill text; capabilities stay.
            if !output.full {
                report.as_object_mut().unwrap().remove("limitations");
            }
            output.emit(report, true);
        }
        Command::Skills {
            command: SkillCommand::Install { root, agent },
        } => {
            let installed = arp4_cli::skills::install(&root, agent)?;
            let mut result = json!({"state":if installed.files == 0 {"skipped"} else {"installed"},"files":installed.files,"root":root});
            if !installed.removed.is_empty() {
                result["removed"] = json!(installed.removed);
            }
            if !installed.kept.is_empty() {
                result["kept_edited"] = json!(installed.kept);
            }
            output.emit(result, true);
        }
        Command::Documents {
            root,
            include_hashes,
            command,
        } => {
            if let DocumentCommand::Schema { kind, pointer } = &command {
                let schemas = arp4_cli::schemas();
                let schema = schemas.get(kind).context("unknown document schema")?;
                let node = match pointer {
                    Some(p) => schema.pointer(p).context("schema pointer not found")?,
                    None => schema,
                };
                let result = if output.full || pointer.is_some() {
                    json!({"kind":kind,"pointer":pointer.as_deref().unwrap_or(""),"schema":node})
                } else {
                    let properties = node["properties"].as_object().map(|props| props.iter().map(|(name, property)| {
                        json!({"name":name,"type":property.get("type"),"required":node["required"].as_array().is_some_and(|a|a.contains(&json!(name)))})
                    }).collect()).unwrap_or_default();
                    let mut page = output.page(properties);
                    page["kind"] = json!(kind);
                    page["type"] = node["type"].clone();
                    page["detail"] = json!(
                        "Use --pointer /properties/<name> for constraints; --full for the complete schema."
                    );
                    page
                };
                output.emit(result, true);
                return Ok(true);
            }
            let root = if let Some(root) = root {
                root
            } else {
                let mut root = std::env::current_dir()?;
                if !matches!(command, DocumentCommand::Init { .. }) {
                    root = arp4_cli::project::discover(&root)?;
                }
                root
            };
            if matches!(command, DocumentCommand::Init { .. }) {
                fs::create_dir_all(&root)?;
            }
            let root = dunce::canonicalize(&root)?;
            let mutate = matches!(
                command,
                DocumentCommand::Init { .. }
                    | DocumentCommand::Import { .. }
                    | DocumentCommand::Remove { .. }
                    | DocumentCommand::Discard { .. }
                    | DocumentCommand::Record { .. }
                    | DocumentCommand::Adopt { .. }
                    | DocumentCommand::Review { .. }
                    | DocumentCommand::Export { out: Some(_), .. }
                    | DocumentCommand::Apply { .. }
                    | DocumentCommand::ConfirmExport { .. }
                    | DocumentCommand::Values { .. }
                    | DocumentCommand::Operations {
                        command: OperationCommand::Remove { .. }
                    }
                    | DocumentCommand::StructureSave { .. }
                    | DocumentCommand::Rows { .. }
                    | DocumentCommand::Columns { .. }
                    | DocumentCommand::Slides { .. }
                    | DocumentCommand::Shapes { .. }
                    | DocumentCommand::Images { .. }
                    | DocumentCommand::SearchRefresh { .. }
            );
            let _lock = if mutate {
                let path = under(&root, ".arp/rust-documents.lock")?;
                fs::create_dir_all(path.parent().unwrap())?;
                let file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .context(
                        "another Rust document operation is running (or stale lock remains)",
                    )?;
                Some(Lock {
                    path,
                    file: Some(file),
                })
            } else {
                None
            };
            if let DocumentCommand::Init { sources } = command {
                Store::init(&root, &sources)?;
                output.emit(json!({"state":"initialized","root":root}), true);
                return Ok(true);
            }
            let store = Store::open(&root)?;
            let result = match command {
                DocumentCommand::Search {
                    query,
                    color,
                    font,
                    size,
                    bold,
                    italic,
                    underline,
                    strike,
                    highlight,
                    fill,
                    document,
                    synonyms,
                    profile,
                    revision,
                } => {
                    let search_started = std::time::Instant::now();
                    let marks: Vec<&str> = [
                        ("bold", bold),
                        ("italic", italic),
                        ("underline", underline),
                        ("strike", strike),
                        ("highlight", highlight),
                        ("fill", fill),
                    ]
                    .into_iter()
                    .filter_map(|(mark, on)| on.then_some(mark))
                    .collect();
                    let format = arp4_cli::documents::search::FormatFilter::new(
                        &color,
                        &font,
                        size.as_deref(),
                        &marks,
                    )?;
                    // A format alone searches as one empty query.
                    let query = if query.is_empty() {
                        vec![String::new()]
                    } else {
                        query
                    };
                    let offset = output.offset.unwrap_or(0);
                    let mut batch = store.search_many(
                        &query.iter().map(String::as_str).collect::<Vec<_>>(),
                        arp4_cli::documents::search::SearchOptions {
                            document: document.as_deref(),
                            synonyms: synonyms.as_deref(),
                            offset,
                            limit: if output.full {
                                None
                            } else {
                                Some(output.limit.unwrap_or(20) as usize)
                            },
                            revision: revision.as_deref(),
                            format,
                        },
                    )?;
                    let response_started = std::time::Instant::now();
                    let pager = Output {
                        full: output.full,
                        limit: output.limit,
                        offset: None,
                    };
                    let first = &batch.results[0];
                    let summary = json!({"indexed_documents":first.indexed_documents,
                        "indexed_at_unix":first.indexed_at_unix,
                        "source_checked_this_search":false});
                    let failed = first.failed.clone();
                    let mut responses = Vec::with_capacity(query.len());
                    let mut tables = response::SearchTables::default();
                    let mut ok = true;
                    for (text, found) in query.iter().zip(batch.results) {
                        let mut items = found.items;
                        for item in &mut items {
                            strip_search_hashes(item);
                            if output.full {
                                round_search_score(item);
                                lift_search_document(item);
                            } else {
                                compact_search_item(item);
                                lift_search_document(item);
                                *item = response::fit_search_item(item.take());
                            }
                        }
                        let mut result = pager.page_search(items, &mut tables);
                        let returned = result["page"]["returned"].as_u64().unwrap() as usize;
                        result["page"]["total"] = json!(found.total);
                        result["page"]["offset"] = json!(offset);
                        result["page"]["next_offset"] =
                            if offset.saturating_add(returned) < found.total {
                                json!(offset + returned)
                            } else {
                                Value::Null
                            };
                        result["revision"] = json!(found.revision);
                        if output.full {
                            result["query_terms"] = json!(found.query_terms);
                        }
                        let query_ok = found.failed.is_empty();
                        ok &= query_ok;
                        if query.len() > 1 {
                            result["query"] = json!(text);
                            result["ok"] = json!(query_ok);
                        } else {
                            result["summary"] = summary.clone();
                            result["failed"] = json!(failed);
                            result["scope"] = json!("adopted_extraction");
                            if !query_ok {
                                result["error"] = json!({"code":"partial_search","message":"Invalid documents were excluded; see failed."});
                            }
                        }
                        responses.push(result);
                    }
                    let result = if query.len() == 1 {
                        let mut result = responses.pop().unwrap();
                        result["tables"] = tables.into_value();
                        result
                    } else {
                        let mut result = json!({"queries": responses, "tables": tables.into_value(),
                            "revision": batch.revision, "scope": "adopted_extraction",
                            "summary":summary, "failed":failed});
                        if !ok {
                            result["error"] = json!({"code":"partial_search","message":"Invalid documents were excluded; see failed."});
                        }
                        result
                    };
                    if query.len() > 1 {
                        match output.fit_search_batch(result) {
                            Ok(result) => output.emit(result, ok),
                            Err(error) => {
                                output.emit(error, false);
                                ok = false;
                            }
                        }
                    } else {
                        output.emit(result, ok);
                    }
                    batch.timings.response += response_started.elapsed();
                    if profile {
                        eprintln!("{}", batch.timings.report(search_started.elapsed()));
                    }
                    return Ok(ok);
                }
                DocumentCommand::SearchFormats { document } => {
                    let inventory = store.search_formats(document.as_deref())?;
                    let ok = inventory.failed.is_empty();
                    let mut result = output.page(inventory.items);
                    result["summary"] = json!({"indexed_documents":inventory.indexed_documents,
                        "indexed_at_unix":inventory.indexed_at_unix,
                        "source_checked_this_search":false});
                    result["failed"] = json!(inventory.failed);
                    result["scope"] = json!("adopted_extraction");
                    if !ok {
                        result["error"] = json!({"code":"partial_search","message":"Invalid documents were excluded; see failed."});
                    }
                    output.emit(result, ok);
                    return Ok(ok);
                }
                DocumentCommand::SearchRefresh { rebuild } => {
                    let refreshed = store.refresh_search_index(rebuild)?;
                    let ok = refreshed.failed.is_empty();
                    output.emit(json!({
                        "state":"indexed",
                        "indexed_documents":refreshed.indexed_documents,
                        "refreshed_documents":refreshed.refreshed_documents,
                        "failed_documents":refreshed.failed.len(),
                        "indexed_at_unix":refreshed.indexed_at_unix,
                        "failed":refreshed.failed,
                        "detail":"Search reads this index until the next search-refresh. External file changes are not checked during search."
                    }), ok);
                    return Ok(ok);
                }
                DocumentCommand::Apply { document } => {
                    let mut result = store.apply(&document)?;
                    if !result["proposal"].is_null() {
                        result["proposal"] = relative(&root, &result["proposal"])?;
                    }
                    if !output.full {
                        count_carried_cells(&mut result["structure"]);
                        summarize_export(&mut result["report"], include_hashes)?;
                        for key in ["written", "document_id", "schema_version"] {
                            result["report"].as_object_mut().unwrap().remove(key);
                        }
                    }
                    result
                }
                DocumentCommand::ConfirmExport {
                    document,
                    candidate,
                    output_sha256,
                    reviewer,
                    reason,
                    layout_reviewed,
                } => store.confirm_export(
                    &document,
                    &candidate,
                    &output_sha256,
                    &reviewer,
                    &reason,
                    layout_reviewed,
                )?,
                DocumentCommand::Values {
                    document,
                    proposal,
                    input,
                    dry_run,
                } => {
                    let (id, proposal) = edited_document(&document, &proposal);
                    let request = request_file(&input)?;
                    let result = store.edit_values(id, proposal, &request, dry_run)?;
                    edit_report(&store, id, proposal, result, include_hashes)?
                }
                DocumentCommand::Operations { command } => match command {
                    OperationCommand::List { document, proposal } => {
                        let (id, proposal) = edited_document(&document, &proposal);
                        let operations = store.operations(id, proposal)?;
                        let mut result = output.page(operations);
                        result["document_id"] = json!(id);
                        if proposal {
                            result["proposal"] = json!(true);
                        }
                        result
                    }
                    OperationCommand::Remove {
                        document,
                        proposal,
                        id,
                        base,
                        dry_run,
                    } => {
                        let (document, proposal) = edited_document(&document, &proposal);
                        let result = store.remove_operation(
                            document,
                            proposal,
                            &id,
                            base.as_deref(),
                            dry_run,
                        )?;
                        edit_report(&store, document, proposal, result, include_hashes)?
                    }
                },
                DocumentCommand::Render {
                    document,
                    candidate,
                    out,
                    sheet,
                    range,
                    page,
                    timeout,
                } => {
                    let mut result = store.render_candidate(
                        &document,
                        &candidate,
                        &out,
                        arp4_cli::documents::RenderPlace {
                            sheet: sheet.as_deref(),
                            range: range.as_deref(),
                            page,
                        },
                        timeout,
                    )?;
                    omit_hashes(&mut result, &["output_sha256"], include_hashes);
                    result
                }
                DocumentCommand::Import {
                    source,
                    force,
                    password_stdin,
                    adopt,
                } => {
                    let unprotection = arp4_cli::documents::Unprotection::new(if password_stdin {
                        passwords(std::io::stdin().lock())?
                    } else {
                        vec![]
                    });
                    let (path, _) = store.resolve(&source)?;
                    if path.is_dir() {
                        let mut result = store.import_folder(&source, force, &unprotection)?;
                        let mut proposals = !array(&result["imported"])?.is_empty();
                        if adopt {
                            let mut adopted = vec![];
                            let mut not_adopted = vec![];
                            let mut failed = array(&result["failed"])?.clone();
                            for id in array(&result["imported"])?.clone() {
                                let id = string(&id)?;
                                match store.adopt_import(id) {
                                    Ok(taken) if taken["state"] == "imported" => {
                                        adopted.push(json!(id))
                                    }
                                    Ok(taken) if taken["state"] == "needs_record" => {
                                        not_adopted.push(json!({"document_id":id,"reason":taken["not_adopted"]}));
                                    }
                                    // The proposal held what the document holds and is gone.
                                    Ok(_) => {}
                                    Err(error) => failed.push(json!({"document_id":id,"reason":format!("imported but not adopted: {error:#}")})),
                                }
                            }
                            proposals = !not_adopted.is_empty();
                            result["adopted"] = json!(adopted);
                            result["not_adopted"] = json!(not_adopted);
                            result["failed"] = json!(failed);
                        }
                        result["state"] = json!(if proposals {
                            "needs_record"
                        } else if adopt && !array(&result["adopted"])?.is_empty() {
                            "imported"
                        } else {
                            "unchanged"
                        });
                        if !array(&result["failed"])?.is_empty() {
                            result["error"] = json!({"code":"operation_failed","message":"Some originals could not be imported or adopted; see failed. The other originals were."});
                            output.emit(result, false);
                            return Ok(false);
                        }
                        result
                    } else {
                        let mut result = store.import_with(&source, force, &unprotection)?;
                        if result["state"] != "unchanged" {
                            result["state"] = json!("needs_record");
                            if adopt {
                                let id = string(&result["document_id"])?.to_owned();
                                let taken = store.adopt_import(&id).with_context(|| {
                                    format!(
                                        "{id} was imported but not adopted; its proposal remains"
                                    )
                                })?;
                                match taken["state"].as_str() {
                                    Some("needs_record") => {
                                        result["not_adopted"] = taken["not_adopted"].clone();
                                    }
                                    state => {
                                        result.as_object_mut().unwrap().remove("proposal");
                                        result["document"] = relative(&root, &taken["document"])?;
                                        result["state"] = json!(state);
                                    }
                                }
                            }
                            if result.get("proposal").is_some() {
                                result["proposal"] = relative(&root, &result["proposal"])?;
                            }
                            if !output.full {
                                count_carried_cells(&mut result["structure"]);
                            }
                        }
                        result
                    }
                }
                DocumentCommand::Remove { document } => {
                    let mut result = store.remove(&document)?;
                    result["state"] = json!("removed");
                    result
                }
                DocumentCommand::Discard { document } => store.discard(&document)?,
                DocumentCommand::StructureRead {
                    document,
                    out,
                    proposal,
                } => {
                    ensure!(!out.exists(), "structure output already exists");
                    let (structure, mut report) = store.structure(&document, proposal)?;
                    arp4_cli::data::write_lazy(&out, structure.lazy(None))?;
                    if !output.full {
                        count_carried_cells(&mut report["carried"]);
                    }
                    json!({"state":"saved","structure":out,"document_id":document,"interpretation":report})
                }
                DocumentCommand::ElementsRead {
                    document,
                    out,
                    proposal,
                } => {
                    ensure!(!out.exists(), "elements output already exists");
                    let pages = store.write_elements(&document, proposal, &out)?;
                    json!({"state":"saved","elements":out,"document_id":document,"pages":pages})
                }
                DocumentCommand::ExtractionRead {
                    document,
                    out,
                    proposal,
                } => {
                    ensure!(!out.exists(), "extraction output already exists");
                    store.write_extraction_view(&document, proposal, &out)?;
                    json!({"state":"saved","extraction":out,"document_id":document})
                }
                DocumentCommand::StructureSave {
                    document,
                    input,
                    proposal,
                } => {
                    let structure = read(&input, None)?;
                    let report = store.save_structure(&document, &structure, proposal)?;
                    let managed = under(&root, &format!(".arp/work/structure/{document}.yml"))?;
                    if managed.is_file()
                        && dunce::canonicalize(&input)? == dunce::canonicalize(&managed)?
                    {
                        fs::remove_file(&managed)?;
                        remove_empty_directories(managed.parent().unwrap(), &store.arp);
                    }
                    json!({"state":"saved","document_id":document,"interpretation":report})
                }
                DocumentCommand::Record {
                    targets,
                    model,
                    actor,
                    prompt,
                } => {
                    let Some(proposal) = single(&store, &targets)? else {
                        let batch = Batch::Record {
                            model: &model,
                            actor: &actor,
                            prompt: &prompt,
                        };
                        return run_batch(&store, &output, targets, &batch, include_hashes);
                    };
                    let mut result = store.record(&proposal, &model, &actor, &prompt)?;
                    omit_hashes(&mut result, &["content", "prompt_sha256"], include_hashes);
                    if !output.full {
                        for key in ["schema_version", "actor", "model"] {
                            result.as_object_mut().unwrap().remove(key);
                        }
                    }
                    result["document_id"] = json!(proposal);
                    result["state"] = json!("recorded");
                    result
                }
                DocumentCommand::Adopt { targets } => {
                    let Some(proposal) = single(&store, &targets)? else {
                        let batch = Batch::Adopt;
                        return run_batch(&store, &output, targets, &batch, include_hashes);
                    };
                    let mut result = store.adopt(&proposal)?;
                    result["document"] = relative(&root, &result["document"])?;
                    if result.get("state").is_none() {
                        result["state"] = json!("needs_review");
                    }
                    result
                }
                DocumentCommand::Review { targets, reviewer } => {
                    let Some(document) = single(&store, &targets)? else {
                        let batch = Batch::Review {
                            reviewer: &reviewer,
                        };
                        return run_batch(&store, &output, targets, &batch, include_hashes);
                    };
                    let mut result = store.review(&document, &reviewer)?;
                    omit_hashes(&mut result, &["content", "formation"], include_hashes);
                    if !output.full {
                        for key in ["schema_version", "reviewer"] {
                            result.as_object_mut().unwrap().remove(key);
                        }
                    }
                    result["document_id"] = json!(document);
                    result["state"] = json!("reviewed");
                    result
                }
                DocumentCommand::Export { document, out } => {
                    // A plan shown a page at a time is saved for its later pages.
                    let mut result = if out.is_none() && !output.full {
                        store.export_plan(&document)?
                    } else {
                        store.export(&document, out.as_deref())?
                    };
                    if !output.full {
                        let items = summarize_export(&mut result, include_hashes)?;
                        if out.is_none() {
                            result
                                .as_object_mut()
                                .unwrap()
                                .extend(output.page(items).as_object().unwrap().clone());
                        }
                        // state says whether the file was written.
                        result.as_object_mut().unwrap().remove("written");
                    }
                    result["state"] = json!(if out.is_some() { "written" } else { "planned" });
                    if let Some(path) = out {
                        // Echo the path as given; the caller already resolved it.
                        result["report"] = json!(path.with_extension(format!(
                            "{}.report.json",
                            path.extension().unwrap().to_string_lossy()
                        )));
                        result["output"] = json!(path);
                    }
                    result
                }
                DocumentCommand::Diff {
                    proposal,
                    document,
                    format,
                    out,
                } => {
                    ensure!(
                        out.is_some() || matches!(format, Format::Json),
                        "--format markdown requires --out; stdout is JSON"
                    );
                    // Changes shown a page at a time, without the semantic
                    // changes no page shows, are saved for the later pages.
                    let result = if out.is_none() && !output.full {
                        store.diff_changes(proposal.as_deref(), document.as_deref())?
                    } else {
                        store.diff(proposal.as_deref(), document.as_deref())?
                    };
                    if let Some(path) = out {
                        let rendered = if matches!(format, Format::Json) {
                            serde_json::to_string(&result)?
                        } else {
                            let mut text = String::from("# Document differences\n");
                            for comparison in array(&result["comparisons"])? {
                                text.push_str(&format!("\n## {}\n", string(&comparison["kind"])?));
                                for change in array(&comparison["changes"])? {
                                    text.push_str(&format!(
                                        "\n- {} ({})\n  before: {}\n  after: {}\n",
                                        string(&change["path"])?,
                                        string(&change["kind"])?,
                                        change["before"],
                                        change["after"]
                                    ));
                                    if let Some(review) = change.get("review_required") {
                                        text.push_str(&format!("  review_required: {review}\n  correspondence: {}\n  affected_entries: {}\n", change["correspondence"], change["affected_entries"]));
                                    }
                                }
                            }
                            text
                        };
                        let path = report_path(&store, path)?;
                        ensure!(!path.exists(), "report already exists");
                        immutable(&path, rendered.as_bytes())?;
                        output.emit(json!({"state":"saved","report":path}), true);
                    } else {
                        let result = if output.full {
                            result
                        } else {
                            let mut changes = vec![];
                            let mut summary = json!({});
                            // With one comparison kind, summary already names it.
                            let several = array(&result["comparisons"])?.len() > 1;
                            for comparison in array(&result["comparisons"])? {
                                let kind = string(&comparison["kind"])?;
                                summary[kind] = json!(array(&comparison["changes"])?.len());
                                for change in array(&comparison["changes"])? {
                                    let mut item = change.clone();
                                    if several {
                                        item["comparison"] = json!(kind);
                                    }
                                    changes.push(item);
                                }
                            }
                            let mut page = output.page(changes);
                            page["summary"] = summary;
                            page["authority_changed"] = result["authority_changed"].clone();
                            page
                        };
                        output.emit(result, true);
                    };
                    return Ok(true);
                }
                DocumentCommand::Rows { command } => {
                    let (target, edit) = row_edit(command)?;
                    edit_sheet(&store, target, edit, include_hashes)?
                }
                DocumentCommand::Columns { command } => {
                    let (target, edit) = column_edit(command)?;
                    edit_sheet(&store, target, edit, include_hashes)?
                }
                DocumentCommand::Slides { command } => {
                    edit_slides(&store, command, include_hashes)?
                }
                DocumentCommand::Shapes { command } => {
                    edit_shapes(&store, command, include_hashes)?
                }
                DocumentCommand::Images { command } => add_image(&store, command, include_hashes)?,
                DocumentCommand::Check {
                    document,
                    proposal,
                    require_reviewed,
                } => {
                    let result = store.status(document.as_deref(), proposal.as_deref(), false)?;
                    return emit_status(&output, result, include_hashes, require_reviewed);
                }
                DocumentCommand::Status { document, proposal } => {
                    let result = store.status(document.as_deref(), proposal.as_deref(), false)?;
                    return emit_status(&output, result, include_hashes, false);
                }
                _ => unreachable!(),
            };
            output.emit(result, true);
        }
    }
    Ok(true)
}

/// A table row names its document once, in the table it lists.
fn lift_search_document(item: &mut Value) {
    let object = item.as_object_mut().expect("search item object");
    if !object.contains_key("table") {
        return;
    }
    for key in [
        "document_id",
        "source_path",
        "extraction_path",
        "state",
        "structure_reviewed",
    ] {
        if let Some(value) = object.remove(key) {
            object["table"][key] = value;
        }
    }
}

fn compact_search_item(item: &mut Value) {
    let object = item.as_object_mut().expect("search item object");
    for key in ["score", "match", "extraction_path"] {
        object.remove(key);
    }
    if object.contains_key("table") {
        // The row's values, its row and the table's columns locate each cell;
        // the line span stays for text originals, whose addresses are internal.
        let lines: Vec<u64> = object["sources"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| [&s["position"]["line_start"], &s["position"]["line_end"]])
            .filter_map(Value::as_u64)
            .collect();
        if let (Some(start), Some(end)) = (lines.iter().min(), lines.iter().max()) {
            object.insert("lines".into(), json!([start, end]));
        }
        for key in ["title", "text", "sources"] {
            object.remove(key);
        }
        return;
    }
    if let Some(sources) = object.get_mut("sources").and_then(Value::as_array_mut) {
        for source in sources {
            let source = source.as_object_mut().expect("search source object");
            for key in ["text", "value", "formula"] {
                source.remove(key);
            }
        }
    }
}

fn strip_search_hashes(item: &mut Value) {
    let object = item.as_object_mut().expect("search item object");
    object.remove("extraction_sha256");
    if let Some(state) = object.get_mut("state").and_then(Value::as_object_mut) {
        state.remove("source_sha256");
        state.remove("content_sha256");
    }
}

fn round_search_score(item: &mut Value) {
    if let Some(score) = item["score"].as_f64() {
        item["score"] = json!(
            format!("{score:.2e}")
                .parse::<f64>()
                .expect("finite BM25 score")
        );
    }
}

fn row_edit(command: RowCommand) -> Result<(EditTarget, SheetEdit)> {
    let (target, kind) = match command {
        RowCommand::Insert {
            target,
            place,
            count,
            style_from,
            merges,
            values,
        } => {
            let mut kind = insert_kind(place, count, style_from, values)?;
            if let EditKind::Insert { merges: copy, .. } = &mut kind {
                *copy = merges;
            }
            (target, kind)
        }
        RowCommand::Delete {
            target,
            from,
            count,
        } => (target, EditKind::Delete { from, count }),
    };
    let edit = sheet_edit(&target, Axis::Rows, kind);
    Ok((target, edit))
}

fn column_edit(command: ColumnCommand) -> Result<(EditTarget, SheetEdit)> {
    let (target, kind) = match command {
        ColumnCommand::Move {
            target,
            place,
            from,
            count,
        } => {
            let position = match (place.after, place.before) {
                (Some(s), _) => Position::After(s),
                (_, Some(s)) => Position::Before(s),
                _ => unreachable!("clap requires position"),
            };
            (
                target,
                EditKind::Move {
                    from,
                    count,
                    position,
                },
            )
        }
        ColumnCommand::Insert {
            target,
            place,
            count,
            values,
        } => {
            let kind = insert_kind(place, count, None, values)?;
            (target, kind)
        }
        ColumnCommand::Delete {
            target,
            from,
            count,
        } => (target, EditKind::Delete { from, count }),
    };
    let edit = sheet_edit(&target, Axis::Columns, kind);
    Ok((target, edit))
}

fn insert_kind(
    place: InsertPlace,
    count: Option<u32>,
    style_from: Option<String>,
    values: Option<PathBuf>,
) -> Result<EditKind> {
    let position = match (place.after, place.before) {
        (Some(after), _) => Position::After(after),
        (None, Some(before)) => Position::Before(before),
        (None, None) => unreachable!("clap requires --after or --before"),
    };
    let values = values
        .map(|path| {
            let (text, json) = if path.as_os_str() == "-" {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                (text, false)
            } else {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("read {}", path.display()))?;
                (text, path.extension().is_some_and(|e| e == "json"))
            };
            // Windows PowerShell 5.1 writes UTF-8 files with a byte order mark.
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            parse(text, json).map_err(|error| {
                anyhow::Error::new(Rejection(json!({"code":"invalid_values","message":format!("--values is not valid YAML or JSON: {error:#}")})))
            })
        })
        .transpose()?;
    Ok(EditKind::Insert {
        position,
        count,
        style_from,
        merges: false,
        values,
    })
}

fn sheet_edit(target: &EditTarget, axis: Axis, kind: EditKind) -> SheetEdit {
    SheetEdit {
        axis,
        kind,
        sheet: target.sheet.clone(),
        id: target.id.clone(),
        reason: target.reason.clone(),
        base: target.base.clone(),
        dry_run: target.dry_run,
        vba_unchanged: target.vba_unchanged,
    }
}

/// Runs a row or column edit and reports it with the check state it leaves.
fn edit_sheet(
    store: &Store,
    target: EditTarget,
    edit: SheetEdit,
    include_hashes: bool,
) -> Result<Value> {
    let (id, proposal) = edited_document(&target.document, &target.proposal);
    let result = store.edit_sheet(id, proposal, &edit)?;
    edit_report(store, id, proposal, result, include_hashes)
}

/// Runs a slide edit and reports it with the check state it leaves.
fn edit_slides(store: &Store, command: SlideCommand, include_hashes: bool) -> Result<Value> {
    let (target, kind) = match command {
        SlideCommand::Insert {
            target,
            from,
            place,
        } => {
            let position = match (place.after, place.before) {
                (Some(after), _) => SlidePosition::After(after),
                (None, Some(before)) => SlidePosition::Before(before),
                (None, None) => unreachable!("clap requires --after or --before"),
            };
            (target, SlideEditKind::Insert { from, position })
        }
        SlideCommand::Add {
            target,
            layout,
            place,
        } => {
            let position = match (place.after, place.before) {
                (Some(after), _) => SlidePosition::After(after),
                (None, Some(before)) => SlidePosition::Before(before),
                (None, None) => unreachable!("clap requires --after or --before"),
            };
            (target, SlideEditKind::Add { layout, position })
        }
        SlideCommand::Delete { target, slide } => (target, SlideEditKind::Delete { slide }),
        SlideCommand::Move {
            target,
            slide,
            place,
        } => {
            let position = match (place.after, place.before) {
                (Some(after), _) => SlidePosition::After(after),
                (None, Some(before)) => SlidePosition::Before(before),
                (None, None) => unreachable!("clap requires --after or --before"),
            };
            (target, SlideEditKind::Move { slide, position })
        }
        SlideCommand::Hide { target, slide } => (
            target,
            SlideEditKind::Visibility {
                slide,
                hidden: true,
            },
        ),
        SlideCommand::Show { target, slide } => (
            target,
            SlideEditKind::Visibility {
                slide,
                hidden: false,
            },
        ),
    };
    let edit = SlideEdit {
        kind,
        id: target.id.clone(),
        reason: target.reason.clone(),
        base: target.base.clone(),
        dry_run: target.dry_run,
    };
    let (id, proposal) = edited_document(&target.document, &target.proposal);
    let result = store.edit_slides(id, proposal, &edit)?;
    edit_report(store, id, proposal, result, include_hashes)
}

/// Runs a shape edit and reports it with the check state it leaves.
fn edit_shapes(store: &Store, command: ShapeCommand, include_hashes: bool) -> Result<Value> {
    let properties =
        |place: Option<&ShapePlace>, look: Option<&ShapeLook>, name: Option<&String>| {
            let mut properties = serde_json::Map::new();
            if let Some(place) = place {
                for (key, value) in [
                    ("left", place.left),
                    ("top", place.top),
                    ("width", place.width),
                    ("height", place.height),
                    ("rotation", place.rotation),
                ] {
                    if let Some(value) = value {
                        properties.insert(key.into(), json!(value));
                    }
                }
            }
            if let Some(look) = look {
                if let Some(fill) = &look.fill {
                    properties.insert("fill".into(), json!(fill));
                }
                if let Some(line) = &look.line {
                    properties.insert("line".into(), json!(line));
                }
                if let Some(weight) = look.line_weight {
                    properties.insert("line_weight".into(), json!(weight));
                }
            }
            if let Some(name) = name {
                properties.insert("name".into(), json!(name));
            }
            Value::Object(properties)
        };
    let (target, operation) = match command {
        ShapeCommand::Update {
            target,
            shape,
            place,
            look,
            name,
        } => {
            let properties = properties(Some(&place), Some(&look), name.as_ref());
            (
                target,
                json!({"kind":"update_shape","shape":shape,"properties":properties}),
            )
        }
        ShapeCommand::Add {
            target,
            shape_type,
            name,
            place,
            look,
            text,
        } => {
            let mut operation = json!({"kind":"add_shape","shape_type":shape_type,"name":name,"properties":properties(Some(&place), Some(&look), None)});
            if let Some(text) = text {
                operation["text"] = json!(text);
            }
            (target, operation)
        }
        ShapeCommand::Delete { target, shape } => {
            (target, json!({"kind":"delete_shape","shape":shape}))
        }
        ShapeCommand::AddPicture {
            target,
            asset,
            name,
            description,
            place,
        } => {
            let mut operation = json!({"kind":"add_picture","asset":asset,"name":name,"properties":properties(Some(&place), None, None)});
            if let Some(description) = description {
                operation["description"] = json!(description);
            }
            (target, operation)
        }
        ShapeCommand::ReplacePicture {
            target,
            shape,
            asset,
        } => (
            target,
            json!({"kind":"replace_picture","shape":shape,"asset":asset}),
        ),
        ShapeCommand::AddConnector {
            target,
            connector_type,
            name,
            from,
            from_site,
            to,
            to_site,
            look,
        } => (
            target,
            json!({"kind":"add_connector","connector_type":connector_type,"name":name,
                "begin":{"shape":from,"site":from_site},"end":{"shape":to,"site":to_site},
                "properties":properties(None, Some(&look), None)}),
        ),
    };
    let mut operation = operation;
    operation["slide"] = json!(target.slide);
    let request = ShapeRequest {
        operation,
        id: target.id.clone(),
        reason: target.reason.clone(),
        base: target.base.clone(),
        dry_run: target.dry_run,
    };
    let (id, proposal) = edited_document(&target.document, &target.proposal);
    let result = store.edit_shapes(id, proposal, &request)?;
    edit_report(store, id, proposal, result, include_hashes)
}

fn add_image(store: &Store, command: ImageCommand, include_hashes: bool) -> Result<Value> {
    let ImageCommand::Add {
        document,
        proposal,
        sheet,
        from,
        to,
        asset,
        name,
        id,
        reason,
        base,
        dry_run,
    } = command;
    let request = ImageRequest {
        sheet,
        from,
        to,
        asset,
        name,
        id,
        reason,
        base,
        dry_run,
    };
    let (id, proposal) = edited_document(&document, &proposal);
    let result = store.add_image(id, proposal, &request)?;
    edit_report(store, id, proposal, result, include_hashes)
}

/// The document ID an edit names, and whether it names a proposal.
fn edited_document<'a>(
    document: &'a Option<String>,
    proposal: &'a Option<String>,
) -> (&'a str, bool) {
    match (document, proposal) {
        (_, Some(id)) => (id.as_str(), true),
        (Some(id), None) => (id.as_str(), false),
        (None, None) => unreachable!("clap requires a document or --proposal"),
    }
}

/// An edit's result with the check state it leaves and the next steps.
fn edit_report(
    store: &Store,
    id: &str,
    proposal: bool,
    mut result: Value,
    include_hashes: bool,
) -> Result<Value> {
    result["document_id"] = json!(id);
    if proposal {
        result["proposal"] = json!(true);
    }
    if result["state"] != "planned" {
        let (document, proposal_id) = if proposal {
            (None, Some(id))
        } else {
            (Some(id), None)
        };
        let rows = store.status(document, proposal_id, false)?;
        let row = &array(&rows)?[0];
        let mut check = json!({"state":row["state"]});
        for key in ["blockers", "pending", "error"] {
            if row
                .get(key)
                .is_some_and(|v| v != &json!([]) && v != &json!(0))
            {
                check[key] = row[key].clone();
            }
        }
        result["check"] = check;
        result["next_actions"] = json!(if proposal {
            vec![
                format!("arp4 documents diff {id}"),
                format!(
                    "arp4 documents record {id} --model <model> --actor <actor> --prompt <file>"
                ),
            ]
        } else {
            vec![
                format!("arp4 documents diff --document {id}"),
                format!("arp4 documents review {id} --reviewer <reviewer>"),
            ]
        });
    }
    omit_hashes(&mut result, &["content"], include_hashes);
    Ok(result)
}

/// Project-relative form of a path under the document root, for agent-facing output.
fn relative(root: &std::path::Path, path: &Value) -> Result<Value> {
    let path = std::path::Path::new(string(path)?);
    Ok(json!(
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    ))
}

/// Replaces the cell lists of how an interpretation was carried with their
/// counts; the lists stay in the proposal's mappings.yml.
fn count_carried_cells(carried: &mut Value) {
    let Some(affected) = carried.get_mut("affected").and_then(Value::as_array_mut) else {
        return;
    };
    for item in affected {
        if let Some(length) = item["cells"].as_array().map(Vec::len) {
            item["cells"] = json!(length);
        }
    }
}

/// Replace an export report's detail arrays with counts under `summary` and drop
/// verification hashes unless requested; the full report stays in `.report.json`.
/// Returns the detail entries so a planning export can page through them.
fn summarize_export(report: &mut Value, include_hashes: bool) -> Result<Vec<Value>> {
    omit_hashes(
        report,
        &["content", "source_sha256", "output_sha256"],
        include_hashes,
    );
    let mut counts = json!({});
    let mut items = vec![];
    for category in [
        "changes",
        "formula_changes",
        "font_changes",
        "unreflected",
        "excluded",
        "omissions",
        "operations",
        "value_edits",
        "deleted_cells",
    ] {
        let values = report
            .as_object_mut()
            .context("export report")?
            .remove(category)
            .unwrap_or(json!([]));
        counts[category] = json!(array(&values)?.len());
        for value in array(&values)? {
            items.push(json!({"category":category,"value":value}));
        }
    }
    report["summary"] = counts;
    report.as_object_mut().unwrap().remove("schema_version");
    Ok(items)
}

/// Where a report given as `path` goes: resolved from the working directory,
/// outside document data or in .arp/cache, with its folder created.
fn report_path(store: &Store, path: PathBuf) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute.parent().context("missing output parent")?;
    let mut ancestor = parent;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .context("missing existing output ancestor")?;
    }
    let suffix = parent.strip_prefix(ancestor)?;
    ensure!(
        suffix
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "invalid output path"
    );
    let parent = dunce::canonicalize(ancestor)?.join(suffix);
    ensure!(
        !parent.starts_with(&store.arp) || parent.starts_with(store.arp.join("cache")),
        "report must be outside document data or in .arp/cache"
    );
    fs::create_dir_all(&parent)?;
    Ok(parent.join(absolute.file_name().context("missing output filename")?))
}

/// The one document `targets` names, taken as before, or None for a batch.
fn single(store: &Store, targets: &Targets) -> Result<Option<String>> {
    Ok(match &targets.document {
        Some(id)
            if !targets.all
                && !targets.dry_run
                && targets.expect.is_none()
                && !store.is_folder(id)? =>
        {
            Some(id.clone())
        }
        _ => None,
    })
}

/// Takes `batch` for the documents `targets` names and emits the lists of those
/// taken, skipped and failed. A failure fails the command after the others are taken.
fn run_batch(
    store: &Store,
    output: &Output,
    targets: Targets,
    batch: &Batch,
    include_hashes: bool,
) -> Result<bool> {
    let expect = match &targets.expect {
        Some(path) => Some(read(path, Some("batch-plan"))?),
        None => None,
    };
    let mut result = store.batch(
        batch,
        targets.document.as_deref(),
        expect.as_ref(),
        targets.dry_run,
    )?;
    let count = |result: &Value, key: &str| array(&result[key]).map(Vec::len);
    if targets.dry_run {
        result["summary"] =
            json!({"planned":count(&result, "planned")?,"skipped":count(&result, "skipped")?});
        if let Some(path) = targets.out {
            let path = report_path(store, path)?;
            ensure!(!path.exists(), "plan already exists");
            let plan =
                json!({"schema_version":"1","action":batch.action(),"documents":result["planned"]});
            validate("batch-plan", &plan)?;
            immutable(&path, &encoded(&plan))?;
            result["plan"] = json!(path);
        }
        if !include_hashes {
            let ids: Vec<_> = array(&result["planned"])?
                .iter()
                .map(|document| document["document_id"].clone())
                .collect();
            result["planned"] = json!(ids);
        }
        result["state"] = json!("planned");
        output.emit(result, true);
        return Ok(true);
    }
    let done = count(&result, batch.done())?;
    let failed = count(&result, "failed")?;
    let mut summary = json!({"skipped":count(&result, "skipped")?,"failed":failed});
    summary[batch.done()] = json!(done);
    result["summary"] = summary;
    result["state"] = json!(if done > 0 { batch.state() } else { "unchanged" });
    if failed > 0 {
        result["error"] = json!({"code":"operation_failed","message":format!("Some documents could not be {}; see failed. The others were.", batch.done())});
        output.emit(result, false);
        return Ok(false);
    }
    output.emit(result, true);
    Ok(true)
}

fn emit_status(
    output: &Output,
    result: Value,
    include_hashes: bool,
    require_reviewed: bool,
) -> Result<bool> {
    let mut summary = json!({});
    let mut rows = array(&result)?.clone();
    let mut ok = true;
    for row in &mut rows {
        let state = string(&row["state"])?;
        summary[state] = json!(summary[state].as_u64().unwrap_or(0) + 1);
        if state == "invalid" || (require_reviewed && row["reviewed"] != true) {
            ok = false;
        }
        omit_hashes(row, &["content", "source_sha256"], include_hashes);
        if !output.full && row["state"] != "invalid" {
            let pending = row["pending"].as_u64().unwrap_or(0);
            let blocked = row["state"] == "blocked";
            let fields = row.as_object_mut().unwrap();
            for key in ["directory", "reviewed", "source_current"] {
                fields.remove(key);
            }
            if pending == 0 {
                fields.remove("pending");
            }
            if !blocked {
                fields.remove("blockers");
            }
        }
    }
    let mut result = output.page(rows);
    result["summary"] = summary;
    if !ok {
        result["error"] = json!({"code":"validation_failed","message":"Inspect item states and errors; summary covers all items, including later pages."});
    }
    output.emit(result, ok);
    Ok(ok)
}

/// The request a command reads from the file `path`, or from standard input
/// for `-`: JSON for a .json file, otherwise YAML (of which JSON is a part).
fn request_file(path: &std::path::Path) -> Result<Value> {
    let (text, json) = if path.as_os_str() == "-" {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
        (text, false)
    } else {
        let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        (text, path.extension().is_some_and(|e| e == "json"))
    };
    // Windows PowerShell 5.1 writes UTF-8 files with a byte order mark.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    parse(text, json).map_err(|error| {
        anyhow::Error::new(Rejection(json!({"code":"invalid_request","message":format!("{} is not valid YAML or JSON: {error:#}", path.display())})))
    })
}

/// The passwords read with --password-stdin: one per line, without the line
/// ending, skipping empty lines.
fn passwords(mut input: impl std::io::Read) -> Result<Vec<String>> {
    let mut text = String::new();
    input.read_to_string(&mut text)?;
    let passwords: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect();
    ensure!(!passwords.is_empty(), "--password-stdin read no password");
    Ok(passwords)
}

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

unsafe extern "C" {
    #[cfg(test)]
    fn mi_option_get(option: i32) -> std::ffi::c_long;
    fn mi_option_set(option: i32, value: std::ffi::c_long);
}

/// `mi_option_purge_delay` of mimalloc v3, which the bindings leave out: the
/// milliseconds freed memory is kept before it goes back to the system.
const PURGE_DELAY: i32 = 15;

/// Freed memory goes back to the system after 50 ms rather than mimalloc's
/// 1000: a command reading a large document frees much of what each step
/// made, and kept it while the next step made more, which raised its peak
/// by a sixth to a third for no gain in time. MIMALLOC_PURGE_DELAY, when set,
/// is left to decide.
fn release_freed_memory_sooner() {
    if std::env::var_os("MIMALLOC_PURGE_DELAY").is_none() {
        unsafe { mi_option_set(PURGE_DELAY, 50) };
    }
}

fn main() -> std::process::ExitCode {
    release_freed_memory_sooner();
    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--internal-excel-render")
    {
        let outcome = std::env::args_os()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("Excel render request path missing"))
            .and_then(|path| arp4_cli::excel::render_worker(std::path::Path::new(&path)));
        return match outcome {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Excel render failed: {error:#}");
                std::process::ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--internal-office-unprotect")
    {
        let outcome = std::env::args_os()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("Office unprotect request path missing"))
            .and_then(|path| arp4_cli::rights_management::worker(std::path::Path::new(&path)));
        return match outcome {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error:#}");
                std::process::ExitCode::from(2)
            }
        };
    }
    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--internal-office-pdf")
    {
        let outcome = std::env::args_os()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("Office PDF request path missing"))
            .and_then(|path| arp4_cli::office_pdf::worker(std::path::Path::new(&path)));
        return match outcome {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error:#}");
                std::process::ExitCode::from(2)
            }
        };
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                error.exit();
            }
            // clap's "try --help" hint addresses a terminal user, not an agent.
            let message = error.to_string();
            let message = message
                .split_once("\n\nFor more information, try")
                .map_or(message.as_str(), |(head, _)| head);
            response::error("invalid_arguments", message);
            return std::process::ExitCode::from(2);
        }
    };
    match run(cli) {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::from(2),
        Err(e) => {
            match rejection(&e) {
                Some(payload) => response::reject(payload),
                None => response::error("operation_failed", &format!("{e:#}")),
            }
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The option number names the purge delay: unset, it is mimalloc v3's
    /// 1000 ms, and setting it reads back.
    #[test]
    fn the_purge_delay_option_is_the_one_released_sooner() {
        if std::env::var_os("MIMALLOC_PURGE_DELAY").is_some() {
            return;
        }
        assert_eq!(unsafe { mi_option_get(PURGE_DELAY) }, 1000);
        release_freed_memory_sooner();
        assert_eq!(unsafe { mi_option_get(PURGE_DELAY) }, 50);
    }
}
