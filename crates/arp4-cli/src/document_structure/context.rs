//! Compact parser facts and verified, directly viewable imported assets.
use super::*;

fn point(value: &Value) -> Option<Value> {
    let row = u32::try_from(value["row"].as_u64()?).ok()?.checked_add(1)?;
    let col = u32::try_from(value["col"].as_u64()?).ok()?.checked_add(1)?;
    if row > 1_048_576 || col > 16_384 {
        return None;
    }
    let column = crate::excel::column_name(col).ok()?;
    let mut result = json!({"cell":format!("{column}{row}")});
    let offsets = [
        value["colOff"].as_i64().unwrap_or(0),
        value["rowOff"].as_i64().unwrap_or(0),
    ];
    if offsets != [0, 0] {
        result["offset"] = json!(offsets);
    }
    Some(result)
}

fn position(drawing: &Value) -> Value {
    let anchor = &drawing["anchor"];
    let mut result = json!({});
    for key in ["from", "to"] {
        if let Some(p) = point(&anchor[key]) {
            result[key] = p;
        }
    }
    for key in ["pos", "ext"] {
        if anchor[key].is_object() {
            result[key] = anchor[key].clone();
        }
    }
    // Needed to interpret rotation and coordinates within groups. Do not flatten
    // nested group coordinates into an invented sheet bounding box.
    if let Some(transform) = drawing["transform"].as_object() {
        match placed(transform) {
            Some(members) => {
                for (name, value) in members {
                    result[name] = value;
                }
            }
            // A transform that is not read is told as the original has it.
            None => result["transform"] = drawing["transform"].clone(),
        }
    }
    result
}

/// A length of DrawingML, in EMU, as points: a whole number where it is one.
fn points(value: &Value) -> Option<Value> {
    let emu: f64 = match value {
        Value::String(text) => text.parse().ok()?,
        other => other.as_f64()?,
    };
    let points = (emu / 12_700.0 * 100.0).round() / 100.0;
    Some(if points.fract() == 0.0 && points.abs() < 9e15 {
        json!(points as i64)
    } else {
        json!(points)
    })
}

/// What a DrawingML transform tells, in points: `box`, the left, top, width
/// and height in the coordinates of the group the drawing is in (the sheet's
/// outside one); `child_box`, those a group gives its members; `rot`, the
/// turn in degrees clockwise; `flip`, h, v or hv. None when a member is not
/// one this reads.
fn placed(transform: &serde_json::Map<String, Value>) -> Option<Vec<(&'static str, Value)>> {
    let rect = |offset: &str, extent: &str| -> Option<Option<Value>> {
        match (transform.get(offset), transform.get(extent)) {
            (None, None) => Some(None),
            (Some(offset), Some(extent)) => Some(Some(json!([
                points(&offset["x"])?,
                points(&offset["y"])?,
                points(&extent["cx"])?,
                points(&extent["cy"])?
            ]))),
            _ => None,
        }
    };
    let mut members = vec![];
    if let Some(rect) = rect("off", "ext")? {
        members.push(("box", rect));
    }
    if let Some(rect) = rect("chOff", "chExt")? {
        members.push(("child_box", rect));
    }
    let set = |name: &str| {
        transform
            .get(name)
            .is_some_and(|flag| flag == "1" || flag == "true" || flag == true)
    };
    let flip: String = [("flipH", "h"), ("flipV", "v")]
        .iter()
        .filter(|(name, _)| set(name))
        .map(|(_, letter)| *letter)
        .collect();
    if !flip.is_empty() {
        members.push(("flip", json!(flip)));
    }
    if let Some(rot) = transform.get("rot") {
        let turn: f64 = match rot {
            Value::String(text) => text.parse().ok()?,
            other => other.as_f64()?,
        };
        let degrees = (turn / 60_000.0 * 100.0).round() / 100.0;
        if degrees != 0.0 {
            members.push(("rot", json!(degrees)));
        }
    }
    let read = ["off", "ext", "chOff", "chExt", "flipH", "flipV", "rot"];
    transform
        .keys()
        .all(|name| read.contains(&name.as_str()))
        .then_some(members)
}

pub(super) fn assets(
    extraction_path: &Path,
    ext: &Extraction,
) -> Result<BTreeMap<String, (PathBuf, String)>> {
    let extraction_path = crate::data::canonicalize(extraction_path)?;
    let parent = extraction_path
        .parent()
        .context("extraction directory missing")?;
    let mut assets = BTreeMap::new();
    for asset in array(&ext["assets"])? {
        let name = string(&asset["path"])?;
        let path = under(parent, &format!("assets/{name}"))?;
        ensure!(
            hash(&fs::read(&path)?) == asset["sha256"],
            "imported image hash mismatch: {name}"
        );
        ensure!(
            assets
                .insert(
                    name.to_owned(),
                    (path, string(&asset["sha256"])?.to_owned())
                )
                .is_none(),
            "duplicate imported asset"
        );
    }
    Ok(assets)
}

/// The size of a read past which it lists the sheets for reading one at a
/// time, unless all of it is asked for.
const WHOLE_READ_BYTES: usize = 48_000;

/// What read returns of the interpretation `structure` of `ext` into
/// `report`: each sheet once, with its elements holding their cells as the
/// original has them and the interpretation reads them, and its drawings
/// and visuals. `only` names the sheets to return; without it a document
/// too large for one response, unless `whole`, lists its sheets by size.
#[allow(clippy::too_many_arguments)]
pub(super) fn read(
    root: &Path,
    extraction_path: &Path,
    ext: &Extraction,
    mut structure: Value,
    report: &mut Value,
    only: &[String],
    range: Option<&str>,
    whole: bool,
) -> Result<()> {
    let within = match range {
        Some(range) => {
            ensure!(
                only.len() == 1,
                "--range reads cells of one sheet: name it with --sheet"
            );
            let (first, last) = range.split_once(':').unwrap_or((range, range));
            let (first, last) = (
                crate::excel::coordinate(first.trim())?,
                crate::excel::coordinate(last.trim())?,
            );
            ensure!(
                first.0 <= last.0 && first.1 <= last.1,
                "--range is given from its top left cell to its bottom right, as A1:H30"
            );
            Some((first, last))
        }
        None => None,
    };
    let inside = |address: &Value| -> bool {
        let Some((first, last)) = within else {
            return true;
        };
        address
            .as_str()
            .and_then(|address| crate::excel::coordinate(address).ok())
            .is_some_and(|(column, row)| {
                first.0 <= column && column <= last.0 && first.1 <= row && row <= last.1
            })
    };
    let assets = assets(extraction_path, ext)?;
    let image_ids: BTreeMap<_, _> = array(&ext["assets"])?
        .iter()
        .map(|asset| {
            let name = string(&asset["path"])?;
            let id = Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .context("image asset name missing")?;
            Ok((name, id.to_owned()))
        })
        .collect::<Result<_>>()?;
    for name in only {
        ensure!(
            ext.sheets
                .iter()
                .any(|sheet| sheet["name"] == name.as_str()),
            "the document has no sheet {name}; its sheets are {}",
            ext.sheets
                .iter()
                .filter_map(|sheet| sheet["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut pictured = BTreeSet::new();
    let mut images = array(&structure["regions"] )?.iter().map(|r| {
        Ok(json!({"id":format!("region-{}",string(&r["id"])?),"region":r["id"],"path":image_path(root,string(&r["image"])?)?,"bbox":r["bbox"]}))
    }).collect::<Result<Vec<_>>>()?;
    // The sheets as extraction-read writes them: a sheet gives the font and
    // style most of its cells have once, and a cell what is its own.
    let mut sheets = match crate::extraction::reading(ext)?.get_mut("sheets") {
        Some(Value::Array(sheets)) => std::mem::take(sheets),
        _ => vec![],
    };
    let mut elements = match structure["elements"].take() {
        Value::Array(elements) => elements,
        _ => vec![],
    };
    for element in &mut elements {
        super::edit::compact_element(ext, element);
    }
    let mut visuals = match structure["visuals"].take() {
        Value::Array(visuals) => visuals,
        _ => vec![],
    };
    for (si, (sheet, view)) in ext.sheets.iter().zip(&mut sheets).enumerate() {
        let name = sheet["name"].clone();
        let view = view.as_object_mut().context("sheet of the reading form")?;
        view.remove("drawings");
        // The part of the package is not a place in the document.
        view.remove("part");
        // Each original cell by its address, taken by the element owning it.
        let mut originals: HashMap<String, Value> = match view.remove("cells") {
            Some(Value::Array(cells)) => cells
                .into_iter()
                .filter_map(|mut cell| {
                    let address = cell.as_object_mut()?.remove("address")?;
                    Some((address.as_str()?.to_owned(), cell))
                })
                .collect(),
            _ => HashMap::default(),
        };
        let mut owned = vec![];
        for mut element in elements.extract_if(.., |element| element["sheet"] == name) {
            for cell in element["cells"].as_array_mut().into_iter().flatten() {
                let Some(original) = cell["address"]
                    .as_str()
                    .and_then(|address| originals.remove(address))
                else {
                    continue;
                };
                let (Some(cell), Value::Object(original)) = (cell.as_object_mut(), original) else {
                    continue;
                };
                for (member, value) in original {
                    // The interpretation's ID of the cell is `id`.
                    let member = if member == "id" {
                        "source_id".to_owned()
                    } else {
                        member
                    };
                    cell.insert(member, value);
                }
            }
            owned.push(element);
        }
        view.insert("elements".into(), Value::Array(owned));
        let mut drawings = vec![];
        // The cells each group is anchored to, which its members repeat.
        let mut anchors: HashMap<String, (Value, Value)> = HashMap::default();
        for (di, drawing) in sheet["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let source = format!("/sheets/{si}/drawings/{di}");
            let mut compact = json!({"source":source,"id":drawing["id"],"kind":drawing["kind"]});
            for (output, input) in [
                ("text", "text"),
                ("shape", "geometry"),
                ("group", "group"),
                ("alt", "description"),
                ("macro", "macro"),
            ] {
                if drawing[input].as_str().is_some_and(|s| !s.is_empty()) {
                    compact[output] = drawing[input].clone();
                }
            }
            if drawing["control"].is_object() {
                compact["control"] = drawing["control"].clone();
            }
            // The arrowheads and the dash, which give a line its direction.
            for key in ["head_end", "tail_end", "dash"] {
                if let Some(mark) = drawing.get(key) {
                    compact[key] = mark.clone();
                }
            }
            let mut position = position(drawing);
            // Outside a group the cells place and size a drawing: its box
            // says the same in points.
            if drawing["kind"] != "group"
                && drawing["group"].as_str().is_none_or(str::is_empty)
                && position.get("from").is_some()
                && position.get("to").is_some()
                && let Some(position) = position.as_object_mut()
            {
                position.remove("box");
            }
            if drawing["kind"] == "group"
                && let Some(id) = drawing["id"].as_str()
            {
                anchors.insert(
                    id.to_owned(),
                    (position["from"].clone(), position["to"].clone()),
                );
            }
            // A member of a group is anchored where its group is, and is
            // placed within it by its transform.
            if let Some((from, to)) = drawing["group"].as_str().and_then(|g| anchors.get(g))
                && position["from"] == *from
                && position["to"] == *to
                && let Some(position) = position.as_object_mut()
            {
                position.remove("from");
                position.remove("to");
            }
            if !position.as_object().unwrap().is_empty() {
                compact["position"] = position;
            }
            if drawing["connections"]
                .as_array()
                .is_some_and(|c| !c.is_empty())
            {
                compact["connections"] = json!(
                    array(&drawing["connections"])?
                        .iter()
                        .map(|c| json!({"end":c["end"],"target":c["target"]}))
                        .collect::<Vec<_>>()
                );
            }
            if let Some(name) = drawing["image"]["asset"].as_str() {
                let (path, sha) = assets
                    .get(name)
                    .context("drawing image missing from assets")?;
                ensure!(
                    drawing["image"]["sha256"] == *sha,
                    "drawing image hash mismatch"
                );
                compact["image_id"] = json!(image_ids[name]);
                images.push(json!({"id":image_ids[name],"source":source,"path":path}));
                pictured.insert(name);
            }
            drawings.push(compact);
        }
        if !drawings.is_empty() {
            view.insert("drawings".into(), Value::Array(drawings));
        }
        let shown: Vec<Value> = visuals
            .extract_if(.., |visual| visual["sheet"] == name)
            .collect();
        if !shown.is_empty() {
            view.insert("visuals".into(), Value::Array(shown));
        }
    }
    for (i, asset) in array(&ext["assets"])?.iter().enumerate() {
        if pictured.contains(string(&asset["path"])?) {
            continue;
        }
        let name = string(&asset["path"])?;
        let (path, _) = &assets[name];
        images.push(json!({"id":image_ids[name],"source":format!("/assets/{i}"),"path":path}));
    }
    for member in ["document", "source", "extraction_hash", "review", "regions"] {
        report[member] = structure[member].take();
    }
    // The visuals on no sheet, as those of a Word document.
    if !visuals.is_empty() {
        report["visuals"] = Value::Array(visuals);
    }
    let size = |value: &Value| serde_json::to_string(value).map_or(0, |text| text.len());
    if !only.is_empty() && !whole {
        for sheet in &mut sheets {
            if !only.iter().any(|name| sheet["name"] == name.as_str()) {
                continue;
            }
            if within.is_some() {
                in_range(sheet, &inside);
            } else if only.len() == 1 && size(sheet) > WHOLE_READ_BYTES {
                // One sheet too large for a response: its elements by where
                // they lie, to read a range at a time.
                list_elements(sheet, &size);
                report["elements_listed"] = json!(true);
                report["detail"] = json!(
                    "The sheet is too large for one response: elements lists what each holds and the cells it spans. Read a range at a time with read --sheet <name> --range <A1:H200>, which returns the cells in it, and change cells with elements.patch; --full returns the whole sheet."
                );
            }
        }
    }
    if !only.is_empty() {
        // The images of the sheets returned, and those on no sheet.
        let kept: Vec<String> = ext
            .sheets
            .iter()
            .enumerate()
            .filter(|(_, sheet)| only.iter().any(|name| sheet["name"] == name.as_str()))
            .map(|(index, _)| format!("/sheets/{index}/"))
            .collect();
        images.retain(|image| {
            image["source"].as_str().is_none_or(|source| {
                !source.starts_with("/sheets/")
                    || kept.iter().any(|sheet| source.starts_with(sheet))
            })
        });
        sheets.retain(|sheet| only.iter().any(|name| sheet["name"] == name.as_str()));
    } else if !whole && sheets.iter().map(size).sum::<usize>() > WHOLE_READ_BYTES {
        // Too large for one response: the sheets by what each holds.
        for sheet in &mut sheets {
            let count = |member: &str| sheet[member].as_array().map_or(0, Vec::len);
            let cells: usize = sheet["elements"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|element| element["cells"].as_array().map_or(0, Vec::len))
                .sum();
            *sheet = json!({"name":sheet["name"],"bytes":size(sheet),"elements":count("elements"),
                "cells":cells,"drawings":count("drawings"),"visuals":count("visuals")});
        }
        report["sheets_listed"] = json!(true);
        report["detail"] = json!(
            "The document is too large for one response: sheets lists what each sheet holds. Read each with read --sheet <name> (repeat --sheet for several), or all with --full."
        );
    }
    report["image_paths"] = json!(images);
    report["sheets"] = Value::Array(sheets);
    Ok(())
}

/// Keeps of the sheet `sheet` of a read what lies in a range: of each
/// element the cells `inside` it, with the number it has in all where some
/// are left out, and the drawings anchored there with the members of their
/// groups. A drawing anchored to no cell is kept.
fn in_range(sheet: &mut Value, inside: &impl Fn(&Value) -> bool) {
    if let Some(elements) = sheet["elements"].as_array_mut() {
        for element in elements.iter_mut() {
            let Some(cells) = element["cells"].as_array_mut() else {
                continue;
            };
            let total = cells.len();
            cells.retain(|cell| inside(&cell["address"]));
            if cells.len() < total {
                element["cells_total"] = json!(total);
            }
        }
        elements.retain(|element| element["cells"].as_array().is_some_and(|c| !c.is_empty()));
    }
    if let Some(drawings) = sheet["drawings"].as_array_mut() {
        let kept: BTreeSet<String> = drawings
            .iter()
            .filter(|drawing| {
                let from = &drawing["position"]["from"]["cell"];
                drawing.get("group").is_none() && (from.is_null() || inside(from))
            })
            .filter_map(|drawing| drawing["id"].as_str().map(str::to_owned))
            .collect();
        // A member follows its group, whatever the depth.
        let groups: HashMap<String, String> = drawings
            .iter()
            .filter_map(|drawing| {
                Some((
                    drawing["id"].as_str()?.to_owned(),
                    drawing["group"].as_str()?.to_owned(),
                ))
            })
            .collect();
        drawings.retain(|drawing| {
            let mut id = drawing["id"].as_str().unwrap_or("");
            for _ in 0..64 {
                match groups.get(id) {
                    Some(group) => id = group,
                    None => break,
                }
            }
            kept.contains(id)
        });
        if drawings.is_empty() {
            sheet.as_object_mut().unwrap().remove("drawings");
        }
    }
}

/// Lists the elements of the sheet `sheet` of a read in place of their
/// cells: the number of cells of each, the range they span and its size,
/// with the numbers of the sheet's drawings and visuals.
fn list_elements(sheet: &mut Value, size: &impl Fn(&Value) -> usize) {
    for element in sheet["elements"].as_array_mut().into_iter().flatten() {
        let places: Vec<(u32, u32)> = element["cells"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|cell| crate::excel::coordinate(cell["address"].as_str()?).ok())
            .collect();
        let corner = |column: u32, row: u32| {
            crate::excel::column_name(column)
                .map_or_else(|_| String::new(), |name| format!("{name}{row}"))
        };
        let span = match (
            places.iter().map(|p| p.0).min(),
            places.iter().map(|p| p.0).max(),
            places.iter().map(|p| p.1).min(),
            places.iter().map(|p| p.1).max(),
        ) {
            (Some(left), Some(right), Some(top), Some(bottom)) => {
                json!(format!("{}:{}", corner(left, top), corner(right, bottom)))
            }
            _ => Value::Null,
        };
        *element = json!({"id":element["id"],"kind":element["kind"],"bytes":size(element),
            "cells":element["cells"].as_array().map_or(0, Vec::len),"range":span});
    }
    for member in ["drawings", "visuals"] {
        if let Some(count) = sheet[member].as_array().map(Vec::len) {
            sheet[member] = json!(count);
        }
    }
}

/// Cell envelope for a whole diagram, retaining the shared anchor of grouped shapes.
/// One-cell and absolute anchors need an explicit range because size is in EMU.
pub(super) fn visual_range(ext: &Extraction, visual: &Value) -> Result<String> {
    let mut min = (u32::MAX, u32::MAX);
    let mut max = (0, 0);
    for source in array(&visual["sources"])? {
        let drawing = ext
            .pointer(string(source)?)
            .context("missing visual source")?;
        ensure!(
            drawing["anchor"]["kind"] == "twoCellAnchor",
            "visual requires an explicit --range for non-cell anchors"
        );
        for key in ["from", "to"] {
            let p = point(&drawing["anchor"][key])
                .context("visual requires an explicit --range for missing anchors")?;
            let (col, row) = crate::excel::coordinate(string(&p["cell"])?)?;
            min = (min.0.min(col), min.1.min(row));
            max = (max.0.max(col), max.1.max(row));
        }
    }
    ensure!(max != (0, 0), "visual has no cell anchors; specify --range");
    let cell = |p: (u32, u32)| {
        point(&json!({"row":p.1 - 1,"col":p.0 - 1})).unwrap()["cell"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    Ok(format!("{}:{}", cell(min), cell(max)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_context_keeps_explicit_connections_and_group_transforms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extraction.json");
        let value = json!({"assets":[],"sheets":[{"name":"S","cells":[],"drawings":[
            {"id":"drawing#1","kind":"group","anchor":{"kind":"absoluteAnchor","pos":{"x":"10","y":"20"},"ext":{"cx":"100","cy":"200"}},"transform":{"chOff":{"x":"1","y":"2"}}},
            {"id":"drawing#2","kind":"connector","group":"drawing#1","anchor":{"kind":"twoCellAnchor","from":{"col":2,"row":4,"colOff":200},"to":{"col":6,"row":8}},"connections":[{"end":"stCxn","target":"drawing#3","site":"0","basis":"explicit"},{"end":"endCxn","target":"drawing#4","site":"1","basis":"explicit"}],"tail_end":"triangle","dash":"dash"}
        ]}]});
        write(&path, &value).unwrap();
        let ext = crate::extraction::sample(value);
        let mut report = json!({});
        read(
            dir.path(),
            &path,
            &ext,
            json!({"regions":[],"elements":[],"visuals":[]}),
            &mut report,
            &[],
            None,
            false,
        )
        .unwrap();
        let group = &report["sheets"][0]["drawings"][0];
        assert_eq!(
            group["position"]["transform"]["chOff"],
            json!({"x":"1","y":"2"})
        );
        assert_eq!(group["position"]["pos"], json!({"x":"10","y":"20"}));
        let connector = &report["sheets"][0]["drawings"][1];
        assert_eq!(connector["group"], "drawing#1");
        assert_eq!(connector["tail_end"], "triangle");
        assert_eq!(connector["dash"], "dash");
        assert!(group.get("tail_end").is_none());
        assert_eq!(
            connector["connections"],
            json!([{"end":"stCxn","target":"drawing#3"},{"end":"endCxn","target":"drawing#4"}])
        );
        assert_eq!(
            connector["position"]["from"],
            json!({"cell":"C5","offset":[200,0]})
        );
        assert!(connector.get("text").is_none());
    }

    #[test]
    fn a_large_document_lists_its_sheets_to_read_one_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extraction.json");
        let cells = |count: usize| -> Vec<Value> {
            (1..=count)
                .map(|row| json!({"address":format!("A{row}"),"type":"string","value":"受注明細の記載".repeat(6)}))
                .collect()
        };
        let value = json!({"assets":[],"sheets":[
            {"name":"大","cells":cells(500),"drawings":[
                {"id":"d#1","kind":"shape","anchor":{"kind":"twoCellAnchor","from":{"col":1,"row":1},"to":{"col":3,"row":4}},
                    "transform":{"off":{"x":"1","y":"2"},"ext":{"cx":"3","cy":"4"}}},
                {"id":"d#2","kind":"shape","anchor":{"kind":"twoCellAnchor","from":{"col":1,"row":1},"to":{"col":3,"row":4}},
                    "transform":{"off":{"x":"1","y":"2"},"ext":{"cx":"3","cy":"4"},"rot":"5400000"}}]},
            {"name":"小","cells":cells(2),"drawings":[]}]});
        write(&path, &value).unwrap();
        let ext = crate::extraction::sample(value);
        let element = |id: &str, sheet: &str, number: usize, count: usize| {
            json!({"id":id,"kind":"text","sheet":sheet,"evidence":[],
                "reading":{"method":"parser","actor":"a","reason":"r"},
                "cells":(1..=count).map(|row| json!({"id":format!("s{number}-A{row}"),"address":format!("A{row}"),
                    "role":"text","headers":[],"text_state":"read"})).collect::<Vec<_>>()})
        };
        let structure = json!({"document":"d","regions":[],"visuals":[
            {"id":"v","kind":"diagram","sheet":"大","sources":["/sheets/0/drawings/0"]}],
            "elements":[element("big", "大", 1, 500), element("small", "小", 2, 2)]});
        let read_with = |only: &[String], whole: bool| {
            let mut report = json!({});
            read(
                dir.path(),
                &path,
                &ext,
                structure.clone(),
                &mut report,
                only,
                None,
                whole,
            )
            .unwrap();
            report
        };
        // Too large for one response: what each sheet holds, and how to read it.
        let listed = read_with(&[], false);
        assert_eq!(listed["sheets_listed"], true);
        assert!(listed["detail"].as_str().unwrap().contains("--sheet"));
        let big = &listed["sheets"][0];
        assert_eq!(big["name"], "大");
        assert_eq!(
            (
                &big["elements"],
                &big["cells"],
                &big["drawings"],
                &big["visuals"]
            ),
            (&json!(1), &json!(500), &json!(2), &json!(1))
        );
        assert!(big["bytes"].as_u64().unwrap() > WHOLE_READ_BYTES as u64);
        assert_eq!(listed["sheets"][1]["cells"], 2);
        // All of it when asked for, each cell with its value in its element.
        let whole = read_with(&[], true);
        assert!(whole.get("sheets_listed").is_none());
        let sheet = &whole["sheets"][0];
        assert_eq!(sheet["elements"][0]["cells"].as_array().unwrap().len(), 500);
        let first = &sheet["elements"][0]["cells"][0];
        assert_eq!(first["address"], "A1");
        assert_eq!(first["value"], "受注明細の記載".repeat(6));
        assert!(first.get("role").is_none() && first.get("id").is_none());
        assert!(sheet.get("cells").is_none());
        assert_eq!(sheet["visuals"][0]["id"], "v");
        // Outside a group the cells place a drawing; a turn is kept, in degrees.
        assert_eq!(
            sheet["drawings"][0]["position"],
            json!({"from":{"cell":"B2"},"to":{"cell":"D5"}})
        );
        assert_eq!(sheet["drawings"][1]["position"]["rot"], json!(90.0));
        assert!(sheet["drawings"][1]["position"].get("box").is_none());
        assert!(sheet["drawings"][0].get("sheet").is_none());
        // One sheet alone is the same sheet.
        let one = read_with(&["小".to_owned()], false);
        assert_eq!(one["sheets"], json!([whole["sheets"][1]]));
        assert!(one.get("sheets_listed").is_none());
        // A document that fits is returned whole without being asked.
        let small = json!({"document":"d","regions":[],"visuals":[],"elements":[element("small", "小", 2, 2)]});
        let mut report = json!({});
        let only_small = crate::extraction::sample(json!({"assets":[],"sheets":[
            {"name":"大","cells":[],"drawings":[]},{"name":"小","cells":cells(2),"drawings":[]}]}));
        read(
            dir.path(),
            &path,
            &only_small,
            small,
            &mut report,
            &[],
            None,
            false,
        )
        .unwrap();
        assert!(report.get("sheets_listed").is_none());
        assert_eq!(
            report["sheets"][1]["elements"][0]["cells"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_large_sheet_lists_its_elements_and_is_read_a_range_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extraction.json");
        let mut cells = vec![];
        for row in 1..=400 {
            for column in ["A", "B"] {
                cells.push(json!({"address":format!("{column}{row}"),"type":"string","value":"受注明細の記載".repeat(5)}));
            }
        }
        let anchor = |row: u64| json!({"kind":"twoCellAnchor","from":{"col":3,"row":row},"to":{"col":5,"row":row + 2}});
        let emu = |x: u64, y: u64, cx: u64, cy: u64| json!({"off":{"x":x.to_string(),"y":y.to_string()},"ext":{"cx":cx.to_string(),"cy":cy.to_string()}});
        let mut group = emu(12_700, 25_400, 1_270_000, 635_000);
        group["chOff"] = json!({"x":"0","y":"0"});
        group["chExt"] = json!({"cx":"1270000","cy":"635000"});
        let mut member = emu(19_050, 0, 127_000, 63_500);
        member["flipH"] = json!("1");
        member["flipV"] = json!("1");
        let value = json!({"assets":[],"sheets":[{"name":"明細","cells":cells,"drawings":[
            {"id":"d#1","kind":"group","anchor":anchor(9),"transform":group},
            {"id":"d#2","kind":"shape","group":"d#1","anchor":anchor(9),"transform":member},
            {"id":"d#3","kind":"shape","anchor":anchor(300),"transform":emu(1, 2, 3, 4)},
            {"id":"d#4","kind":"shape","anchor":{"kind":"absoluteAnchor","pos":{"x":"1","y":"2"},"ext":{"cx":"3","cy":"4"}}}]}]});
        write(&path, &value).unwrap();
        let ext = crate::extraction::sample(value);
        let cell = |column: &str, row: usize, role: &str| {
            json!({"id":format!("s1-{column}{row}"),"address":format!("{column}{row}"),
                "role":role,"headers":[],"text_state":"read"})
        };
        let reading = json!({"method":"parser","actor":"a","reason":"r"});
        let mut table: Vec<Value> = vec![];
        for row in 1..=300 {
            table.push(cell(
                "A",
                row,
                if row == 1 { "column_header" } else { "data" },
            ));
            table.push(cell(
                "B",
                row,
                if row == 1 { "column_header" } else { "data" },
            ));
        }
        for row in 2..=300 {
            for at in [2 * (row - 1), 2 * (row - 1) + 1] {
                let column = if at % 2 == 0 { "A" } else { "B" };
                table[at]["headers"] = json!([format!("s1-{column}1")]);
            }
        }
        let rest: Vec<Value> = (301..=400)
            .flat_map(|row| [cell("A", row, "text"), cell("B", row, "text")])
            .collect();
        let structure = json!({"document":"d","regions":[],"visuals":[],"elements":[
            {"id":"table","kind":"table","sheet":"明細","evidence":[],"reading":reading,"cells":table},
            {"id":"rest","kind":"text","sheet":"明細","evidence":[],"reading":reading,"cells":rest}]});
        let read_with = |range: Option<&str>, whole: bool| {
            let mut report = json!({});
            read(
                dir.path(),
                &path,
                &ext,
                structure.clone(),
                &mut report,
                &["明細".to_owned()],
                range,
                whole,
            )
            .unwrap();
            report
        };
        // The sheet alone is too large: its elements by the cells they span.
        let listed = read_with(None, false);
        assert_eq!(listed["elements_listed"], true);
        assert!(listed["detail"].as_str().unwrap().contains("--range"));
        let sheet = &listed["sheets"][0];
        assert_eq!(sheet["elements"][0]["id"], "table");
        assert_eq!(sheet["elements"][0]["cells"], 600);
        assert_eq!(sheet["elements"][0]["range"], "A1:B300");
        assert_eq!(sheet["elements"][1]["range"], "A301:B400");
        assert!(sheet["elements"][0]["bytes"].as_u64().unwrap() > 0);
        assert_eq!(sheet["drawings"], 4);
        // A range returns the cells in it, each element saying how many it has.
        let part = read_with(Some("A1:F12"), false);
        assert!(part.get("elements_listed").is_none());
        let sheet = &part["sheets"][0];
        let elements = sheet["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0]["cells"].as_array().unwrap().len(), 24);
        assert_eq!(elements[0]["cells_total"], 600);
        // The headers the roles give are left out, as in a whole read.
        assert_eq!(elements[0]["cells"][0]["role"], "column_header");
        assert!(elements[0]["cells"][2].get("headers").is_none());
        assert!(elements[0]["cells"][2].get("role").is_none());
        // The drawings anchored in the range, a group with its members, in
        // points; a drawing anchored to no cell comes with every range.
        let ids: Vec<&str> = sheet["drawings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|drawing| drawing["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["d#1", "d#2", "d#4"]);
        assert_eq!(
            sheet["drawings"][0]["position"],
            json!({"from":{"cell":"D10"},"to":{"cell":"F12"},
                "box":[1, 2, 100, 50],"child_box":[0, 0, 100, 50]})
        );
        // A member is placed in its group, where its group is anchored.
        assert_eq!(
            sheet["drawings"][1]["position"],
            json!({"box":[1.5, 0, 10, 5],"flip":"hv"})
        );
        let later = read_with(Some("A290:F310"), false);
        let elements = later["sheets"][0]["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0]["cells"].as_array().unwrap().len(), 22);
        assert_eq!(elements[1]["cells"].as_array().unwrap().len(), 20);
        assert_eq!(elements[1]["cells_total"], 200);
        let ids: Vec<&str> = later["sheets"][0]["drawings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|drawing| drawing["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["d#3", "d#4"]);
        // All of the sheet when asked for.
        let whole = read_with(None, true);
        assert!(whole.get("elements_listed").is_none());
        assert_eq!(
            whole["sheets"][0]["elements"][0]["cells"]
                .as_array()
                .unwrap()
                .len(),
            600
        );
        // A range needs its sheet named, and its corners in order.
        let mut report = json!({});
        let refused = |range: &str, only: &[String], report: &mut Value| {
            read(
                dir.path(),
                &path,
                &ext,
                structure.clone(),
                report,
                only,
                Some(range),
                false,
            )
            .unwrap_err()
            .to_string()
        };
        assert!(refused("A1:B2", &[], &mut report).contains("--sheet"));
        assert!(refused("B2:A1", &["明細".to_owned()], &mut report).contains("A1:H30"));
    }

    #[test]
    fn diagram_range_encloses_cell_anchors_without_swapping_rows_and_columns() {
        let mut ext =
            crate::extraction::sample(json!({"sheets":[{"name":"S","merges":[],"drawings":[
                {"anchor":{"kind":"twoCellAnchor","from":{"col":1,"row":4},"to":{"col":5,"row":8}}},
                {"anchor":{"kind":"twoCellAnchor","from":{"col":3,"row":2},"to":{"col":7,"row":11}}}
            ]}]}));
        let visual = json!({"sheet":"S","sources":["/sheets/0/drawings/0","/sheets/0/drawings/1"]});
        assert_eq!(visual_range(&ext, &visual).unwrap(), "B3:H12");
        ext.sheets[0]["drawings"][1]["anchor"]["kind"] = json!("oneCellAnchor");
        assert!(visual_range(&ext, &visual).is_err());
        ext.sheets[0]["drawings"][1]["anchor"]["kind"] = json!("absoluteAnchor");
        assert!(visual_range(&ext, &visual).is_err());
        assert_eq!(
            point(&json!({"col":2,"row":4,"colOff":200,"rowOff":0})).unwrap(),
            json!({"cell":"C5","offset":[200,0]})
        );
    }
}
