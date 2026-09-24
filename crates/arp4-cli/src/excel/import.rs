use super::*;

pub(super) fn sheet_parts(
    parts: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<(String, String, String)>> {
    let workbook = xml(parts.get("xl/workbook.xml").context("missing workbook")?)?;
    ensure!(
        workbook.root_element().has_tag_name((NS, "workbook")),
        "transitional OOXML workbook required"
    );
    let rels = xml(parts
        .get("xl/_rels/workbook.xml.rels")
        .context("missing workbook relationships")?)?;
    let mut relationships = BTreeMap::new();
    for r in rels.root_element().children().filter(Node::is_element) {
        let id = r.attribute("Id").context("missing relationship ID")?;
        ensure!(
            relationships.insert(id, r).is_none(),
            "duplicate relationship ID"
        );
    }
    let mut output = vec![];
    let mut names = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for s in child(workbook.root_element(), "sheets")
        .context("missing sheets")?
        .children()
        .filter(|n| n.has_tag_name((NS, "sheet")))
    {
        let name = s.attribute("name").context("missing sheet name")?;
        let rel = relationships
            .get(
                s.attribute((REL, "id"))
                    .context("missing sheet relationship")?,
            )
            .context("missing relationship")?;
        ensure!(
            rel.attribute("TargetMode") != Some("External"),
            "external worksheet is unsupported"
        );
        let target = rel
            .attribute("Target")
            .context("missing worksheet target")?;
        ensure!(!target.contains(['\\', ':', '%']), "invalid worksheet part");
        let path = if target.starts_with('/') {
            target.trim_start_matches('/').to_owned()
        } else {
            format!("xl/{target}")
        };
        let mut segments = vec![];
        for segment in path.split('/') {
            match segment {
                ".." => {
                    ensure!(segments.pop().is_some(), "invalid worksheet part");
                }
                "." | "" => {}
                _ => segments.push(segment),
            }
        }
        let part = segments.join("/");
        ensure!(
            part.starts_with("xl/") && parts.contains_key(&part),
            "missing/outside worksheet part"
        );
        ensure!(
            names.insert(name.to_lowercase()) && targets.insert(part.clone()),
            "duplicate worksheet name or part"
        );
        output.push((
            name.into(),
            part,
            s.attribute("state").unwrap_or("visible").into(),
        ));
    }
    ensure!(!output.is_empty(), "workbook has no worksheets");
    Ok(output)
}

impl Workbook {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_bytes(fs::read(path)?)
    }
    pub fn from_bytes(raw: Vec<u8>) -> Result<Self> {
        let mut archive = ZipArchive::new(Cursor::new(&raw))?;
        ensure!(
            archive.len() <= 10000
                && archive.decompressed_size().unwrap_or(u128::MAX) <= 512 * 1024 * 1024,
            "Excel archive exceeds size budget"
        );
        // ZipArchive indexes by name; inspect the directory before duplicate entries can be hidden.
        let mut offset = usize::try_from(archive.central_directory_start())?;
        let mut central_names = BTreeSet::new();
        while raw.get(offset..offset + 4) == Some(b"PK\x01\x02") {
            let header = raw
                .get(offset..offset + 46)
                .context("truncated ZIP directory")?;
            let length = |i| usize::from(u16::from_le_bytes([header[i], header[i + 1]]));
            let name_length = length(28);
            let next = offset + 46 + name_length + length(30) + length(32);
            ensure!(next <= raw.len(), "truncated ZIP directory");
            ensure!(
                central_names.insert(raw[offset + 46..offset + 46 + name_length].to_vec()),
                "duplicate Excel ZIP members"
            );
            offset = next;
        }
        ensure!(
            central_names.len() == archive.len(),
            "invalid ZIP directory"
        );
        let mut parts = BTreeMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            ensure!(
                entry.size() <= 256 * 1024 * 1024,
                "Excel part exceeds size budget"
            );
            let name = entry.name().to_owned();
            ensure!(
                !name.contains('\\') && !name.split('/').any(|s| s == ".."),
                "invalid ZIP path"
            );
            let mut data = vec![];
            entry.read_to_end(&mut data)?;
            ensure!(
                parts.insert(name, data).is_none(),
                "duplicate Excel ZIP members"
            );
        }
        let mut shared = vec![];
        if let Some(bytes) = parts.get("xl/sharedStrings.xml") {
            let doc = xml(bytes)?;
            for item in doc.root_element().children().filter(Node::is_element) {
                shared.push(texts(item));
            }
        }
        let builtin: Value = serde_json::from_str(include_str!(
            "../../../../contracts/excel-number-formats.json"
        ))?;
        let mut formats: BTreeMap<u32, String> = builtin
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), v.as_str().unwrap().into()))
            .collect();
        let mut styles = vec!["General".to_owned()];
        let mut appearance = vec![json!({"bold":false,"fill":0,"border":0})];
        if let Some(bytes) = parts.get("xl/styles.xml") {
            let doc = xml(bytes)?;
            if let Some(n) = child(doc.root_element(), "numFmts") {
                for f in n.children().filter(Node::is_element) {
                    formats.insert(
                        f.attribute("numFmtId")
                            .context("missing format ID")?
                            .parse()?,
                        f.attribute("formatCode").context("missing format")?.into(),
                    );
                }
            }
            if let Some(n) = child(doc.root_element(), "cellXfs") {
                let fonts: Vec<_> = child(doc.root_element(), "fonts")
                    .map(|n| {
                        n.children()
                            .filter(Node::is_element)
                            .map(|f| child(f, "b").is_some_and(|b| b.attribute("val") != Some("0")))
                            .collect()
                    })
                    .unwrap_or_default();
                appearance = n
                    .children()
                    .filter(Node::is_element)
                    .map(|f| {
                        let font = f.attribute("fontId").unwrap_or("0").parse::<usize>()?;
                        Ok(json!({"bold":fonts.get(font).copied().unwrap_or(false),
                        "fill":f.attribute("fillId").unwrap_or("0").parse::<u32>()?,
                        "border":f.attribute("borderId").unwrap_or("0").parse::<u32>()?}))
                    })
                    .collect::<Result<Vec<_>>>()?;
                styles = n
                    .children()
                    .filter(Node::is_element)
                    .map(|f| {
                        Ok(formats
                            .get(&f.attribute("numFmtId").unwrap_or("0").parse()?)
                            .cloned()
                            .unwrap_or_else(|| "General".into()))
                    })
                    .collect::<Result<_>>()?;
            }
        }
        let mut sheets = vec![];
        for (index, (name, part, state)) in sheet_parts(&parts)?.into_iter().enumerate() {
            let doc = xml(&parts[&part])?;
            ensure!(
                doc.root_element().has_tag_name((NS, "worksheet")),
                "invalid worksheet namespace"
            );
            let mut cells = vec![];
            let mut addresses = BTreeSet::new();
            if let Some(data) = child(doc.root_element(), "sheetData") {
                for row in data.children().filter(|n| n.has_tag_name((NS, "row"))) {
                    for c in row.children().filter(|n| n.has_tag_name((NS, "c"))) {
                        let address = c.attribute("r").context("missing cell address")?;
                        coordinate(address)?;
                        ensure!(addresses.insert(address), "duplicate Excel cell");
                        let formula = child(c, "f");
                        let raw_value = child(c, "v").and_then(|n| n.text()).unwrap_or("");
                        let (kind, value) = match c.attribute("t").unwrap_or("n") {
                            "inlineStr" => ("string", json!(texts(c))),
                            _ if raw_value.is_empty() => ("null", Value::Null),
                            "s" => (
                                "string",
                                json!(
                                    shared
                                        .get(raw_value.parse::<usize>()?)
                                        .context("invalid shared string index")?
                                ),
                            ),
                            "b" => {
                                ensure!(
                                    raw_value == "0" || raw_value == "1",
                                    "invalid Excel boolean"
                                );
                                ("boolean", json!(raw_value == "1"))
                            }
                            "n" => {
                                let v = if let Ok(n) = raw_value.parse::<i64>() {
                                    json!(n)
                                } else {
                                    let n: f64 = raw_value.parse()?;
                                    ensure!(n.is_finite(), "non-finite Excel number");
                                    json!(n)
                                };
                                ("number", v)
                            }
                            "e" => ("error", json!(raw_value)),
                            _ => ("string", json!(raw_value)),
                        };
                        if value.is_null() && formula.is_none() {
                            continue;
                        }
                        let format = styles
                            .get(c.attribute("s").unwrap_or("0").parse::<usize>()?)
                            .context("invalid style index")?;
                        cells.push(json!({"id":format!("c-{}-{address}",index+1),"address":address,"type":if formula.is_some(){"formula"}else{kind},"value":value,"cached":if formula.is_some(){value.clone()}else{Value::Null},"formula":formula.map(|f|f.text().unwrap_or("")),"number_format":format}));
                        cells.last_mut().unwrap()["style"] = appearance
                            .get(c.attribute("s").unwrap_or("0").parse::<usize>()?)
                            .context("invalid appearance index")?
                            .clone();
                    }
                }
            }
            let merges = child(doc.root_element(), "mergeCells")
                .map(|n| {
                    n.children()
                        .filter(Node::is_element)
                        .map(|m| {
                            m.attribute("ref")
                                .context("missing merge reference")
                                .map(str::to_owned)
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            for m in &merges {
                let (a, b) = m.split_once(':').context("invalid merge range")?;
                let (c1, r1) = coordinate(a)?;
                let (c2, r2) = coordinate(b)?;
                ensure!(c1 <= c2 && r1 <= r2, "reversed merge range");
            }
            let drawings = visuals::extract_visuals(&parts, &part, doc.root_element())?;
            let tables = visuals::extract_tables(&parts, &part, doc.root_element())?;
            sheets.push(json!({"name":name,"part":part,"state":state,"merges":merges,"cells":cells,"drawings":drawings,"tables":tables}));
        }
        Ok(Self { raw, parts, sheets })
    }
}
