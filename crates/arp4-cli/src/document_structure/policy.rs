//! Requirements derived from the captured document, before semantic extraction.
use super::*;

pub fn policy() -> Value {
    serde_json::from_str(include_str!("../../../../contracts/structure-policy.json")).unwrap()
}

pub fn extension(extraction: &Value) -> String {
    Path::new(extraction["source"]["path"].as_str().unwrap_or(""))
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn requirements(extraction: &Value) -> Result<Vec<String>> {
    let policy = policy();
    let ext = json!(extension(extraction));
    if array(&policy["always_required_extensions"])?.contains(&ext) {
        return Ok(vec!["office_document".into()]);
    }
    if !array(&policy["conditional_extensions"])?.contains(&ext) {
        return Ok(vec![]);
    }
    let mut reasons = vec![];
    if array(&extraction["sheets"])?
        .iter()
        .any(|s| s["tables"].as_array().is_some_and(|t| !t.is_empty()))
        || inference::elements(extraction)?
            .iter()
            .any(|e| e["kind"] == "table")
    {
        reasons.push("table".into());
    }
    let visuals = visuals(extraction)?;
    for kind in ["image", "diagram"] {
        if visuals.iter().any(|v| v["kind"] == kind) {
            reasons.push(kind.into());
        }
    }
    Ok(reasons)
}

/// Three or more shapes on a sheet are diagram candidates. Their actual relationship
/// must be checked by the reader; disconnected DrawingML does not prove independence.
pub fn visuals(extraction: &Value) -> Result<Vec<Value>> {
    if !["xlsx", "xlsm"].contains(&extension(extraction).as_str()) {
        return Ok(vec![]);
    }
    let minimum = policy()["diagram_min_shapes"].as_u64().unwrap() as usize;
    let mut output = vec![];
    let mut pictured_assets = BTreeSet::new();
    for (si, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let mut shapes = vec![];
        for (di, d) in sheet["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let pointer = format!("/sheets/{si}/drawings/{di}");
            if d["kind"] == "picture" {
                output.push(json!({"id":format!("image-{si}-{di}"),"kind":"image","sheet":sheet["name"],"sources":[pointer]}));
                if let Some(asset) = d["image"]["asset"].as_str() {
                    pictured_assets.insert(asset);
                }
            }
            if ["shape", "connector", "graphic"].contains(&d["kind"].as_str().unwrap_or("")) {
                shapes.push(pointer);
            }
        }
        if shapes.len() >= minimum {
            output.push(json!({"id":format!("diagram-{si}"),"kind":"diagram","sheet":sheet["name"],"sources":shapes}));
        }
    }
    // Assets can include pictures not represented in the captured DrawingML.
    for (i, asset) in array(&extraction["assets"])?.iter().enumerate() {
        if !asset["path"]
            .as_str()
            .is_some_and(|p| pictured_assets.contains(p))
        {
            output.push(json!({"id":format!("asset-{i}"),"kind":"image","sources":[format!("/assets/{i}")]}));
        }
    }
    for v in &mut output {
        v["state"] = json!("not_examined");
        v["description"] = json!("");
        v["actor"] = json!("");
        v["evidence"] = json!([]);
        if v["kind"] == "image" {
            v["ocr"] = json!({"status":"not_examined","text":"","reason":""});
            let asset_name = v["sources"]
                .as_array()
                .and_then(|sources| sources.first())
                .and_then(Value::as_str)
                .and_then(|pointer| extraction.pointer(pointer))
                .and_then(|source| {
                    source["image"]["asset"]
                        .as_str()
                        .or_else(|| source["path"].as_str())
                });
            if let Some(ocr) = asset_name.and_then(|name| {
                extraction["assets"]
                    .as_array()?
                    .iter()
                    .find(|asset| asset["path"] == name)
                    .and_then(|asset| asset.get("ocr"))
            }) {
                v["ocr"] = ocr.clone();
            }
        }
    }
    Ok(output)
}

pub fn validate_visuals(extraction: &Value, value: &Value) -> Result<Vec<Value>> {
    let expected = visuals(extraction)?;
    let actual = array(&value["visuals"])?;
    ensure!(
        expected.len() == actual.len(),
        "structure omits or adds visual objects"
    );
    let mut unresolved = vec![];
    for original in expected {
        let matches: Vec<_> = actual
            .iter()
            .filter(|v| v["id"] == original["id"])
            .collect();
        ensure!(matches.len() == 1, "missing or duplicate visual object");
        let v = matches[0];
        for key in ["kind", "sheet", "sources"] {
            ensure!(v[key] == original[key], "visual source identity mismatch");
        }
        for id in array(&v["evidence"])? {
            let region = array(&value["regions"])?.iter().find(|r| r["id"] == *id);
            if let Some(region) = region {
                ensure!(
                    v["sheet"].is_null() || region["sheet"] == v["sheet"],
                    "visual evidence on different sheet"
                );
                continue;
            }
            ensure!(
                array(&v["sources"])?.contains(id)
                    && extraction
                        .pointer(string(id)?)
                        .is_some_and(|s| s["image"]["asset"].is_string()
                            || (string(id).is_ok_and(|p| p.starts_with("/assets/"))
                                && s["path"].is_string())),
                "unknown visual image evidence"
            );
        }
        if !visual_ready(v) {
            unresolved.push(json!({"visual":v["id"],"state":"visual_or_ocr_unresolved"}));
        }
    }
    Ok(unresolved)
}

pub fn visual_ready(v: &Value) -> bool {
    let nonempty = |value: &Value| value.as_str().is_some_and(|s| !s.trim().is_empty());
    v["state"] == "read"
        && nonempty(&v["actor"])
        && nonempty(&v["description"])
        && (v["kind"] != "image"
            || (nonempty(&v["ocr"]["reason"])
                && ((v["ocr"]["status"] == "available" && v["ocr"]["text"].is_string())
                    || v["ocr"]["status"] == "unavailable")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extraction() -> Value {
        json!({"source":{"path":"docs/input.xlsx"},"assets":[],"sheets":[{
            "name":"S","merges":[],"cells":[{"address":"A1","value":"Title","formula":null}],"drawings":[]}]})
    }

    #[test]
    fn office_formats_and_conditional_excel() {
        let mut ext = extraction();
        assert!(requirements(&ext).unwrap().is_empty());
        for format in ["docx", "pptx", "pdf"] {
            ext["source"]["path"] = json!(format!("input.{format}"));
            assert_eq!(requirements(&ext).unwrap(), vec!["office_document"]);
        }
        ext["source"]["path"] = json!("input.XLSX");
        ext["sheets"][0]["tables"] = json!([{"range":"A1:B2","header_rows":1}]);
        assert_eq!(requirements(&ext).unwrap(), vec!["table"]);
        ext["source"]["path"] = json!("input.csv");
        assert!(requirements(&ext).unwrap().is_empty());
    }

    #[test]
    fn inferred_table_also_requires_review() {
        let mut ext = extraction();
        ext["sheets"][0]["cells"] = json!(
            [
                ("A1", "Name"),
                ("B1", "Value"),
                ("A2", "Limit"),
                ("B2", "5")
            ]
            .iter()
            .map(|(a, v)| json!({"address":a,"value":v,"formula":null}))
            .collect::<Vec<_>>()
        );
        assert_eq!(requirements(&ext).unwrap(), vec!["table"]);
    }

    #[test]
    fn image_presence_does_not_depend_on_ocr_and_diagrams_have_a_boundary() {
        let mut ext = extraction();
        ext["sheets"][0]["drawings"] = json!([{"kind":"shape"},{"kind":"shape"},{"kind":"group"}]);
        assert!(requirements(&ext).unwrap().is_empty());
        ext["sheets"][0]["drawings"][2]["kind"] = json!("connector");
        assert_eq!(requirements(&ext).unwrap(), vec!["diagram"]);
        ext["sheets"][0]["drawings"] = json!([{"kind":"picture","image":{"asset":"a.png"}}]);
        ext["assets"] = json!([{"path":"a.png"}]);
        assert_eq!(requirements(&ext).unwrap(), vec!["image"]);
        assert_eq!(visuals(&ext).unwrap().len(), 1);
        let mut value = json!({"regions":[],"visuals":visuals(&ext).unwrap()});
        assert_eq!(validate_visuals(&ext, &value).unwrap().len(), 1);
        value["visuals"][0]["state"] = json!("read");
        value["visuals"][0]["actor"] = json!("reader");
        value["visuals"][0]["description"] = json!("Read image");
        value["visuals"][0]["ocr"] = json!({"status":"available","text":"","reason":"OCR engine"});
        assert!(validate_visuals(&ext, &value).unwrap().is_empty());
        value["visuals"][0]["ocr"]["text"] = json!("Extracted image label");
        assert!(validate_visuals(&ext, &value).unwrap().is_empty());
        value["visuals"][0]["ocr"] =
            json!({"status":"unavailable","text":"","reason":"No OCR engine available"});
        assert!(validate_visuals(&ext, &value).unwrap().is_empty());
        value["visuals"] = json!([]);
        assert!(validate_visuals(&ext, &value).is_err());
    }

    #[test]
    fn empty_ocr_is_reviewable_and_requested_correction_replaces_it() {
        let mut ext = extraction();
        ext["sheets"][0]["drawings"] = json!([{"kind":"picture"}]);
        let mut value = json!({
            "schema_version":1,"document":"doc",
            "source":{"path":"docs/input.xlsx","sha256":"a".repeat(64)},
            "extraction_hash":"b".repeat(64),"elements":[],"regions":[],
            "visuals":visuals(&ext).unwrap(),"review":{"status":"pending"}
        });
        value["visuals"][0]["state"] = json!("read");
        value["visuals"][0]["actor"] = json!("ocr-reader");
        value["visuals"][0]["description"] =
            json!("OCR executed; no text recognized. Image not inspected by an LLM.");
        value["visuals"][0]["ocr"] =
            json!({"status":"available","text":"","reason":"OCR completed with an empty result"});
        let accept = |value: &mut Value| {
            value["review"] = json!({"status":"accepted","content_hash":content_hash(value),
                "actor":"reviewer","reason":"Checked reading records"});
        };
        accept(&mut value);
        // No LLM correction, image evidence or nonempty OCR text is required.
        validate_snapshot("doc", &value).unwrap();
        let mut pending = value.clone();
        pending["visuals"][0]["ocr"]["status"] = json!("not_examined");
        accept(&mut pending);
        assert!(validate_snapshot("doc", &pending).is_err());

        value["visuals"][0]["ocr"]["text"] = json!("Corrected label from image");
        value["visuals"][0]["ocr"]["llm_correction"] = json!({"actor":"visual-reader","user_instruction":"Inspect this image and correct OCR"});
        accept(&mut value);
        assert!(validate_snapshot("doc", &value).is_err()); // image evidence required
        value["regions"] = json!([{"id":"image-view","sheet":"S","range":"A1","image":"evidence/view.png",
            "image_sha256":"c".repeat(64),"source_sha256":"a".repeat(64),"bbox":[0,0,1,1]}]);
        value["visuals"][0]["evidence"] = json!(["image-view"]);
        accept(&mut value);
        validate_snapshot("doc", &value).unwrap();
        assert!(validate_visuals(&ext, &value).unwrap().is_empty());
        let mut no_instruction = value.clone();
        no_instruction["visuals"][0]["ocr"]["llm_correction"]
            .as_object_mut()
            .unwrap()
            .remove("user_instruction");
        accept(&mut no_instruction);
        assert!(validate_snapshot("doc", &no_instruction).is_err());

        value["visuals"][0]["ocr"]["text"] = json!("Further corrected label");
        assert!(validate_snapshot("doc", &value).is_err()); // old review is stale
        accept(&mut value);
        validate_snapshot("doc", &value).unwrap();
    }
}
