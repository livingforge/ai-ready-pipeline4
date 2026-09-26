use super::*;

/// Sheet types that carry no cell grid for extraction. Their parts are kept
/// byte-for-byte on writeback because only patched parts are rewritten.
fn skipped_sheet_kind(relationship_type: &str) -> Option<&'static str> {
    match relationship_type
        .strip_prefix(REL)
        .or_else(|| relationship_type.strip_prefix(MS_REL))?
    {
        "/chartsheet" => Some("chartsheet"),
        "/dialogsheet" => Some("dialogsheet"),
        "/xlMacrosheet" | "/xlIntlMacrosheet" => Some("macrosheet"),
        _ => None,
    }
}

/// Master cells of shared formulas by group index: (column, row, formula).
fn shared_formula_masters<'a>(
    sheet_data: Node<'a, '_>,
) -> Result<BTreeMap<&'a str, (u32, u32, &'a str)>> {
    let mut masters = BTreeMap::new();
    for cell in sheet_data
        .descendants()
        .filter(|n| n.has_tag_name((NS, "c")))
    {
        let Some(formula) = child(cell, "f") else {
            continue;
        };
        if formula.attribute("t") == Some("shared")
            && formula.attribute("ref").is_some()
            && let Some(text) = formula.text()
        {
            let (column, row) = coordinate(cell.attribute("r").context("missing cell address")?)?;
            masters.insert(
                formula
                    .attribute("si")
                    .context("shared formula without si")?,
                (column, row, text),
            );
        }
    }
    Ok(masters)
}

/// The formula a cell shows. Members of a shared formula store only the group
/// index; Excel derives their formula from the master by moving relative references.
fn formula_text(
    formula: Node<'_, '_>,
    address: &str,
    masters: &BTreeMap<&str, (u32, u32, &str)>,
) -> Result<String> {
    if let Some(text) = formula.text() {
        return Ok(text.to_owned());
    }
    if formula.attribute("t") != Some("shared") {
        return Ok(String::new());
    }
    let group = formula
        .attribute("si")
        .context("shared formula without si")?;
    let (master_column, master_row, text) = masters
        .get(group)
        .context("shared formula master missing")?;
    let (column, row) = coordinate(address)?;
    references::shift_relative(
        text,
        i64::from(row) - i64::from(*master_row),
        i64::from(column) - i64::from(*master_column),
    )
}

/// A worksheet as (name, part, state).
type Worksheet = (String, String, String);

/// Returns worksheets and the non-worksheet sheets as {name, kind}.
pub(super) fn sheet_parts(
    parts: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<Worksheet>, Vec<Value>)> {
    let workbook = xml(parts.get("xl/workbook.xml").context("missing workbook")?)?;
    ensure!(
        workbook.root_element().tag_name().namespace()
            != Some("http://purl.oclc.org/ooxml/spreadsheetml/main"),
        "Strict Open XML workbooks are not supported; save the file in Excel as 'Excel Workbook (*.xlsx)' and import that copy"
    );
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
    let mut skipped = vec![];
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
        let kind = rel
            .attribute("Type")
            .context("missing sheet relationship type")?;
        if let Some(kind) = skipped_sheet_kind(kind) {
            ensure!(
                names.insert(name.to_lowercase()),
                "duplicate worksheet name or part"
            );
            skipped.push(json!({"name":name,"kind":kind}));
            continue;
        }
        ensure!(
            kind == format!("{REL}/worksheet"),
            "unsupported sheet relationship type: {kind}"
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
    Ok((output, skipped))
}

impl Workbook {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_bytes(fs::read(path)?)
    }
    pub fn from_bytes(raw: Vec<u8>) -> Result<Self> {
        crate::document_source::ensure_zip_package(&raw)?;
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
        let mut cell_formats = vec![CellFormat {
            number_format: "General".to_owned(),
            appearance: json!({"bold":false,"fill":0,"border":0}),
        }];
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
                            .map(|f| {
                                // ST_OnOff: `false` and `off` turn it off as `0` does.
                                child(f, "b").is_some_and(|b| {
                                    !matches!(b.attribute("val"), Some("0" | "false" | "off"))
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                cell_formats = n
                    .children()
                    .filter(Node::is_element)
                    .map(|f| {
                        let font = f.attribute("fontId").unwrap_or("0").parse::<usize>()?;
                        let appearance = json!({"bold":fonts.get(font).copied().unwrap_or(false),
                        "fill":f.attribute("fillId").unwrap_or("0").parse::<u32>()?,
                        "border":f.attribute("borderId").unwrap_or("0").parse::<u32>()?});
                        let number_format = formats
                            .get(&f.attribute("numFmtId").unwrap_or("0").parse()?)
                            .cloned()
                            .unwrap_or_else(|| "General".into());
                        Ok(CellFormat {
                            number_format,
                            appearance,
                        })
                    })
                    .collect::<Result<_>>()?;
            }
        }
        let workbook = xml(&parts["xl/workbook.xml"])?;
        let date1904 = child(workbook.root_element(), "workbookPr")
            .and_then(|properties| properties.attribute("date1904"))
            .is_some_and(|flag| matches!(flag, "1" | "true" | "on"));
        let mut rich_values = vec![];
        let mut sheets = vec![];
        let mut sheet_cells = vec![];
        let (worksheets, skipped_sheets) = sheet_parts(&parts)?;
        let persons = visuals::persons(&parts)?;
        for (name, part, state) in worksheets {
            let doc = xml(&parts[&part])?;
            ensure!(
                doc.root_element().has_tag_name((NS, "worksheet")),
                "invalid worksheet namespace"
            );
            let mut cells = vec![];
            let mut addresses = BTreeSet::new();
            if let Some(data) = child(doc.root_element(), "sheetData") {
                let masters = shared_formula_masters(data)?;
                for row in data.children().filter(|n| n.has_tag_name((NS, "row"))) {
                    for c in row.children().filter(|n| n.has_tag_name((NS, "c"))) {
                        let address = c.attribute("r").context("missing cell address")?;
                        coordinate(address)?;
                        ensure!(addresses.insert(address), "duplicate Excel cell");
                        let formula = child(c, "f");
                        let formula_text = formula
                            .map(|f| formula_text(f, address, &masters))
                            .transpose()?;
                        let raw_value = child(c, "v").and_then(|n| n.text()).unwrap_or("");
                        // A picture placed in the cell or a linked data type keeps
                        // its value in rich data; the cell holds only `#VALUE!`.
                        if c.attribute("vm").is_some() {
                            rich_values.push(format!("{name}!{address}"));
                        }
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
                            "d" => match iso_date_serial(raw_value, date1904) {
                                Some(serial) if serial.fract() == 0.0 => {
                                    ("number", json!(serial as i64))
                                }
                                Some(serial) => ("number", json!(serial)),
                                None => ("string", json!(raw_value)),
                            },
                            _ => ("string", json!(decode_xstring(raw_value))),
                        };
                        if value.is_null() && formula.is_none() {
                            continue;
                        }
                        let format = c.attribute("s").unwrap_or("0").parse::<usize>()?;
                        ensure!(format < cell_formats.len(), "invalid style index");
                        cells.push(Cell {
                            address: address.to_owned(),
                            kind: if formula.is_some() { "formula" } else { kind },
                            value,
                            formula: formula_text,
                            format,
                        });
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
            let comments = visuals::extract_comments(&parts, &persons, &part)?;
            let computed = computed_ranges(&parts, &part, doc.root_element())?;
            sheets.push(json!({"name":name,"part":part,"state":state,"merges":merges,"drawings":drawings,"tables":tables,"comments":comments,"computed":computed}));
            sheet_cells.push(cells);
        }
        // Name unique images in first-use order across the workbook. Hashes remain
        // integrity metadata rather than names that readers must copy.
        let mut image_names = BTreeMap::new();
        for sheet in &mut sheets {
            for drawing in sheet["drawings"].as_array_mut().unwrap() {
                if let Some(sha) = drawing["image"]["sha256"].as_str() {
                    let next = image_names.len() + 1;
                    let extension = Path::new(drawing["image"]["part"].as_str().unwrap())
                        .extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                        .unwrap_or("bin");
                    let name = image_names
                        .entry(sha.to_owned())
                        .or_insert_with(|| format!("image-{next:03}.{extension}"))
                        .clone();
                    drawing["image"]["asset"] = json!(name);
                }
            }
        }
        Ok(Self {
            raw,
            parts,
            sheets,
            cells: sheet_cells,
            formats: cell_formats,
            skipped_sheets,
            date1904,
            rich_values,
        })
    }
}

/// Ranges whose cells Excel fills from a formula or a pivot table: array formulas
/// (Ctrl+Shift+Enter arrays and dynamic spills), What-If data tables and pivot
/// table bodies. Their member cells look like constants, but Excel overwrites any
/// value written there.
fn computed_ranges(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
    sheet: Node<'_, '_>,
) -> Result<Vec<Value>> {
    let mut ranges = vec![];
    for formula in sheet.descendants().filter(|n| n.has_tag_name((NS, "f"))) {
        let kind = match formula.attribute("t") {
            Some("array") => "array",
            Some("dataTable") => "data_table",
            _ => continue,
        };
        if let Some(range) = formula.attribute("ref") {
            Area::parse(range)?;
            ranges.push(json!({"range":range,"kind":kind}));
        }
    }
    for pivot in related_parts(parts, part, "/pivotTable")? {
        let doc = xml(&parts[&pivot])?;
        let location =
            child(doc.root_element(), "location").context("pivot table without location")?;
        let range = location
            .attribute("ref")
            .context("pivot table without location")?;
        Area::parse(range)?;
        ranges.push(json!({"range":range,"kind":"pivot"}));
    }
    Ok(ranges)
}

/// The date serial of an ISO 8601 date or date-time (`t="d"` cells, which some
/// writers use instead of a number), counted as Excel counts dates: from 1904-01-01
/// in the 1904 date system, else with 1900-01-01 as 1 and Excel's 1900-02-29.
fn iso_date_serial(text: &str, date1904: bool) -> Option<f64> {
    let text = text.trim().trim_end_matches('Z');
    let (date, time) = text.split_once('T').unwrap_or((text, ""));
    let mut fields = date.splitn(3, '-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (fields.next()??, fields.next()??, fields.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days from 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut serial = if date1904 {
        days + 24_107
    } else {
        // 1899-12-30 is day 0; Excel counts a 1900-02-29, so earlier dates are one less.
        let serial = days + 25_569;
        if serial < 61 { serial - 1 } else { serial }
    } as f64;
    if !time.is_empty() {
        let mut parts = time.splitn(3, ':');
        let hours: f64 = parts.next()?.parse().ok()?;
        let minutes: f64 = parts.next().unwrap_or("0").parse().ok()?;
        let seconds: f64 = parts.next().unwrap_or("0").parse().ok()?;
        serial += (hours * 3600.0 + minutes * 60.0 + seconds) / 86_400.0;
    }
    (serial >= 0.0).then_some(serial)
}
