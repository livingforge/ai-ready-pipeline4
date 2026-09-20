use crate::data::*;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read, Write},
    path::Path,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub struct Workbook {
    pub raw: Vec<u8>,
    pub parts: BTreeMap<String, Vec<u8>>,
    pub sheets: Vec<Value>,
}
fn xml(bytes: &[u8]) -> Result<Document<'_>> {
    Ok(Document::parse(std::str::from_utf8(bytes)?)?)
}
fn child<'a, 'b>(node: Node<'a, 'b>, name: &str) -> Option<Node<'a, 'b>> {
    node.children().find(|n| n.has_tag_name((NS, name)))
}
fn texts(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.has_tag_name((NS, "t")))
        .filter_map(|n| n.text())
        .collect()
}
pub fn coordinate(address: &str) -> Result<(u32, u32)> {
    let split = address
        .find(|c: char| c.is_ascii_digit())
        .context("invalid Excel coordinate")?;
    let (letters, row) = address.split_at(split);
    ensure!(
        !letters.is_empty() && !row.starts_with('0'),
        "invalid Excel coordinate"
    );
    let mut column = 0u32;
    for b in letters.bytes() {
        ensure!(b.is_ascii_uppercase(), "invalid Excel column");
        column = column
            .checked_mul(26)
            .and_then(|c| c.checked_add(u32::from(b - b'A' + 1)))
            .context("Excel column overflow")?;
    }
    let row: u32 = row.parse()?;
    ensure!(
        (1..=16384).contains(&column) && (1..=1048576).contains(&row),
        "Excel coordinate outside bounds"
    );
    Ok((column, row))
}
pub fn filename(name: &str) -> String {
    let trimmed = name.trim_end_matches([' ', '.']).len();
    let mut out = String::new();
    for (i, c) in name.char_indices() {
        if c < ' ' || "<>:\"/\\|?*%".contains(c) || i >= trimmed {
            out.push_str(&format!("%{:02X}", u32::from(c)))
        } else {
            out.push(c)
        }
    }
    if regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)")
        .unwrap()
        .is_match(&out)
        || out.eq_ignore_ascii_case("assets")
    {
        let first = out.remove(0);
        out = format!("%{:02X}{out}", u32::from(first));
    }
    format!("{out}.yml")
}
fn sheet_parts(parts: &BTreeMap<String, Vec<u8>>) -> Result<Vec<(String, String, String)>> {
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
        let builtin: Value =
            serde_json::from_str(include_str!("../../../contracts/excel-number-formats.json"))?;
        let mut formats: BTreeMap<u32, String> = builtin
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), v.as_str().unwrap().into()))
            .collect();
        let mut styles = vec!["General".to_owned()];
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
            sheets
                .push(json!({"name":name,"part":part,"state":state,"merges":merges,"cells":cells}));
        }
        Ok(Self { raw, parts, sheets })
    }
    pub fn patch(&self, destination: &Path, changes: &[Value]) -> Result<Value> {
        ensure!(
            !self
                .parts
                .keys()
                .any(|n| n.to_lowercase().starts_with("_xmlsignatures/")),
            "signed Excel cannot be modified"
        );
        let mut updates: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
        for change in changes {
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .context("missing writeback sheet")?;
            let cell = string(&change["cell"])?;
            let found = array(&sheet["cells"])?
                .iter()
                .find(|c| c["address"] == cell)
                .context("missing writeback cell")?;
            ensure!(
                found["type"] != "formula" && found["type"] != "error",
                "formula/error cells cannot be overwritten"
            );
            ensure!(
                kind(&change["after"]) != "object",
                "scalar writeback required"
            );
            ensure!(
                change["after"].is_null() || found["type"] == kind(&change["after"]),
                "Excel target type mismatch"
            );
            let (col, row) = coordinate(cell)?;
            for merge in array(&sheet["merges"])? {
                let (a, b) = string(merge)?.split_once(':').unwrap();
                let (c1, r1) = coordinate(a)?;
                let (c2, r2) = coordinate(b)?;
                ensure!(
                    !(c1 <= col && col <= c2 && r1 <= row && row <= r2) || (col, row) == (c1, r1),
                    "merged cell is not top-left"
                );
            }
            ensure!(
                updates
                    .entry(string(&sheet["part"])?.into())
                    .or_default()
                    .insert(cell.into(), change["after"].clone())
                    .is_none(),
                "duplicate writeback target"
            );
        }
        let recalc = !changes.is_empty()
            && self.sheets.iter().any(|s| {
                s["cells"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c["type"] == "formula")
            });
        let mut patched = BTreeMap::new();
        let type_attribute = regex::Regex::new(r#"\s+t\s*=\s*(?:"[^"]*"|'[^']*')"#)?;
        for s in &self.sheets {
            let part = string(&s["part"])?;
            let original = std::str::from_utf8(&self.parts[part])?;
            let doc = Document::parse(original)?;
            let mut edits = vec![];
            let mut seen = BTreeSet::new();
            for cell in doc.descendants().filter(|n| {
                n.has_tag_name((NS, "c")) && n.parent().is_some_and(|p| p.has_tag_name((NS, "row")))
            }) {
                let address = cell.attribute("r").context("missing address")?;
                if let Some(value) = updates.get(part).and_then(|u| u.get(address)) {
                    seen.insert(address.to_owned());
                    let range = cell.range();
                    let raw = &original[range.clone()];
                    let end = raw.find('>').context("invalid cell")?;
                    let tag = raw[1..]
                        .split([' ', '\t', '\r', '\n', '/', '>'])
                        .next()
                        .unwrap();
                    let prefix = tag.strip_suffix('c').unwrap();
                    let opening = type_attribute
                        .replace_all(&raw[..end], "")
                        .trim_end_matches('/')
                        .to_owned();
                    let body = match value {
                        Value::Null => ">".to_owned(),
                        Value::String(text) => {
                            ensure!(
                                text.encode_utf16().count() <= 32767
                                    && !text
                                        .chars()
                                        .any(|c| c < ' ' && !matches!(c, '\t' | '\n' | '\r')),
                                "unsupported Excel string"
                            );
                            let text = text
                                .replace('&', "&amp;")
                                .replace('<', "&lt;")
                                .replace('>', "&gt;")
                                .replace('\r', "&#13;");
                            format!(
                                " t=\"inlineStr\"><{prefix}is><{prefix}t xml:space=\"preserve\">{text}</{prefix}t></{prefix}is>"
                            )
                        }
                        Value::Bool(b) => {
                            format!(" t=\"b\"><{prefix}v>{}</{prefix}v>", if *b { 1 } else { 0 })
                        }
                        Value::Number(n) => format!(" t=\"n\"><{prefix}v>{n}</{prefix}v>"),
                        _ => bail!("scalar required"),
                    };
                    edits.push((range, format!("{opening}{body}</{tag}>")));
                } else if recalc
                    && child(cell, "f").is_some()
                    && let Some(cache) = child(cell, "v")
                {
                    edits.push((cache.range(), String::new()));
                }
            }
            if let Some(wanted) = updates.get(part) {
                ensure!(
                    seen == wanted.keys().cloned().collect(),
                    "writeback cell missing"
                );
            }
            if !edits.is_empty() {
                edits.sort_by_key(|(range, _)| range.start);
                let mut result = original.to_owned();
                for (range, replacement) in edits.into_iter().rev() {
                    result.replace_range(range, &replacement)
                }
                patched.insert(part.to_owned(), result.into_bytes());
            }
        }
        if recalc {
            let original = std::str::from_utf8(&self.parts["xl/workbook.xml"])?;
            let doc = Document::parse(original)?;
            let root = doc.root_element();
            let start = &original[root.range().start + 1..];
            let tag = start.split([' ', '>', '\n', '\r', '\t']).next().unwrap();
            let prefix = tag.strip_suffix("workbook").unwrap();
            let calc = format!(
                "<{prefix}calcPr calcId=\"0\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\" calcMode=\"auto\"/>"
            );
            let mut result = original.to_owned();
            if let Some(old) = child(root, "calcPr") {
                result.replace_range(old.range(), &calc)
            } else {
                let pos = root
                    .children()
                    .find(|n| {
                        n.is_element()
                            && [
                                "oleSize",
                                "customWorkbookViews",
                                "pivotCaches",
                                "smartTagPr",
                                "smartTagTypes",
                                "webPublishing",
                                "fileRecoveryPr",
                                "webPublishObjects",
                                "extLst",
                            ]
                            .contains(&n.tag_name().name())
                    })
                    .map(|n| n.range().start)
                    .unwrap_or_else(|| original.rfind("</").unwrap());
                result.insert_str(pos, &calc)
            }
            patched.insert("xl/workbook.xml".into(), result.into_bytes());
        }
        let mut source = ZipArchive::new(Cursor::new(&self.raw))?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut output = ZipWriter::new(file);
        output.set_raw_comment(source.comment().to_vec().into())?;
        for i in 0..source.len() {
            let entry = source.by_index(i)?;
            if let Some(bytes) = patched.get(entry.name()) {
                let mut options =
                    SimpleFileOptions::default().compression_method(entry.compression());
                if let Some(time) = entry.last_modified() {
                    options = options.last_modified_time(time)
                }
                if let Some(mode) = entry.unix_mode() {
                    options = options.unix_permissions(mode)
                }
                output.start_file(entry.name(), options)?;
                output.write_all(bytes)?;
            } else {
                output.raw_copy_file(entry)?;
            }
        }
        output.finish()?.sync_all()?;
        let reread = Self::open(destination)?;
        for change in changes {
            let found = reread
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .and_then(|s| {
                    s["cells"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["address"] == change["cell"])
                })
                .map(|c| &c["value"])
                .unwrap_or(&Value::Null);
            ensure!(
                found == &change["after"]
                    || (found.is_number()
                        && change["after"].is_number()
                        && found.as_f64() == change["after"].as_f64()),
                "Excel read-back failed"
            );
        }
        Ok(
            json!({"changed_parts":patched.keys().collect::<Vec<_>>(),"requires_excel_recalculation":recalc}),
        )
    }
}
