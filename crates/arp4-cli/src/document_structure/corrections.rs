use super::*;

pub(super) fn empty(extraction: &Value) -> Value {
    json!({"schema_version":1,"document":extraction["document_id"],
        "source_path":extraction["source"]["path"],
        "source_sha256":extraction["source"]["sha256"],
        "baseline_extraction_hash":hash(&encoded(extraction)),
        "elements":[],"element_order":[],"visuals":[],"regions":[],
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

pub(super) fn record(root: &Path, extraction: &Value, structure: &Value) -> Result<Value> {
    validate(root, extraction, structure)?;
    let inferred = inference::elements(extraction)?;
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
    for element in array(&structure["elements"])? {
        if inferred.contains(element) {
            continue;
        }
        let sheet = string(&element["sheet"])?;
        let sources: Vec<_> = array(&element["cells"])?
            .iter()
            .map(|cell| {
                let address = string(&cell["address"])?;
                let source = cells
                    .get(&(sheet.to_owned(), address.to_owned()))
                    .context("correction references missing source cell")?;
                Ok(json!({"address":address,"sha256":hash(&encoded(source))}))
            })
            .collect::<Result<_>>()?;
        elements.push(json!({"element":element,"sources":sources}));
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
        "element_order":array(&structure["elements"])?.iter().map(|element| element["id"].clone()).collect::<Vec<_>>(),
        "visuals":visuals,"regions":structure["regions"],"review":structure["review"]});
    validate_journal(&journal)?;
    Ok(journal)
}
