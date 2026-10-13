use super::grid::{Axes, Cells, Point};
use super::*;
use std::collections::HashMap;

pub(super) fn empty(extraction: &Value, extraction_hash: &str) -> Value {
    json!({"schema_version":2,
        "source_sha256":extraction["source"]["sha256"],
        "baseline_extraction_hash":extraction_hash,
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

/// A cell of an extraction with the sheet it is on, which holds its font and
/// style.
pub(super) type SourceCell<'a> = (&'a Value, &'a Value);

/// A cell as a correction binds it: with its font, style and cached result in
/// place, and without its address and ID, which follow from where it is, so a
/// cell moved with its row or column is the same; null for a cell the
/// extraction lacks.
fn content(cell: Option<SourceCell<'_>>) -> Result<Value> {
    let Some((sheet, cell)) = cell else {
        return Ok(Value::Null);
    };
    let mut content = crate::extraction::cell(sheet, cell)?;
    if let Some(fields) = content.as_object_mut() {
        fields.remove("address");
        fields.remove("id");
    }
    Ok(content)
}

/// The hash a correction binds a cell with ([`content`]).
pub(super) fn cell_hash(cell: Option<SourceCell<'_>>) -> Result<String> {
    Ok(hash(&encoded(&content(cell)?)))
}

/// A sheet of an extraction: its number, which the IDs the parser gives its
/// cells start with, and its cells by address and by position.
pub(super) struct Sheet<'a> {
    pub(super) number: usize,
    value: &'a Value,
    by_address: HashMap<&'a str, &'a Value>,
    pub(super) cells: Cells,
}

impl<'a> Sheet<'a> {
    pub(super) fn source(&self, address: &str) -> Option<SourceCell<'a>> {
        self.by_address.get(address).map(|cell| (self.value, *cell))
    }
}

/// The sheets of an extraction by name.
pub(super) fn sheets(extraction: &Value) -> Result<HashMap<&str, Sheet<'_>>> {
    array(&extraction["sheets"])?
        .iter()
        .enumerate()
        .map(|(index, sheet)| {
            let cells = array(&sheet["cells"])?;
            let by_address: HashMap<_, _> = cells
                .iter()
                .map(|cell| Ok((string(&cell["address"])?, cell)))
                .collect::<Result<_>>()?;
            ensure!(by_address.len() == cells.len(), "duplicate source cell");
            Ok((
                string(&sheet["name"])?,
                Sheet {
                    number: index + 1,
                    value: sheet,
                    by_address,
                    cells: Cells::of(sheet)?,
                },
            ))
        })
        .collect()
}

/// The address of the cell at `point`.
pub(super) fn address((column, row): Point) -> Result<String> {
    Ok(format!("{}{row}", crate::excel::column_name(column)?))
}

/// What the parser gives each cell of the elements it infers: its ID, role
/// and headers, by sheet and address.
pub(super) struct Defaults {
    cells: HashMap<(String, String), (Value, Value, Value)>,
    /// The address of each of those cells by ID, which headers name.
    addresses: HashMap<String, String>,
}

impl Defaults {
    pub(super) fn of(elements: &[Value]) -> Result<Self> {
        let mut defaults = Self {
            cells: HashMap::new(),
            addresses: HashMap::new(),
        };
        for element in elements {
            let sheet = string(&element["sheet"])?;
            for cell in array(&element["cells"])? {
                let address = string(&cell["address"])?;
                defaults
                    .addresses
                    .insert(string(&cell["id"])?.to_owned(), address.to_owned());
                defaults.cells.insert(
                    (sheet.to_owned(), address.to_owned()),
                    (
                        cell["id"].clone(),
                        cell["role"].clone(),
                        cell["headers"].clone(),
                    ),
                );
            }
        }
        Ok(defaults)
    }

    /// The ID, role and headers the parser gives the cell at `address` of the
    /// sheet `name`, numbered `number`: for a cell of none of its elements, the
    /// ID it would take, no role and no headers.
    pub(super) fn get(&self, number: usize, name: &str, address: &str) -> (Value, Value, Value) {
        self.cells
            .get(&(name.to_owned(), address.to_owned()))
            .cloned()
            .unwrap_or_else(|| {
                (
                    json!(format!("s{number}-{address}")),
                    json!("unassigned"),
                    json!([]),
                )
            })
    }
}

/// The hash a correction binds an element with: of each of its cells at
/// `addresses`, in order, as the extraction holds it ([`content`]) and with the
/// role and headers the parser gives it, a header by its place among the
/// cells. Moving the element with its rows and columns changes none of it.
pub(super) fn element_hash(
    sheet: &Sheet<'_>,
    name: &str,
    defaults: &Defaults,
    addresses: &[String],
) -> Result<String> {
    let places: HashMap<&str, usize> = addresses
        .iter()
        .enumerate()
        .map(|(place, address)| (address.as_str(), place))
        .collect();
    let cells = addresses
        .iter()
        .map(|address| {
            let (_, role, headers) = defaults.get(sheet.number, name, address);
            let headers: Vec<Option<usize>> = array(&headers)?
                .iter()
                .map(|header| {
                    header
                        .as_str()
                        .and_then(|id| defaults.addresses.get(id))
                        .and_then(|address| places.get(address.as_str()))
                        .copied()
                })
                .collect();
            Ok(json!({"cell":content(sheet.source(address))?,"role":role,"headers":headers}))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(hash(&encoded(&json!(cells))))
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

/// The correction of `element`, on the sheet `name`: the element without its
/// cells, the ranges of its cells, and the ID, role and headers of each cell
/// where they differ from what the parser gives it. Cells are named by the
/// identities of their rows and columns in `axes`; `addresses` gives the
/// address of each cell of the structure by sheet and ID, which the context of
/// a parent names.
fn correction(
    element: &Value,
    name: &str,
    sheet: &Sheet<'_>,
    defaults: &Defaults,
    addresses: &HashMap<(&str, &str), &str>,
    axes: &Axes,
) -> Result<Value> {
    let cells = array(&element["cells"])?;
    let mut members = Vec::with_capacity(cells.len());
    let mut points = Vec::with_capacity(cells.len());
    for cell in cells {
        let address = string(&cell["address"])?;
        ensure!(
            sheet.source(address).is_some(),
            "correction references missing source cell"
        );
        points.push(crate::excel::coordinate(address)?);
        members.push(address.to_owned());
    }
    let named = |address: &str| axes.name(name, crate::excel::coordinate(address)?);
    let ranges = sheet
        .cells
        .ranges(&points)
        .into_iter()
        .map(|range| axes.range(name, range))
        .collect::<Result<Vec<_>>>()?;
    let mut changes = vec![];
    for (cell, address) in cells.iter().zip(&members) {
        let (id, role, headers) = defaults.get(sheet.number, name, address);
        let mut change = serde_json::Map::new();
        if cell["id"] != id {
            change.insert("id".into(), cell["id"].clone());
        }
        if cell["role"] != role {
            change.insert("role".into(), cell["role"].clone());
        }
        if cell["headers"] != headers {
            let headers = array(&cell["headers"])?
                .iter()
                .map(|header| {
                    let address = addresses
                        .get(&(name, string(header)?))
                        .context("header must reference a cell in the same element")?;
                    named(address)
                })
                .collect::<Result<Vec<_>>>()?;
            change.insert("headers".into(), json!(headers));
        }
        if !change.is_empty() {
            change.insert("cell".into(), json!(named(address)?));
            changes.push(Value::Object(change));
        }
    }
    let mut stored = json!({"id":element["id"],"kind":element["kind"],"sheet":element["sheet"],
        "reading":element["reading"]});
    if let Some(descriptions) = element.get("descriptions") {
        stored["descriptions"] = descriptions.clone();
    }
    if let Some(parent) = element.get("parent") {
        let context = array(&parent["context"])?
            .iter()
            .map(|id| {
                let address = addresses
                    .get(&(name, string(id)?))
                    .context("table context must reference a cell of its parent")?;
                named(address)
            })
            .collect::<Result<Vec<_>>>()?;
        stored["parent"] = json!({"element":parent["element"],"context":context});
    }
    if !array(&element["evidence"])?.is_empty() {
        stored["evidence"] = element["evidence"].clone();
    }
    let mut correction = json!({"element":stored,"cells":ranges,
        "sha256":element_hash(sheet, name, defaults, &members)?});
    if !changes.is_empty() {
        correction["changes"] = json!(changes);
    }
    Ok(correction)
}

/// The element a `correction` gives on the extraction `sheet`, and whether its
/// cells, and what the parser gives them, are those it was recorded for. Its
/// cells are those of its ranges, with the ID, role and headers the parser
/// gives each and those its changes give; the context of its parent is the
/// cells it names, by `ids`, the IDs changes give cells by sheet and address. None when
/// it names a cell that is not there, or one twice.
pub(super) fn replayed(
    correction: &Value,
    sheet: &Sheet<'_>,
    defaults: &Defaults,
    ids: &HashMap<(String, String), Value>,
    axes: &Axes,
) -> Result<Option<(Value, bool)>> {
    let name = string(&correction["element"]["sheet"])?;
    let mut points = vec![];
    for range in array(&correction["cells"])? {
        let Ok(bounds) = axes.bounds(name, string(range)?) else {
            return Ok(None);
        };
        points.extend(sheet.cells.within(bounds));
    }
    let places: HashMap<Point, usize> = points
        .iter()
        .enumerate()
        .map(|(place, point)| (*point, place))
        .collect();
    if points.is_empty() || places.len() != points.len() {
        return Ok(None);
    }
    let addresses = points
        .iter()
        .map(|point| address(*point))
        .collect::<Result<Vec<_>>>()?;
    let place = |cell: &Value| -> Result<Option<usize>> {
        Ok(axes
            .point(name, string(cell)?)
            .ok()
            .and_then(|point| places.get(&point))
            .copied())
    };
    let mut changes = BTreeMap::new();
    for change in correction["changes"].as_array().into_iter().flatten() {
        let Some(at) = place(&change["cell"])? else {
            return Ok(None);
        };
        changes.insert(at, change);
    }
    let mut cells: Vec<Value> = addresses
        .iter()
        .enumerate()
        .map(|(at, address)| {
            let (mut id, mut role, headers) = defaults.get(sheet.number, name, address);
            if let Some(change) = changes.get(&at) {
                id = change.get("id").cloned().unwrap_or(id);
                role = change.get("role").cloned().unwrap_or(role);
            }
            json!({"id":id,"address":address,"role":role,"headers":headers,
                "text_state":text_state(sheet.source(address).map(|(_, cell)| cell))})
        })
        .collect();
    // The headers a change gives, by the IDs of the cells it names.
    for (at, change) in &changes {
        if let Some(headers) = change.get("headers") {
            let mut ids = vec![];
            for header in array(headers)? {
                let Some(header) = place(header)? else {
                    return Ok(None);
                };
                ids.push(cells[header]["id"].clone());
            }
            cells[*at]["headers"] = json!(ids);
        }
    }
    let matched = element_hash(sheet, name, defaults, &addresses)? == correction["sha256"];
    let mut element = correction["element"].clone();
    element["cells"] = json!(cells);
    if element.get("evidence").is_none() {
        element["evidence"] = json!([]);
    }
    if let Some(context) = element
        .get_mut("parent")
        .and_then(|parent| parent["context"].as_array_mut())
    {
        let named = std::mem::take(context);
        for cell in named {
            if let Ok(point) = axes.point(name, string(&cell)?) {
                let address = address(point)?;
                context.push(
                    ids.get(&(name.to_owned(), address.clone()))
                        .cloned()
                        .unwrap_or_else(|| json!(format!("s{}-{address}", sheet.number))),
                );
            }
        }
    }
    Ok(Some((element, matched)))
}

/// Records what a reader changed of the parser's interpretation: elements
/// whose structure differs, readings of elements the parser got right, cells
/// whose text_state is not the one the parser reads, edited visuals, image
/// regions and the review. Anything else replay infers again. Cells are named
/// by the identities of their rows and columns in `axes`.
pub(super) fn record(
    root: &Path,
    extraction: &Value,
    structure: &Value,
    axes: &Axes,
) -> Result<Value> {
    validate(root, extraction, structure)?;
    let inferred = inference::elements(extraction)?;
    let inferred_structures: Vec<Value> = inferred.iter().map(structure_of).collect();
    let defaults = Defaults::of(&inferred)?;
    let original_visuals = policy::visuals(extraction)?;
    let sheets = sheets(extraction)?;
    let mut addresses = HashMap::new();
    for element in array(&structure["elements"])? {
        let sheet = string(&element["sheet"])?;
        for cell in array(&element["cells"])? {
            addresses.insert((sheet, string(&cell["id"])?), string(&cell["address"])?);
        }
    }
    let mut elements = Vec::new();
    let mut readings = Vec::new();
    let mut cell_states = Vec::new();
    for element in array(&structure["elements"])? {
        let name = string(&element["sheet"])?;
        let sheet = sheets.get(name).context("unknown element sheet")?;
        let named = |address: &str| axes.name(name, crate::excel::coordinate(address)?);
        for cell in array(&element["cells"])? {
            let address = string(&cell["address"])?;
            let source = sheet.source(address);
            if cell["text_state"] != text_state(source.map(|(_, cell)| cell)) {
                cell_states.push(json!({"sheet":name,"cell":named(address)?,
                    "sha256":cell_hash(source)?,"text_state":cell["text_state"]}));
            }
        }
        let own = structure_of(element);
        if let Some(index) = inferred_structures.iter().position(|e| *e == own) {
            if element["reading"] != inferred[index]["reading"] {
                let anchor = string(&element["cells"][0]["address"])?;
                readings.push(json!({"element":element["id"],"sheet":name,
                    "anchor":named(anchor)?,"sha256":cell_hash(sheet.source(anchor))?,
                    "reading":element["reading"]}));
            }
            continue;
        }
        elements.push(correction(
            element, name, sheet, &defaults, &addresses, axes,
        )?);
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
    let journal = json!({"schema_version":2,"source_sha256":structure["source"]["sha256"],
        "baseline_extraction_hash":structure["extraction_hash"],"elements":elements,
        "readings":readings,"cell_states":cell_states,
        "element_order":array(&structure["elements"])?.iter().map(|element| element["id"].clone()).collect::<Vec<_>>(),
        "visuals":visuals,"regions":structure["regions"],"review":structure["review"]});
    validate_journal(&journal)?;
    Ok(journal)
}
