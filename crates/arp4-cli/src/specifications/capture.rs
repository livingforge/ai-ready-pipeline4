use super::*;

pub fn capture(paths: &[PathBuf]) -> Result<Input> {
    let mut input = Input {
        schema_version: 1,
        revisions: BTreeMap::new(),
        sources: vec![],
        warnings: vec![],
        structures: BTreeMap::new(),
        structure_requirements: BTreeMap::new(),
    };
    for path in paths {
        let source_count = input.sources.len();
        let extraction = read(path, Some("extraction"))?;
        let document = string(&extraction["document_id"])?;
        input.structure_requirements.insert(
            document.into(),
            crate::document_structure::requirements(&extraction)?,
        );
        ensure!(
            input
                .revisions
                .insert(document.into(), hash(&encoded(&extraction)))
                .is_none(),
            "duplicate input document: {document}"
        );
        for finding in array(&extraction["findings"])? {
            input.warnings.push(format!("{document}: {finding}"));
        }
        // Capture scalar payloads with JSON pointers, retaining table/row context in locations.
        fn walk(value: &Value, pointer: String, document: &str, sources: &mut Vec<Source>) {
            match value {
                Value::Object(map) => {
                    for (key, value) in map {
                        walk(
                            value,
                            format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                            document,
                            sources,
                        );
                    }
                }
                Value::Array(values) => {
                    for (index, value) in values.iter().enumerate() {
                        walk(value, format!("{pointer}/{index}"), document, sources);
                    }
                }
                Value::Null => {}
                _ => {
                    let text = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    if !text.trim().is_empty() {
                        sources.push(Source {
                            id: hash(&encoded(&json!([document, pointer]))),
                            document: document.into(),
                            location: pointer,
                            text,
                            context: None,
                        });
                    }
                }
            }
        }
        for (si, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
            let mut cells = array(&sheet["cells"])?
                .iter()
                .enumerate()
                .map(|(i, c)| Ok((crate::excel::coordinate(string(&c["address"])?)?, i, c)))
                .collect::<Result<Vec<_>>>()?;
            cells.sort_by_key(|((column, row), _, _)| (*row, *column));
            for ((column, row), ci, cell) in cells {
                for key in ["value", "formula"] {
                    let start = input.sources.len();
                    walk(
                        &cell[key],
                        format!("/sheets/{si}/cells/{ci}/{key}"),
                        document,
                        &mut input.sources,
                    );
                    for source in &mut input.sources[start..] {
                        source.context = Some(CellContext {
                            sheet: string(&sheet["name"])?.into(),
                            cell: string(&cell["address"])?.into(),
                            row,
                            column,
                            merges: array(&sheet["merges"])?
                                .iter()
                                .map(|v| Ok(string(v)?.to_owned()))
                                .collect::<Result<_>>()?,
                            number_format: string(&cell["number_format"])?.into(),
                            position: cell
                                .get("position")
                                .map(|p| serde_json::from_value(p.clone()))
                                .transpose()?,
                        });
                    }
                }
            }
            // Drawing text is native source text, not OCR and not a writable cell.
            // Its JSON pointer retains the object identity without inventing a cell address.
            if let Some(drawings) = sheet["drawings"].as_array() {
                for (di, drawing) in drawings.iter().enumerate() {
                    for key in ["text", "description"] {
                        walk(
                            &drawing[key],
                            format!("/sheets/{si}/drawings/{di}/{key}"),
                            document,
                            &mut input.sources,
                        );
                    }
                }
            }
        }
        for (pi, page) in array(&extraction["pages"])?.iter().enumerate() {
            for key in ["title", "notes"] {
                walk(
                    &page[key],
                    format!("/pages/{pi}/{key}"),
                    document,
                    &mut input.sources,
                );
            }
            for (ci, chunk) in array(&page["chunks"])?.iter().enumerate() {
                for key in ["heading", "text", "rows", "cells"] {
                    walk(
                        &chunk[key],
                        format!("/pages/{pi}/chunks/{ci}/{key}"),
                        document,
                        &mut input.sources,
                    );
                }
            }
        }
        ensure!(
            input.sources.len() > source_count,
            "document has no extractable source text: {document}"
        );
        if !array(&extraction["assets"])?.is_empty() {
            input.warnings.push(format!(
                "{document}: assets require original-document inspection"
            ));
        }
    }
    // Stable document order, retaining sheet/row/column order within each document.
    input.sources.sort_by(|a, b| a.document.cmp(&b.document));
    ensure!(!input.sources.is_empty(), "no extractable source text");
    Ok(input)
}

pub fn load_input(path: &std::path::Path) -> Result<Input> {
    let input: Input = serde_json::from_value(read(path, None)?)?;
    ensure!(
        input.schema_version == 1 && !input.sources.is_empty(),
        "invalid input version or empty sources"
    );
    let mut ids = BTreeSet::new();
    validate_structure_requirements(&input)?;
    for (document, structure) in &input.structures {
        ensure!(
            input.revisions.contains_key(document),
            "unknown structure document"
        );
        crate::document_structure::validate_snapshot(document, structure)?;
    }
    for s in &input.sources {
        ensure!(
            ids.insert(&s.id)
                && input.revisions.contains_key(&s.document)
                && !s.text.trim().is_empty(),
            "invalid or duplicate source: {}",
            s.id
        );
        ensure!(
            s.id == hash(&encoded(&json!([s.document, s.location]))),
            "source identity mismatch"
        );
    }
    Ok(input)
}

pub fn validate_structure_requirements(input: &Input) -> Result<()> {
    ensure!(
        input
            .structure_requirements
            .keys()
            .eq(input.revisions.keys()),
        "structure requirements must account for every captured document; re-capture input"
    );
    let schema = crate::document_structure::schema();
    jsonschema::validator_for(&schema["$defs"]["capture_structure_requirements"])?
        .validate(&serde_json::to_value(&input.structure_requirements)?)
        .map_err(|e| anyhow::anyhow!("invalid structure requirements: {e}"))?;
    for (document, reasons) in &input.structure_requirements {
        if !reasons.is_empty() {
            let structure = input.structures.get(document).with_context(|| format!(
                "document {document} requires reviewed structure ({}); run spec structure init/check/review and capture with --root and --structure", reasons.join(", ")))?;
            crate::document_structure::validate_snapshot(document, structure)?;
        }
    }
    Ok(())
}
pub fn load_model(path: &std::path::Path) -> Result<Model> {
    let value = read(path, None)?;
    ensure!(
        value["schema_version"] == 1,
        "model schema_version 1 required; regenerate using spec prompt/schema"
    );
    Ok(serde_json::from_value(value)?)
}
