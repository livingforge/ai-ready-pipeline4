mod response;
mod workspace;

use anyhow::{Context, Result, ensure};
use arp4_cli::{data::*, documents::Store};
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
                        | DocumentCommand::Diff { out: None, .. }
                        | DocumentCommand::Export { out: None, .. },
                    ..
                }
            ),
            "--limit/--offset require workflow read/status/history, spec check, schema without --pointer, check, status, diff without --out, or export without --out"
        );
    }
    match cli.command {
        Command::Spec { command } => return spec_command::execute(command, output),
        Command::Doctor { .. } => {
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
            let files = arp4_cli::skills::install(&root, agent)?;
            output.emit(json!({"state":if files == 0 {"skipped"} else {"installed"},"files":files,"root":root}), true);
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
                    | DocumentCommand::Record { .. }
                    | DocumentCommand::Adopt { .. }
                    | DocumentCommand::Review { .. }
                    | DocumentCommand::Export { out: Some(_), .. }
                    | DocumentCommand::Apply { .. }
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
                DocumentCommand::Apply { document } => {
                    let mut result = store.apply(&document)?;
                    result["proposal"] = relative(&root, &result["proposal"])?;
                    if !output.full {
                        summarize_export(&mut result["report"], include_hashes)?;
                        for key in ["written", "document_id", "schema_version"] {
                            result["report"].as_object_mut().unwrap().remove(key);
                        }
                    }
                    result
                }
                DocumentCommand::Import { source, id } => {
                    let mut result = store.import(&source, &id)?;
                    result["proposal"] = relative(&root, &result["proposal"])?;
                    result["state"] = json!("needs_record");
                    result
                }
                DocumentCommand::Record {
                    proposal,
                    model,
                    actor,
                    prompt,
                } => {
                    let mut result = store.record(&proposal, &model, &actor, &prompt)?;
                    omit_hashes(
                        &mut result,
                        &["extraction", "content", "prompt_sha256"],
                        include_hashes,
                    );
                    if !output.full {
                        for key in ["schema_version", "actor", "model"] {
                            result.as_object_mut().unwrap().remove(key);
                        }
                    }
                    result["proposal_id"] = json!(proposal);
                    result["state"] = json!("recorded");
                    result
                }
                DocumentCommand::Adopt { proposal, reviewer } => {
                    let mut result = store.adopt(&proposal, &reviewer)?;
                    result["document"] = relative(&root, &result["document"])?;
                    result["state"] = json!("reviewed");
                    result
                }
                DocumentCommand::Review { document, reviewer } => {
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
                DocumentCommand::Export {
                    document,
                    out,
                    engine,
                } => {
                    let mut result = store.export(&document, out.as_deref(), &engine)?;
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
                    let result = store.diff(proposal.as_deref(), document.as_deref())?;
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
                            !parent.starts_with(&store.arp)
                                || parent.starts_with(store.arp.join("cache")),
                            "report must be outside document data or in .arp/cache"
                        );
                        fs::create_dir_all(&parent)?;
                        let path =
                            parent.join(absolute.file_name().context("missing output filename")?);
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
                DocumentCommand::Check {
                    document,
                    proposal,
                    require_reviewed,
                    ..
                } => {
                    let result = store.status(document.as_deref(), proposal.as_deref(), false)?;
                    return emit_status(&output, result, include_hashes, require_reviewed);
                }
                DocumentCommand::Status {
                    document, proposal, ..
                } => {
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
        "unreflected",
        "excluded",
        "omissions",
        "operations",
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
        omit_hashes(row, &["content"], include_hashes);
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

fn main() -> std::process::ExitCode {
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
