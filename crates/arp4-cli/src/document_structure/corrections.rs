use super::*;

pub(super) fn empty(extraction: &Value) -> Value {
    json!({"schema_version":1,"document":extraction["document_id"],
        "source_path":extraction["source"]["path"],
        "source_sha256":extraction["source"]["sha256"],
        "baseline_extraction_hash":hash(&encoded(extraction)),
        "elements":[],"readings":[],"cell_states":[],"element_order":[],"visuals":[],"regions":[],
        "review":{"status":"pending"}})
}

pub(super) fn journal_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema",
        "$ref":"#/$defs/correction_journal","$defs":schema()["$defs"]})
}

pub(super) fn validate_journal(journal: &Value) -> Result<()> {
    // Compiled once: every document inspection validates its journal.
    static VALIDATOR: std::sync::LazyLock<jsonschema::Validator> = std::sync::LazyLock::new(|| {
        jsonschema::validator_for(&journal_schema()).expect("embedded journal schema compiles")
    });
    let errors: Vec<_> = VALIDATOR
        .iter_errors(journal)
        .map(|e| e.to_string())
        .collect();
    ensure!(
        errors.is_empty(),
        "invalid correction journal: {}",
        errors.join("; ")
    );
    Ok(())
}

/// The text_state the parser reads from a cell's value: `read` when it holds
/// text or a formula, `empty` otherwise, also for a cell the extraction lacks.
pub(super) fn text_state(cell: Option<&Value>) -> &'static str {
    match cell {
        Some(cell)
            if (!cell["value"].is_null() && cell["value"] != "") || cell["formula"].is_string() =>
        {
            "read"
        }
        _ => "empty",
    }
}

/// The hash a correction binds a cell with; a cell the extraction lacks has
/// the hash of null.
pub(super) fn cell_hash(cell: Option<&Value>) -> String {
    hash(&encoded(cell.unwrap_or(&Value::Null)))
}

/// An element's structure: what a correction changes, without the reading
/// and without the text_state of its cells, which are recorded apart.
fn structure_of(element: &Value) -> Value {
    let mut element = element.clone();
    element.as_object_mut().unwrap().remove("reading");
    for cell in element["cells"].as_array_mut().into_iter().flatten() {
        cell.as_object_mut().unwrap().remove("text_state");
    }
    element
}

/// Records what a reader changed of the parser's interpretation: elements
/// whose structure differs, readings of elements the parser got right, cells
/// whose text_state is not the one the parser reads, edited visuals, image
/// regions and the review. Anything else replay infers again.
pub(super) fn record(root: &Path, extraction: &Value, structure: &Value) -> Result<Value> {
    validate(root, extraction, structure)?;
    let inferred = inference::elements(extraction)?;
    let inferred_structures: Vec<Value> = inferred.iter().map(structure_of).collect();
    let original_visuals = policy::visuals(extraction)?;
    let mut cells = BTreeMap::new();
    for sheet in array(&extraction["sheets"])? {
        let name = string(&sheet["name"])?;
        for cell in array(&sheet["cells"])? {
            cells.insert(
                (name.to_owned(), string(&cell["address"])?.to_owned()),
                cell,
            );
        }
    }
    let mut elements = Vec::new();
    let mut readings = Vec::new();
    let mut cell_states = Vec::new();
    for element in array(&structure["elements"])? {
        let sheet = string(&element["sheet"])?;
        let source = |address: &str| cells.get(&(sheet.to_owned(), address.to_owned())).copied();
        for cell in array(&element["cells"])? {
            let address = string(&cell["address"])?;
            if cell["text_state"] != text_state(source(address)) {
                cell_states.push(json!({"sheet":sheet,"address":address,
                    "sha256":cell_hash(source(address)),"text_state":cell["text_state"]}));
            }
        }
        let own = structure_of(element);
        if let Some(index) = inferred_structures.iter().position(|e| *e == own) {
            if element["reading"] != inferred[index]["reading"] {
                let anchor = string(&element["cells"][0]["address"])?;
                readings.push(json!({"element":element["id"],"sheet":sheet,
                    "anchor":{"address":anchor,"sha256":cell_hash(source(anchor))},
                    "reading":element["reading"]}));
            }
            continue;
        }
        let mut corrected = element.clone();
        let mut sources = Vec::new();
        for cell in corrected["cells"].as_array_mut().unwrap() {
            let address = string(&cell["address"])?.to_owned();
            let found = source(&address).context("correction references missing source cell")?;
            cell["text_state"] = json!(text_state(Some(found)));
            sources.push(json!({"address":address,"sha256":cell_hash(Some(found))}));
        }
        elements.push(json!({"element":corrected,"sources":sources}));
    }
    let mut visuals = Vec::new();
    for visual in array(&structure["visuals"])? {
        if original_visuals.contains(visual) {
            continue;
        }
        let sources: Vec<_> = array(&visual["sources"])?
            .iter()
            .map(|source| {
                let pointer = string(source)?;
                let object = extraction
                    .pointer(pointer)
                    .context("missing visual source")?;
                Ok(json!({"pointer":pointer,"sha256":hash(&encoded(object))}))
            })
            .collect::<Result<_>>()?;
        visuals.push(json!({"visual":visual,"sources":sources}));
    }
    let journal = json!({"schema_version":1,"document":structure["document"],
        "source_path":structure["source"]["path"],"source_sha256":structure["source"]["sha256"],
        "baseline_extraction_hash":structure["extraction_hash"],"elements":elements,
        "readings":readings,"cell_states":cell_states,
        "element_order":array(&structure["elements"])?.iter().map(|element| element["id"].clone()).collect::<Vec<_>>(),
        "visuals":visuals,"regions":structure["regions"],"review":structure["review"]});
    validate_journal(&journal)?;
    Ok(journal)
}
