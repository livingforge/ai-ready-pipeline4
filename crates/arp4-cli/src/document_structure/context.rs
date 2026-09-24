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
    if drawing["transform"].is_object() {
        result["transform"] = drawing["transform"].clone();
    }
    result
}

pub(super) fn assets(
    extraction_path: &Path,
    ext: &Value,
) -> Result<BTreeMap<String, (PathBuf, String)>> {
    let extraction_path = dunce::canonicalize(extraction_path)?;
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

pub(super) fn read(
    root: &Path,
    extraction_path: &Path,
    ext: &Value,
    report: &mut Value,
) -> Result<()> {
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
    let mut pictured = BTreeSet::new();
    let mut images = array(&report["structure"]["regions"] )?.iter().map(|r| {
        Ok(json!({"id":format!("region-{}",string(&r["id"])?),"region":r["id"],"path":image_path(root,string(&r["image"])?)?,"bbox":r["bbox"]}))
    }).collect::<Result<Vec<_>>>()?;
    let mut drawings = vec![];
    for (si, sheet) in array(&ext["sheets"])?.iter().enumerate() {
        for (di, drawing) in sheet["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let source = format!("/sheets/{si}/drawings/{di}");
            let mut compact = json!({"source":source,"sheet":sheet["name"],"id":drawing["id"],"kind":drawing["kind"]});
            for (output, input) in [
                ("text", "text"),
                ("shape", "geometry"),
                ("group", "group"),
                ("alt", "description"),
            ] {
                if drawing[input].as_str().is_some_and(|s| !s.is_empty()) {
                    compact[output] = drawing[input].clone();
                }
            }
            let position = position(drawing);
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
                images.push(json!({"id":image_ids[name],"source":source,"path":path,"sheet":sheet["name"],"position":compact["position"]}));
                pictured.insert(name);
            }
            drawings.push(compact);
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
    report["drawings"] = json!(drawings);
    report["image_paths"] = json!(images);
    report["original_cells"] = ext["sheets"].clone();
    for sheet in report["original_cells"].as_array_mut().unwrap() {
        sheet.as_object_mut().unwrap().remove("drawings");
    }
    Ok(())
}

/// Cell envelope for a whole diagram, retaining the shared anchor of grouped shapes.
/// One-cell and absolute anchors need an explicit range because size is in EMU.
pub(super) fn visual_range(ext: &Value, visual: &Value) -> Result<String> {
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
        let ext = json!({"assets":[],"sheets":[{"name":"S","cells":[],"drawings":[
            {"id":"drawing#1","kind":"group","anchor":{"kind":"absoluteAnchor","pos":{"x":"10","y":"20"},"ext":{"cx":"100","cy":"200"}},"transform":{"chOff":{"x":"1","y":"2"}}},
            {"id":"drawing#2","kind":"connector","group":"drawing#1","anchor":{"kind":"twoCellAnchor","from":{"col":2,"row":4,"colOff":200},"to":{"col":6,"row":8}},"connections":[{"end":"stCxn","target":"drawing#3","site":"0","basis":"explicit"},{"end":"endCxn","target":"drawing#4","site":"1","basis":"explicit"}]}
        ]}]});
        write(&path, &ext).unwrap();
        let mut report = json!({"structure":{"regions":[]}});
        read(dir.path(), &path, &ext, &mut report).unwrap();
        let group = &report["drawings"][0];
        assert_eq!(
            group["position"]["transform"]["chOff"],
            json!({"x":"1","y":"2"})
        );
        assert_eq!(group["position"]["pos"], json!({"x":"10","y":"20"}));
        let connector = &report["drawings"][1];
        assert_eq!(connector["group"], "drawing#1");
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
    fn diagram_range_encloses_cell_anchors_without_swapping_rows_and_columns() {
        let mut ext = json!({"sheets":[{"name":"S","merges":[],"drawings":[
            {"anchor":{"kind":"twoCellAnchor","from":{"col":1,"row":4},"to":{"col":5,"row":8}}},
            {"anchor":{"kind":"twoCellAnchor","from":{"col":3,"row":2},"to":{"col":7,"row":11}}}
        ]}]});
        let visual = json!({"sheet":"S","sources":["/sheets/0/drawings/0","/sheets/0/drawings/1"]});
        assert_eq!(visual_range(&ext, &visual).unwrap(), "B3:H12");
        ext["sheets"][0]["drawings"][1]["anchor"]["kind"] = json!("oneCellAnchor");
        assert!(visual_range(&ext, &visual).is_err());
        ext["sheets"][0]["drawings"][1]["anchor"]["kind"] = json!("absoluteAnchor");
        assert!(visual_range(&ext, &visual).is_err());
        assert_eq!(
            point(&json!({"col":2,"row":4,"colOff":200,"rowOff":0})).unwrap(),
            json!({"cell":"C5","offset":[200,0]})
        );
    }
}
