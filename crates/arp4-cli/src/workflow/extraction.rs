//! Source-preserving extraction partitions. Shared context never counts as disposed text.
use super::*;

pub fn reply_schema() -> Value {
    let mut full = crate::semantic_contract::reply_schema();
    for field in ["requirements", "related"] {
        full["$defs"]["classification"]["properties"][field]["maxItems"] = json!(0);
    }
    let defs = full.as_object_mut().unwrap().remove("$defs").unwrap();
    json!({"$defs":defs,"oneOf":[full,{"type":"object","additionalProperties":false,"required":["packet","document","request_tables","reason"],"properties":{
        "packet":{"type":"string"},"document":{"type":"string"},"request_tables":{"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"integer","minimum":0}},"reason":{"type":"string","minLength":1}}}]})
}

pub fn task_text(packet: &Value) -> String {
    format!(
        "{}\nExtract claims for scope.sources only; scope.context provides context but is not an extraction target. Keys belong to this partition; omit relationships until the linking stage after all partitions are assembled. Context rows are candidates, not confirmed headers. If context is insufficient, return ONLY packet, document, request_tables (indexes into sources.tables), reason to request additional table context. Do not guess or exclude an unexplained claim.\nReply schema: {}\n{}",
        crate::semantic::PROMPT,
        reply_schema(),
        packet
    )
}

pub fn document_task_text(packet: &Value, modules: &Value) -> String {
    format!(
        "{}\n\n{}\n\nProject module vocabulary: {}\nReply schema: {}",
        crate::semantic::PROMPT,
        packet,
        modules,
        crate::semantic_contract::reply_schema()
    )
}

pub fn partitions(
    packet: &Value,
    declared: &[Value],
    max_chars: usize,
    max_sources: usize,
    max_bytes: usize,
    modules: &Value,
) -> Result<Vec<Value>> {
    if !declared.is_empty() {
        return Ok(declared.to_vec());
    }
    let rows = arr(&packet["sources"]["rows"])?;
    let max_sources = if max_sources == 0 {
        usize::MAX
    } else {
        max_sources
    };
    let max_chars = if max_chars == 0 {
        usize::MAX
    } else {
        max_chars
    };
    if rows.len() <= max_sources
        && document_task_text(packet, modules).len() <= max_bytes
        && rows
            .iter()
            .map(|r| r[4].as_str().unwrap_or("").chars().count())
            .sum::<usize>()
            <= max_chars
    {
        return Ok(vec![]);
    }
    let tables = arr(&packet["sources"]["tables"])?;
    let mut sheets: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in rows {
        let table = &tables[row[1].as_u64().context("table index required")? as usize];
        sheets
            .entry(table["sheet"].to_string())
            .or_default()
            .push(row);
    }
    let mut result = Vec::new();
    for sheet in sheets.values() {
        let mut start = 0;
        while start < sheet.len() {
            let mut end = start;
            let mut chars = 0;
            while end < sheet.len() && end - start < max_sources {
                let size = s(&sheet[end][4])?.chars().count();
                if end > start && chars + size > max_chars {
                    break;
                }
                let candidate = region(sheet, start, end + 1);
                let bytes = task_text(&scoped_packet(packet, &candidate, modules)?).len();
                if bytes > max_bytes {
                    ensure!(
                        end > start,
                        "extraction source and context exceed byte limit ({bytes} > {max_bytes}); increase --extract-max-bytes or provide an explicit region plan"
                    );
                    break;
                }
                chars += size;
                end += 1;
            }
            result.push(region(sheet, start, end));
            start = end;
        }
    }
    Ok(result)
}

fn region(sheet: &[&Value], start: usize, end: usize) -> Value {
    let targets: BTreeSet<_> = sheet[start..end].iter().map(|r| r[0].to_string()).collect();
    let context: BTreeSet<_> = sheet
        .iter()
        .take(8)
        .chain(sheet[start.saturating_sub(8)..start].iter())
        .map(|r| r[0].to_string())
        .filter(|r| !targets.contains(r))
        .collect();
    json!({"sources":sheet[start..end].iter().map(|r|r[0].clone()).collect::<Vec<_>>(),
        "context":context.iter().map(|r|serde_json::from_str::<Value>(r).unwrap()).collect::<Vec<_>>()})
}

#[cfg(test)]
mod sizing_tests;

pub fn scoped_packet(packet: &Value, region: &Value, modules: &Value) -> Result<Value> {
    let wanted: BTreeSet<_> = arr(&region["sources"])?
        .iter()
        .chain(arr(&region["context"])?)
        .map(|r| s(r))
        .collect::<Result<_>>()?;
    let mut result = packet.clone();
    result["sources"]["rows"] = json!(
        arr(&packet["sources"]["rows"])?
            .iter()
            .filter(|r| wanted.contains(r[0].as_str().unwrap_or("")))
            .collect::<Vec<_>>()
    );
    result["parent_packet"] = packet["packet"].clone();
    result["scope"] = region.clone();
    result["modules"] = modules.clone();
    result.as_object_mut().unwrap().remove("packet");
    result["packet"] = json!(hash(&encode(&result)));
    Ok(result)
}

fn selector_source(value: &Value) -> Result<&str> {
    s(if value.is_string() {
        value
    } else {
        &value["source"]
    })
}

pub fn validate_scope(reply: &Value, packet: &Value) -> Result<()> {
    let Some(scope) = packet.get("scope") else {
        return Ok(());
    };
    let targets: BTreeSet<_> = arr(&scope["sources"])?
        .iter()
        .map(s)
        .collect::<Result<_>>()?;
    let allowed: BTreeSet<_> = targets
        .iter()
        .copied()
        .chain(
            arr(&scope["context"])?
                .iter()
                .map(s)
                .collect::<Result<Vec<_>>>()?,
        )
        .collect();
    for item in arr(&reply["items"])? {
        for field in ["requirements", "related"] {
            ensure!(
                item["classification"]
                    .get(field)
                    .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty)),
                "partition extraction cannot contain links; complete them in the linking stage"
            );
        }
        let evidence = arr(&item["evidence"])?;
        ensure!(
            evidence
                .iter()
                .any(|e| selector_source(e).is_ok_and(|s| targets.contains(s))),
            "partition item needs evidence from an owned source"
        );
        for e in evidence
            .iter()
            .chain(
                item["condition"]["evidence"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
            .chain(
                ["basis", "unit_basis", "semantics_basis"]
                    .iter()
                    .filter_map(|k| item["value"].get(k)),
            )
        {
            ensure!(
                allowed.contains(selector_source(e)?),
                "partition cites source outside supplied context"
            );
        }
    }
    for exclusion in arr(&reply["exclusions"])? {
        ensure!(
            targets.contains(selector_source(&exclusion["evidence"])?),
            "shared context must not be excluded by this partition"
        );
    }
    Ok(())
}

impl Workflow {
    pub(super) fn expand_extraction(&self, task: &Task, reply: &Value) -> Result<Task> {
        let scoped = self.artifact(&task.packet)?;
        ensure!(
            reply["packet"] == scoped["packet"] && reply["document"] == scoped["document"],
            "stale extraction context request"
        );
        let mut region = scoped
            .get("scope")
            .context("complete document already supplied")?
            .clone();
        let full = self.store.json(s(
            &self.state.packets[task.document.as_deref().context("task document required")?]
        )?)?;
        let tables = arr(&full["sources"]["tables"])?;
        let requested: BTreeSet<usize> = arr(&reply["request_tables"])?
            .iter()
            .map(|v| {
                v.as_u64()
                    .map(|v| v as usize)
                    .context("table index required")
            })
            .collect::<Result<_>>()?;
        ensure!(
            requested.iter().all(|i| *i < tables.len()),
            "unknown extraction context table"
        );
        let targets: BTreeSet<String> = arr(&region["sources"])?
            .iter()
            .map(|v| s(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        let mut context: BTreeSet<String> = arr(&region["context"])?
            .iter()
            .map(|v| s(v).map(str::to_owned))
            .collect::<Result<_>>()?;
        let before = context.len();
        for row in arr(&full["sources"]["rows"])? {
            if requested.contains(&(row[1].as_u64().unwrap() as usize))
                && !targets.contains(s(&row[0])?)
            {
                context.insert(s(&row[0])?.to_owned());
            }
        }
        ensure!(context.len() > before, "requested context already supplied");
        region["context"] = json!(context);
        let packet = scoped_packet(&full, &region, &self.state.modules)?;
        let mut meta = json!({"document":task.document,"packet":self.store.put_json(&packet)?});
        for key in ["partition", "base", "incremental", "context"] {
            if let Some(value) = serde_json::to_value(task)?.get(key) {
                meta[key] = value.clone();
            }
        }
        let text = if task.incremental == Some(true) {
            // Keep the original identity instructions and previous items when adding context.
            format!(
                "{}\nUpdated packet (use this packet and scope): {}",
                String::from_utf8(self.store.get(task.task.as_str())?)?,
                packet
            )
        } else {
            task_text(&packet)
        };
        self.task(Stage::Extract, text.as_bytes(), meta)
    }

    pub(super) fn extraction_candidate(&self, tasks: &[Task]) -> Result<Value> {
        let mut by_doc: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
        for task in tasks {
            by_doc
                .entry(
                    task.document
                        .as_deref()
                        .context("task document required")?
                        .to_owned(),
                )
                .or_default()
                .push(task);
        }
        let mut next = self.state.replies.clone();
        for (doc, tasks) in by_doc {
            if tasks.len() == 1 && tasks[0].incremental == Some(true) {
                let task = tasks[0];
                let merged = incremental::merge(
                    &self.artifact(&task.base)?,
                    &self.artifact(&task.reply)?,
                    &self.artifact(&task.packet)?,
                )?;
                next[&doc] = json!(self.store.put_json(&merged)?);
                continue;
            }
            if tasks.len() == 1 && tasks[0].partition.is_none() {
                next[&doc] = json!(tasks[0].reply);
                continue;
            }
            let packet = self.store.json(s(&self.state.packets[&doc])?)?;
            let mut merged = json!({"packet":packet["packet"],"document":doc,"modules":{},"items":[],"exclusions":[],"open_issues":[]});
            for task in tasks {
                let reply = self.artifact(&task.reply)?;
                let prefix = format!("p{}_", task.partition.context("partition id required")?);
                for (slug, name) in obj(&reply["modules"])? {
                    ensure!(
                        merged["modules"].get(slug).is_none_or(|old| old == name),
                        "module vocabulary conflict: {slug}; update a corrected reply with a consistent name"
                    );
                    merged["modules"][slug] = name.clone();
                }
                for item in arr(&reply["items"])? {
                    let mut item = item.clone();
                    item["key"] = json!(format!("{prefix}{}", s(&item["key"])?));
                    merged["items"].as_array_mut().unwrap().push(item);
                }
                merged["exclusions"]
                    .as_array_mut()
                    .unwrap()
                    .extend(arr(&reply["exclusions"])?.clone());
                merged["open_issues"]
                    .as_array_mut()
                    .unwrap()
                    .extend(arr(&reply["open_issues"])?.clone());
            }
            self.validate_reply(&merged)?;
            next[&doc] = json!(self.store.put_json(&merged)?);
        }
        if tasks.iter().any(|task| task.incremental == Some(true)) {
            self.repair_missing_links(&mut next)?;
        }
        self.validate_replies(&next)?;
        Ok(next)
    }
}
