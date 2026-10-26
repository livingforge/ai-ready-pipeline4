use super::*;
use std::collections::{HashMap, HashSet};

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

/// How much of a worksheet's cells, in bytes of its XML, is parsed at a time:
/// the tree of a large sheet held several times its text.
const ROWS_PART: usize = 1 << 20;

/// Where the `sheetData` of a worksheet's XML lies, found from its tags alone,
/// so that the rest of the sheet is parsed whole and its rows
/// [`ROWS_PART`] at a time.
pub(crate) struct SheetData<'t> {
    /// The start tags of the root element and of `sheetData`, and their
    /// names: each part of the rows is parsed inside them, as they declare
    /// the namespaces of the rows.
    root_tag: &'t str,
    root_name: &'t str,
    data_tag: &'t str,
    data_name: &'t str,
    /// Where the start tag of `sheetData` begins.
    pub(crate) start: usize,
    /// What `sheetData` holds.
    inner: std::ops::Range<usize>,
    /// Where each element that `sheetData` holds ends, in order.
    ends: Vec<usize>,
}

impl<'t> SheetData<'t> {
    /// The first `sheetData` under the root element of the worksheet XML
    /// `text`; none when the sheet has none or an empty one. Text and
    /// attribute values hold no `<`, so the markup is found by it alone.
    pub(crate) fn of(text: &'t str) -> Result<Option<Self>> {
        let find = |from: usize, pattern: &str| -> Result<usize> {
            text[from..]
                .find(pattern)
                .map(|i| from + i)
                .context("unterminated markup in worksheet XML")
        };
        let mut at = 0;
        let mut depth = 0usize;
        let mut root = None;
        let mut data: Option<(usize, &str, &str, usize)> = None;
        let mut ends = vec![];
        while let Some(i) = text[at..].find('<') {
            let open = at + i;
            let rest = &text[open..];
            if rest.starts_with("<?") {
                at = find(open, "?>")? + 2;
            } else if rest.starts_with("<!--") {
                at = find(open, "-->")? + 3;
            } else if rest.starts_with("<![CDATA[") {
                at = find(open, "]]>")? + 3;
            } else if rest.starts_with("<!") {
                at = find(open, ">")? + 1;
            } else if rest.starts_with("</") {
                at = find(open, ">")? + 1;
                depth = depth.checked_sub(1).context("unbalanced worksheet XML")?;
                if let Some((start, data_tag, data_name, inner)) = data {
                    match depth {
                        1 => {
                            let (root_tag, root_name) =
                                root.context("worksheet XML without root")?;
                            return Ok(Some(Self {
                                root_tag,
                                root_name,
                                data_tag,
                                data_name,
                                start,
                                inner: inner..open,
                                ends,
                            }));
                        }
                        2 => ends.push(at),
                        _ => {}
                    }
                }
            } else {
                // A start tag ends at the first `>` outside quotes.
                let mut quote = None;
                let mut close = None;
                for (j, &byte) in rest.as_bytes().iter().enumerate().skip(1) {
                    match quote {
                        Some(q) if byte == q => quote = None,
                        Some(_) => {}
                        None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                        None if byte == b'>' => {
                            close = Some(open + j);
                            break;
                        }
                        None => {}
                    }
                }
                let close = close.context("unterminated tag in worksheet XML")?;
                let tag = &text[open..=close];
                let empty = tag.ends_with("/>");
                let name = tag[1..]
                    .split(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
                    .next()
                    .unwrap_or_default();
                at = close + 1;
                if depth == 0 {
                    root = Some((tag, name));
                } else if depth == 1
                    && data.is_none()
                    && name.rsplit(':').next() == Some("sheetData")
                {
                    if empty {
                        return Ok(None);
                    }
                    data = Some((open, tag, name, at));
                } else if empty && depth == 2 && data.is_some() {
                    ends.push(at);
                }
                if !empty {
                    depth += 1;
                }
            }
        }
        Ok(None)
    }

    /// The worksheet `text` without what `sheetData` holds.
    pub(crate) fn rest(&self, text: &str) -> String {
        format!("{}{}", &text[..self.inner.start], &text[self.inner.end..])
    }

    /// The rows of `text` as documents of their own, each of at least
    /// [`ROWS_PART`] bytes of whole elements but the last, inside the start
    /// tags of the root and of `sheetData`.
    pub(crate) fn parts(&self, text: &str) -> Vec<String> {
        let wrap = |range: std::ops::Range<usize>| {
            format!(
                "{}{}{}</{}></{}>",
                self.root_tag, self.data_tag, &text[range], self.data_name, self.root_name
            )
        };
        let mut parts = vec![];
        let mut start = self.inner.start;
        for &end in &self.ends {
            if end - start >= ROWS_PART {
                parts.push(start..end);
                start = end;
            }
        }
        parts.push(start..self.inner.end);
        parts.into_iter().map(wrap).collect()
    }
}

/// The sizes of the columns and rows of `worksheet`, which place the cells
/// on the sheet: the default width of a column (in characters of the
/// normal font's digits, padding included, as Excel stores widths) and
/// height of a row (in points), and the columns (`[min, max, width]`) and
/// the rows (`[first, last, height]`, `heights` given by row) of other sizes,
/// adjacent ones of the same size as one run.
fn grid(worksheet: Node<'_, '_>, heights: &[(u32, f64)]) -> Value {
    let format = child(worksheet, "sheetFormatPr");
    let number = |name: &str| {
        format
            .and_then(|f| f.attribute(name))
            .and_then(|v| v.parse::<f64>().ok())
    };
    // Excel's default column holds eight digits of the normal font, and its
    // width adds the padding to them, as a column's width does.
    let column_width = number("defaultColWidth")
        .or_else(|| number("baseColWidth").map(|base| base + 1.140625))
        .unwrap_or(9.140625);
    let row_height = number("defaultRowHeight").unwrap_or(15.0);
    // A run of each size, which `add` extends by the lines after its last.
    let add =
        |runs: &mut Vec<(u32, u32, f64)>, first: u32, last: u32, size: f64| match runs.last_mut() {
            Some((_, end, run)) if *end + 1 == first && *run == size => *end = last,
            _ => runs.push((first, last, size)),
        };
    let mut columns = vec![];
    for col in worksheet
        .children()
        .filter(|n| n.has_tag_name((NS, "cols")))
        .flat_map(|n| n.children())
        .filter(|n| n.has_tag_name((NS, "col")))
    {
        let size = || -> Option<(u32, u32, f64)> {
            let min = col.attribute("min")?.parse::<u32>().ok()?;
            let max = col.attribute("max")?.parse::<u32>().ok()?.min(16_384);
            Some((min, max, col.attribute("width")?.parse::<f64>().ok()?))
        };
        if let Some((min, max, width)) = size() {
            add(&mut columns, min, max, width);
        }
    }
    let mut rows = vec![];
    for &(row, height) in heights {
        add(&mut rows, row, row, height);
    }
    let runs = |runs: Vec<(u32, u32, f64)>| -> Vec<Value> {
        runs.into_iter()
            .map(|(first, last, size)| json!([first, last, size]))
            .collect()
    };
    json!({"column_width":column_width,"row_height":row_height,"columns":runs(columns),"rows":runs(rows)})
}

/// The cells of a worksheet as its rows are read, a part of them at a time.
struct Rows<'w> {
    sheet: &'w str,
    shared: &'w [(String, bool, Vec<crate::fonts::RunFont>)],
    palette: &'w fonts::Palette,
    cell_formats: &'w [CellFormat],
    date1904: bool,
    rich_values: &'w mut Vec<String>,
    cells: Vec<Cell>,
    /// The places of the cells read, by column and row: a large sheet has a
    /// cell for each of its values, and their addresses took several times
    /// the room.
    places: HashSet<(u32, u32)>,
    /// Border sides by position, and the cells holding nothing.
    sides: HashMap<(u32, u32), [bool; 4]>,
    blanks: Vec<(u32, u32)>,
    filled: Vec<(u32, u32)>,
    hidden_rows: Vec<u32>,
    /// The heights the rows give themselves (`ht`), in points, by row.
    heights: Vec<(u32, f64)>,
    /// The number of the last row read, which a row without one follows.
    last_row: u32,
    /// The masters of shared formulas by group index: (column, row, formula).
    masters: BTreeMap<String, (u32, u32, String)>,
    /// The cells whose formula a shared formula's master gives, by their
    /// place in `cells`, with their group: given once every master is read,
    /// as the last master of a group is the one taken.
    members: Vec<(usize, String)>,
    /// The ranges array formulas and data tables fill, in order.
    computed: Vec<Value>,
}

impl Rows<'_> {
    /// Reads the cells of the rows `data` holds, a `sheetData` element.
    fn read(&mut self, data: Node<'_, '_>) -> Result<()> {
        for cell in data.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
            let Some(formula) = child(cell, "f") else {
                continue;
            };
            if formula.attribute("t") == Some("shared")
                && formula.attribute("ref").is_some()
                && let Some(text) = formula.text()
            {
                let (column, row) =
                    coordinate(cell.attribute("r").context("missing cell address")?)?;
                self.masters.insert(
                    formula
                        .attribute("si")
                        .context("shared formula without si")?
                        .to_owned(),
                    (column, row, text.to_owned()),
                );
            }
        }
        self.computed.extend(computed_formulas(data.descendants())?);
        let hidden = |n: &Node<'_, '_>| matches!(n.attribute("hidden"), Some("1" | "true"));
        for row in data.children().filter(|n| n.has_tag_name((NS, "row"))) {
            if hidden(&row)
                && let Some(number) = row.attribute("r")
            {
                self.hidden_rows.push(number.parse::<u32>()?);
            }
            self.last_row = match row.attribute("r") {
                Some(number) => number.parse::<u32>()?,
                None => self.last_row + 1,
            };
            if let Some(height) = row.attribute("ht").and_then(|h| h.parse::<f64>().ok()) {
                self.heights.push((self.last_row, height));
            }
            for c in row.children().filter(|n| n.has_tag_name((NS, "c"))) {
                let address = c.attribute("r").context("missing cell address")?;
                // An address names one place, which no other address names.
                let position = coordinate(address)?;
                ensure!(self.places.insert(position), "duplicate Excel cell");
                let formula = child(c, "f");
                let mut member = None;
                let formula_text = match formula {
                    Some(f) => Some(match f.text() {
                        Some(text) => text.to_owned(),
                        None if f.attribute("t") == Some("shared") => {
                            member = Some(
                                f.attribute("si")
                                    .context("shared formula without si")?
                                    .to_owned(),
                            );
                            String::new()
                        }
                        None => String::new(),
                    }),
                    None => None,
                };
                let raw_value = child(c, "v").and_then(|n| n.text()).unwrap_or("");
                // A picture placed in the cell or a linked data type keeps
                // its value in rich data; the cell holds only `#VALUE!`.
                if c.attribute("vm").is_some() {
                    self.rich_values.push(format!("{}!{address}", self.sheet));
                }
                let mut bold_text = false;
                let mut runs = vec![];
                let (kind, value) = match c.attribute("t").unwrap_or("n") {
                    "inlineStr" => {
                        bold_text = child(c, "is").is_some_and(bold_runs);
                        runs = child(c, "is")
                            .map(|is| self.palette.runs(is))
                            .unwrap_or_default();
                        ("string", json!(texts(c)))
                    }
                    _ if raw_value.is_empty() => ("null", Value::Null),
                    "s" => {
                        let (text, bold, shared_runs) = self
                            .shared
                            .get(raw_value.parse::<usize>()?)
                            .context("invalid shared string index")?;
                        bold_text = *bold;
                        runs = shared_runs.clone();
                        ("string", json!(text))
                    }
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
                    "d" => match iso_date_serial(raw_value, self.date1904) {
                        Some(serial) if serial.fract() == 0.0 => ("number", json!(serial as i64)),
                        Some(serial) => ("number", json!(serial)),
                        None => ("string", json!(raw_value)),
                    },
                    _ => ("string", json!(decode_xstring(raw_value))),
                };
                let format = c.attribute("s").unwrap_or("0").parse::<usize>()?;
                ensure!(format < self.cell_formats.len(), "invalid style index");
                if self.cell_formats[format].sides.iter().any(|side| *side) {
                    self.sides.insert(position, self.cell_formats[format].sides);
                }
                if value.is_null() && formula.is_none() {
                    self.blanks.push(position);
                    continue;
                }
                self.filled.push(position);
                if let Some(group) = member {
                    self.members.push((self.cells.len(), group));
                }
                self.cells.push(Cell {
                    address: address.into(),
                    kind: if formula.is_some() {
                        crate::extraction::CellKind::Formula
                    } else {
                        crate::extraction::CellKind::parse(kind)?
                    },
                    value,
                    formula: formula_text.map(String::into_boxed_str),
                    format: u32::try_from(format)?,
                    bold_text,
                    runs: runs.into_boxed_slice(),
                });
            }
        }
        Ok(())
    }

    /// Gives the members of shared formulas the formula of their master:
    /// Excel derives it by moving relative references.
    fn share_formulas(&mut self) -> Result<()> {
        for (index, group) in std::mem::take(&mut self.members) {
            let (master_column, master_row, text) = self
                .masters
                .get(&group)
                .context("shared formula master missing")?;
            let cell = &mut self.cells[index];
            let (column, row) = coordinate(&cell.address)?;
            cell.formula = Some(
                references::shift_relative(
                    text,
                    i64::from(row) - i64::from(*master_row),
                    i64::from(column) - i64::from(*master_column),
                )?
                .into_boxed_str(),
            );
        }
        Ok(())
    }
}

/// The ranges that the array formulas and data tables below `node` fill, in
/// order (see [`computed_ranges`]).
fn computed_formulas<'a, 'i: 'a>(nodes: impl Iterator<Item = Node<'a, 'i>>) -> Result<Vec<Value>> {
    let mut ranges = vec![];
    for formula in nodes.filter(|n| n.has_tag_name((NS, "f"))) {
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
    Ok(ranges)
}

impl Workbook {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_bytes(fs::read(path)?)
    }
    pub fn from_bytes(raw: Vec<u8>) -> Result<Self> {
        let package = crate::document_source::office_package(&raw)?;
        let mut archive = ZipArchive::new(Cursor::new(&*package))?;
        ensure!(
            archive.len() <= 10000
                && crate::document_source::uncompressed_size(&mut archive)? <= 512 * 1024 * 1024,
            "Excel archive exceeds size budget"
        );
        // ZipArchive indexes by name; inspect the directory before duplicate entries can be hidden.
        let mut offset = usize::try_from(archive.central_directory_start())?;
        let mut central_names = BTreeSet::new();
        while package.get(offset..offset + 4) == Some(b"PK\x01\x02") {
            let header = package
                .get(offset..offset + 46)
                .context("truncated ZIP directory")?;
            let length = |i| usize::from(u16::from_le_bytes([header[i], header[i + 1]]));
            let name_length = length(28);
            let next = offset + 46 + name_length + length(30) + length(32);
            ensure!(next <= package.len(), "truncated ZIP directory");
            ensure!(
                central_names.insert(package[offset + 46..offset + 46 + name_length].to_vec()),
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
            let data = crate::document_source::read_entry(&mut entry)?;
            ensure!(
                parts.insert(name, data).is_none(),
                "duplicate Excel ZIP members"
            );
        }
        let styles = parts
            .get("xl/styles.xml")
            .map(|bytes| xml(bytes))
            .transpose()?;
        let palette = fonts::Palette::new(fonts::workbook_theme(&parts)?, styles.as_ref());
        let mut shared = vec![];
        if let Some(bytes) = parts.get("xl/sharedStrings.xml") {
            let doc = xml(bytes)?;
            for item in doc.root_element().children().filter(Node::is_element) {
                shared.push((texts(item), bold_runs(item), palette.runs(item)));
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
            appearance: json!({"bold":false,"fill":0,"border":0,"strike":false}),
            font: crate::fonts::RunFont::default(),
            sides: [false; 4],
        }];
        if let Some(doc) = &styles {
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
            // The sides each border draws; a side without a style draws no line.
            let border_sides: Vec<[bool; 4]> = child(doc.root_element(), "borders")
                .map(|n| {
                    n.children()
                        .filter(Node::is_element)
                        .map(|border| {
                            let drawn = |names: &[&str]| {
                                names.iter().any(|name| {
                                    child(border, name).is_some_and(|side| {
                                        side.attribute("style").is_some_and(|s| s != "none")
                                    })
                                })
                            };
                            [
                                drawn(&["left", "start"]),
                                drawn(&["right", "end"]),
                                drawn(&["top"]),
                                drawn(&["bottom"]),
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
            // Fills that paint the cell: a pattern other than none, or a gradient.
            let painted: Vec<bool> = child(doc.root_element(), "fills")
                .map(|n| {
                    n.children()
                        .filter(Node::is_element)
                        .map(|fill| {
                            child(fill, "gradientFill").is_some()
                                || child(fill, "patternFill").is_some_and(|pattern| {
                                    pattern
                                        .attribute("patternType")
                                        .is_some_and(|kind| kind != "none")
                                })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(n) = child(doc.root_element(), "cellXfs") {
                let font_list: Vec<_> = child(doc.root_element(), "fonts")
                    .map(|n| {
                        n.children()
                            .filter(Node::is_element)
                            .map(|f| (on(f, "b"), palette.font(f, false)))
                            .collect()
                    })
                    .unwrap_or_default();
                let style_fonts: Vec<usize> = child(doc.root_element(), "cellStyleXfs")
                    .map(|n| {
                        n.children()
                            .filter(Node::is_element)
                            .map(|xf| {
                                xf.attribute("fontId")
                                    .and_then(|id| id.parse().ok())
                                    .unwrap_or(0)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // A font without a name or size, as some writers save one, shows
                // those of its cell style's font, then of the workbook's default font.
                let inherited = |xf: Node<'_, '_>| {
                    let style = xf
                        .attribute("xfId")
                        .and_then(|id| id.parse::<usize>().ok())
                        .and_then(|id| style_fonts.get(id).copied())
                        .unwrap_or(0);
                    let font = |index: usize| {
                        font_list
                            .get(index)
                            .map(|(_, font)| font.clone())
                            .unwrap_or_default()
                    };
                    font(style).or(&font(0))
                };
                cell_formats = n
                    .children()
                    .filter(Node::is_element)
                    .map(|f| {
                        let font = f.attribute("fontId").unwrap_or("0").parse::<usize>()?;
                        let (bold, font) = font_list.get(font).cloned().unwrap_or_default();
                        let font = font.or(&inherited(f));
                        let border = f.attribute("borderId").unwrap_or("0").parse::<u32>()?;
                        let sides = border_sides
                            .get(border as usize)
                            .copied()
                            .unwrap_or_default();
                        let fill = f.attribute("fillId").unwrap_or("0").parse::<u32>()?;
                        // A border drawing no side or a fill painting nothing looks
                        // like none, as writers save such formats for plain cells.
                        // Each cell's runs tell whether its text is struck through.
                        let mut appearance = json!({"bold":bold,
                        "fill":if painted.get(fill as usize).copied().unwrap_or(true) { fill } else { 0 },
                        "border":if border_sides.get(border as usize).is_none() || sides.contains(&true) { border } else { 0 },
                        "strike":false});
                        let indent = child(f, "alignment")
                            .and_then(|alignment| alignment.attribute("indent"))
                            .and_then(|level| level.parse::<u32>().ok())
                            .unwrap_or(0);
                        if indent > 0 {
                            appearance["indent"] = json!(indent);
                        }
                        let number_format = formats
                            .get(&f.attribute("numFmtId").unwrap_or("0").parse()?)
                            .cloned()
                            .unwrap_or_else(|| "General".into());
                        Ok(CellFormat {
                            number_format,
                            appearance,
                            font,
                            sides,
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
            let text = std::str::from_utf8(&parts[&part])?;
            // The sheet is parsed without its cells, and its rows a part at a time.
            let data = SheetData::of(text)?;
            let rest = data.as_ref().map(|data| data.rest(text));
            let doc = Document::parse(rest.as_deref().unwrap_or(text))?;
            ensure!(
                doc.root_element().has_tag_name((NS, "worksheet")),
                "invalid worksheet namespace"
            );
            let hidden = |n: &Node<'_, '_>| matches!(n.attribute("hidden"), Some("1" | "true"));
            let mut hidden_columns = vec![];
            for columns in doc
                .root_element()
                .children()
                .filter(|n| n.has_tag_name((NS, "cols")))
                .flat_map(|n| n.children())
                .filter(|n| n.has_tag_name((NS, "col")) && hidden(n))
            {
                let bound = |name: &str| -> Result<u32> {
                    Ok(columns
                        .attribute(name)
                        .context("missing column bound")?
                        .parse()?)
                };
                // A width set for the rest of the sheet ends at the last column.
                for column in bound("min")?..=bound("max")?.min(16_384) {
                    hidden_columns.push(column_name(column)?);
                }
            }
            let mut rows = Rows {
                sheet: &name,
                shared: &shared,
                palette: &palette,
                cell_formats: &cell_formats,
                date1904,
                rich_values: &mut rich_values,
                cells: vec![],
                places: HashSet::new(),
                sides: HashMap::new(),
                blanks: vec![],
                filled: vec![],
                hidden_rows: vec![],
                heights: vec![],
                last_row: 0,
                masters: BTreeMap::new(),
                members: vec![],
                computed: vec![],
            };
            // Where the cells were: the formulas around them come before and after theirs.
            let mut cells_at = usize::MAX;
            if let Some(sheet_data) = child(doc.root_element(), "sheetData") {
                match &data {
                    Some(data) => {
                        ensure!(
                            sheet_data.range().start == data.start,
                            "the sheetData of {part} is not the worksheet's"
                        );
                        cells_at = data.start;
                        for rows_part in data.parts(text) {
                            let part_doc = Document::parse(&rows_part)?;
                            rows.read(
                                child(part_doc.root_element(), "sheetData")
                                    .context("worksheet rows missing")?,
                            )?;
                        }
                    }
                    None => rows.read(sheet_data)?,
                }
            }
            rows.share_formulas()?;
            let around = |before: bool| {
                computed_formulas(
                    doc.root_element()
                        .descendants()
                        .filter(|n| (n.range().start < cells_at) == before),
                )
            };
            let mut formulas = around(true)?;
            formulas.append(&mut rows.computed);
            formulas.append(&mut around(false)?);
            let Rows {
                cells,
                sides,
                blanks,
                filled,
                hidden_rows,
                heights,
                ..
            } = rows;
            let mut merges = child(doc.root_element(), "mergeCells")
                .map(|n| {
                    n.children()
                        .filter(Node::is_element)
                        .map(|m| {
                            let range = m.attribute("ref").context("missing merge reference")?;
                            let (a, b) = range.split_once(':').context("invalid merge range")?;
                            let (c1, r1) = coordinate(a)?;
                            let (c2, r2) = coordinate(b)?;
                            ensure!(c1 <= c2 && r1 <= r2, "reversed merge range");
                            Ok(((r1, c1, r2, c2), range.to_owned()))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            // Excel rewrites mergeCells in its own order on save; keep the
            // extraction independent of it so a save alone changes nothing.
            merges.sort();
            let merges: Vec<String> = merges.into_iter().map(|(_, range)| range).collect();
            let drawings = visuals::extract_visuals(
                &parts,
                &part,
                doc.root_element(),
                palette.theme.as_ref(),
            )?;
            let tables = visuals::extract_tables(&parts, &part, doc.root_element())?;
            let comments = visuals::extract_comments(&parts, &persons, &part)?;
            let computed = computed_ranges(&parts, &part, formulas)?;
            let mut sheet = json!({"name":name,"part":part,"state":state,"hidden_rows":hidden_rows,"hidden_columns":hidden_columns,"merges":merges,"drawings":drawings,"tables":tables,"comments":comments,"computed":computed});
            // Drawings are placed on the cells, which these sizes place on the sheet.
            if sheet["drawings"].as_array().is_some_and(|d| !d.is_empty()) {
                sheet["grid"] = grid(doc.root_element(), &heights);
            }
            let boxes = bordered_blanks(&blanks, &filled, &sides, &merges)?;
            if !boxes.is_empty() {
                sheet["bordered_blanks"] = json!(boxes);
            }
            sheets.push(sheet);
            sheet_cells.push(crate::data::exact(cells));
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
/// value written there. `formulas` are the ranges of the array formulas and
/// data tables, in the order of the sheet; those of pivot tables follow.
fn computed_ranges(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
    formulas: Vec<Value>,
) -> Result<Vec<Value>> {
    let mut ranges = formulas;
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

/// Empty cells and empty merged ranges drawn as boxes: ranges holding no value
/// or formula whose four sides all show a line, drawn by the cell or by the
/// neighbour across that side, or three when the fourth faces no cell, as the
/// bottom of a table's last row left open. The top and bottom of an empty cell
/// are those of the box its column draws around it, the rows with no line
/// between them, as a field under a heading with no line between or in a
/// column whose rows are not ruled apart. An empty field of a ruled table is
/// one; the edge of a frame drawn around tables shows one or two sides and is
/// not.
/// So are the empty cells of a band holding values written without merging,
/// which spill over them, such as a name after its marker: lines run along
/// the top and bottom of its boxes, which may span rows with no line between
/// them, and close it at one end at least, with no line between its cells
/// (`filled`: cells with a value).
/// Single cells are joined into rectangles; the ranges are in row order.
fn bordered_blanks(
    blanks: &[(u32, u32)],
    filled: &[(u32, u32)],
    sides: &HashMap<(u32, u32), [bool; 4]>,
    merges: &[String],
) -> Result<Vec<String>> {
    let side =
        |position: (u32, u32), index: usize| sides.get(&position).is_some_and(|drawn| drawn[index]);
    let mut merged: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut covered = HashSet::new();
    for range in merges {
        let (a, b) = range.split_once(':').context("invalid merge range")?;
        let (start, end) = (coordinate(a)?, coordinate(b)?);
        merged.insert(start, end);
        for column in start.0..=end.0 {
            for row in start.1..=end.1 {
                if (column, row) != start {
                    covered.insert((column, row));
                }
            }
        }
    }
    // Whether a line runs along the top, or the bottom, of the cell.
    let over =
        |(column, row): (u32, u32)| side((column, row), 2) || row > 1 && side((column, row - 1), 3);
    let under = |(column, row): (u32, u32)| side((column, row), 3) || side((column, row + 1), 2);
    let written: HashSet<(u32, u32)> = blanks.iter().chain(filled).copied().collect();
    // The box of each written cell: the rows of its column written one under
    // another with no line between them, as a record drawn over two rows with
    // no line across it.
    let mut boxes: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    let mut cells: Vec<(u32, u32)> = written.iter().copied().collect();
    cells.sort_unstable();
    let mut start = 0;
    for k in 1..=cells.len() {
        let joined = cells.get(k).is_some_and(|&(column, row)| {
            row > 1 && cells[k - 1] == (column, row - 1) && !over((column, row))
        });
        if !joined {
            let span = (cells[start].1, cells[k - 1].1);
            boxes.extend(cells[start..k].iter().map(|&p| (p, span)));
            start = k;
        }
    }
    // The rows of the column drawn together with the cell, no line between
    // them: its own box, or those of the written cells above and below it.
    let column_box = |(column, row): (u32, u32)| {
        if let Some(&span) = boxes.get(&(column, row)) {
            return span;
        }
        let top = (row > 1 && !over((column, row)))
            .then(|| boxes.get(&(column, row - 1)))
            .flatten()
            .map_or(row, |span| span.0);
        let bottom = (!under((column, row)))
            .then(|| boxes.get(&(column, row + 1)))
            .flatten()
            .map_or(row, |span| span.1);
        (top, bottom)
    };
    // A line above and below the cell's box, one of them maybe left out
    // where the box faces nothing, as the bottom of a table's last row.
    let ruled = |(column, row): (u32, u32)| {
        let (top, bottom) = column_box((column, row));
        let (above, below) = (over((column, top)), under((column, bottom)));
        let open_above = top == 1 || !written.contains(&(column, top - 1));
        let open_below = !written.contains(&(column, bottom + 1));
        (above || below) && (above || open_above) && (below || open_below)
    };
    let mut ranges = vec![];
    let mut singles = BTreeSet::new();
    for &start in blanks {
        if covered.contains(&start) {
            continue;
        }
        let end = merged.get(&start).copied().unwrap_or(start);
        let ((left, top), (right, bottom)) = (start, end);
        // The top and bottom are those of the box its column draws around it,
        // as a merged field under a heading with no line between.
        let (top_at, bottom_at) = (column_box((left, top)).0, column_box((left, bottom)).1);
        let drawn = [
            side((left, top), 0) || left > 1 && side((left - 1, top), 1),
            side((right, top), 1) || side((right + 1, top), 0),
            over((left, top_at)),
            under((left, bottom_at)),
        ];
        // Beyond each side, cells or nothing drawn: a side may be left open
        // where it faces nothing, as the bottom of a table's last row.
        let beyond = [
            left > 1 && written.contains(&(left - 1, top)),
            written.contains(&(right + 1, top)),
            top_at > 1 && written.contains(&(left, top_at - 1)),
            written.contains(&(left, bottom_at + 1)),
        ];
        let lines = drawn.iter().filter(|d| **d).count();
        let closed = lines == 4 || lines == 3 && (0..4).all(|k| drawn[k] || !beyond[k]);
        if !closed {
            continue;
        }
        if start == end {
            singles.insert((top, left));
        } else {
            ranges.push(((top, left), (bottom, right)));
        }
    }
    let empty: HashSet<_> = blanks
        .iter()
        .copied()
        .filter(|p| !covered.contains(p) && !merged.contains_key(p))
        .collect();
    // Whether a line runs down the right edge of the cell.
    let closes = |(column, row): (u32, u32)| side((column, row), 1) || side((column + 1, row), 0);
    let values: HashSet<_> = filled.iter().copied().collect();
    // A cell of the band: empty or holding a value, neither merged nor covered.
    let along = |p: (u32, u32)| {
        (empty.contains(&p)
            || values.contains(&p) && !covered.contains(&p) && !merged.contains_key(&p))
            && ruled(p)
    };
    // The cells of the bands walked: every cell of a band walks to the same ends.
    let mut walked = HashSet::new();
    for &(column, row) in filled {
        if walked.contains(&(column, row)) || !along((column, row)) {
            continue;
        }
        let mut left = column;
        while left > 1 && !closes((left - 1, row)) && along((left - 1, row)) {
            left -= 1;
        }
        let mut right = column;
        while !closes((right, row)) && along((right + 1, row)) {
            right += 1;
        }
        walked.extend((left..=right).map(|c| (c, row)));
        let opened = if left == 1 {
            side((1, row), 0)
        } else {
            closes((left - 1, row))
        };
        // A line at one end at least: a frame left open on one side still
        // closes its bands at the other, where a row of lines alone does not.
        if opened || closes((right, row)) {
            singles.extend(
                (left..=right)
                    .filter(|c| empty.contains(&(*c, row)))
                    .map(|c| (row, c)),
            );
        }
    }
    // Runs of single cells along a row, then runs of equal rows below each other.
    let mut runs: Vec<(u32, u32, u32)> = vec![];
    for (row, column) in singles {
        match runs.last_mut() {
            Some((r, _, end)) if *r == row && *end + 1 == column => *end = column,
            _ => runs.push((row, column, column)),
        }
    }
    let mut open: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for (row, left, right) in runs {
        match open.get_mut(&(left, right)) {
            Some((_, bottom)) if *bottom + 1 == row => *bottom = row,
            _ => {
                if let Some((top, bottom)) = open.insert((left, right), (row, row)) {
                    ranges.push(((top, left), (bottom, right)));
                }
            }
        }
    }
    for ((left, right), (top, bottom)) in open {
        ranges.push(((top, left), (bottom, right)));
    }
    ranges.sort_unstable();
    ranges
        .into_iter()
        .map(|((top, left), (bottom, right))| {
            Ok(format!(
                "{}{top}:{}{bottom}",
                column_name(left)?,
                column_name(right)?
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_band_of_values_holds_the_empty_cells_between_its_lines() {
        // Row 1 ruled above and below, closed after C1 and after F1: values in
        // A1, B1 and E1, the other cells empty.
        let filled = [(1, 1), (2, 1), (5, 1)];
        let blanks = [(3, 1), (4, 1), (6, 1)];
        let sides: HashMap<(u32, u32), [bool; 4]> = (1..=6)
            .map(|column| ((column, 1), [false, column == 3 || column == 6, true, true]))
            .collect();
        assert_eq!(
            bordered_blanks(&blanks, &filled, &sides, &[]).unwrap(),
            ["C1:D1", "F1:F1"]
        );
    }

    /// The addresses of the cells of every row of `text`, a worksheet, read
    /// from the whole document.
    fn whole(text: &str) -> Vec<String> {
        let doc = Document::parse(text).unwrap();
        let data = child(doc.root_element(), "sheetData").unwrap();
        data.descendants()
            .filter(|n| n.has_tag_name((NS, "c")))
            .map(|c| c.attribute("r").unwrap().to_owned())
            .collect()
    }

    /// The same read from the parts [`SheetData`] cuts the rows into.
    fn parted(text: &str) -> Vec<String> {
        let data = SheetData::of(text).unwrap().unwrap();
        let rest = Document::parse(&data.rest(text))
            .unwrap()
            .input_text()
            .len();
        assert!(rest < text.len());
        data.parts(text)
            .iter()
            .flat_map(|part| {
                let doc = Document::parse(part).unwrap();
                let data = child(doc.root_element(), "sheetData").unwrap();
                data.descendants()
                    .filter(|n| n.has_tag_name((NS, "c")))
                    .map(|c| c.attribute("r").unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn rows_are_found_by_their_tags_whatever_surrounds_them() {
        let text = format!(
            r#"<?xml version="1.0"?><!-- <sheetData> --><x:worksheet xmlns:x="{NS}" a='>'><x:sheetPr/><x:sheetData><x:row r="1"><x:c r="A1" t="inlineStr"><x:is><x:t><![CDATA[<row>]]></x:t></x:is></x:c></x:row><!-- </x:row> --><x:row r="2"/><?pi <x:c?><x:row r="3"><x:c r="B3" s="0"><x:f t="array" ref="B3">"a>b"</x:f></x:c></x:row></x:sheetData><x:cols/></x:worksheet>"#
        );
        assert_eq!(parted(&text), whole(&text));
        assert_eq!(whole(&text), ["A1", "B3"]);
        let data = SheetData::of(&text).unwrap().unwrap();
        assert_eq!(data.ends.len(), 3);
        assert_eq!(&text[data.start..data.start + 13], "<x:sheetData>");
        let rest = data.rest(&text);
        let doc = Document::parse(&rest).unwrap();
        let sheet_data = child(doc.root_element(), "sheetData").unwrap();
        assert_eq!(sheet_data.range().start, data.start);
        assert!(child(doc.root_element(), "cols").is_some());
    }

    #[test]
    fn large_rows_are_cut_into_parts_of_whole_rows() {
        let rows: String = (1..=40_000)
            .map(|r| {
                format!(
                    r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c><c r="B{r}" t="s"><v>0</v></c></row>"#
                )
            })
            .collect();
        let text = format!(r#"<worksheet xmlns="{NS}"><sheetData>{rows}</sheetData></worksheet>"#);
        let data = SheetData::of(&text).unwrap().unwrap();
        assert!(data.parts(&text).len() > 1);
        assert_eq!(parted(&text), whole(&text));
        assert_eq!(whole(&text).len(), 80_000);
    }

    #[test]
    fn a_sheet_without_rows_is_read_whole() {
        for text in [
            format!(r#"<worksheet xmlns="{NS}"><sheetData/></worksheet>"#),
            format!(r#"<worksheet xmlns="{NS}"><dimension ref="A1"/></worksheet>"#),
        ] {
            assert!(SheetData::of(&text).unwrap().is_none());
        }
        let text = format!(r#"<worksheet xmlns="{NS}"><sheetData> </sheetData></worksheet>"#);
        assert_eq!(parted(&text), Vec::<String>::new());
    }
}
