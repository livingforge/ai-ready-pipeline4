//! A page that shows the content of a document laid out as its original lays
//! it out (`documents view`), for a person to read at a glance: each value
//! the content holds is drawn where the original shows it, and a value or
//! font the content changes is marked with the original's. The page is
//! written to `.arp/view/`, which is not tracked; nothing else changes.
use super::*;
use crate::view::{
    Addition, CellStructure, CheckboxStructure, Content, DiagramStructure, Shown, Structure,
    TableStructure,
};

impl Store {
    /// Writes the view of the adopted document `id`, or of its proposal, and
    /// returns where it is, with the number of its pages and of the values
    /// and fonts the content changes.
    pub fn view(&self, id: &str, proposal: bool) -> Result<Value> {
        let dir = self.structure_dir(id, proposal)?;
        let inspected = self.inspect(&dir, false)?;
        ensure!(
            inspected.source_current,
            "source changed/missing; re-import before view"
        );
        let source_path = string(&inspected.meta["source"]["path"])?;
        let original = Source::open(&under(&self.root, source_path)?)?;
        let content = shown(&inspected)?;
        let structure = self.readings(id, proposal, &dir, &inspected, &content)?;
        let page = crate::view::page(&crate::view::Document {
            id,
            source: source_path,
            proposal,
            original: &original,
            extraction: &inspected.extraction,
            content: &content,
            assets: &dir.join("assets"),
            operations: array(&inspected.mappings["operations"])
                .map(Vec::len)
                .unwrap_or(0),
            structure: &structure,
        })?;
        let out = self.view_path(id, proposal)?;
        // A project made before views had their own folder ignores it from here on.
        crate::project::ensure_lines(&under(&self.arp, ".gitignore")?, &["/view/"])?;
        replace_cache(&out, page.html.as_bytes())?;
        Ok(json!({"state":"saved","view":out,"document_id":id,
            "pages":inspected.extraction.sheets.len(),"differences":page.differences}))
    }

    /// Removes the views a removed proposal of `id` leaves behind, and with
    /// `adopted` the view of its adopted document too, with the folders they
    /// leave empty.
    pub(super) fn remove_views(&self, id: &str, adopted: bool) -> Result<()> {
        for proposal in [true, false].into_iter().filter(|p| *p || adopted) {
            let path = self.view_path(id, proposal)?;
            match fs::remove_file(&path) {
                Ok(()) => remove_empty_directories(path.parent().unwrap(), &self.arp),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("remove the view {}", path.display()));
                }
            }
        }
        Ok(())
    }

    fn view_path(&self, id: &str, proposal: bool) -> Result<PathBuf> {
        under(
            &self.arp,
            &format!("view/{}{id}.html", if proposal { "proposal/" } else { "" }),
        )
    }
}

impl Store {
    /// How each sheet of the document in `dir` is read, by sheet name: the
    /// tables, texts, diagrams and check boxes its structure interpretation
    /// finds, and where the body puts each cell of a table.
    fn readings(
        &self,
        id: &str,
        proposal: bool,
        dir: &Path,
        inspected: &Inspection,
        content: &Content,
    ) -> Result<BTreeMap<String, Structure>> {
        let (layout, interpretation) = self.interpreted_layout(id, proposal)?;
        let elements = elements_view(id, proposal, &layout, &interpretation)?.into_value()?;
        let mut values = HashMap::new();
        for sheet in &inspected.extraction.sheets {
            for cell in &sheet.cells {
                values.insert(
                    (
                        string(&sheet["name"])?.to_owned(),
                        cell.address().to_owned(),
                    ),
                    cell.value().to_json(),
                );
            }
        }
        let mut structures = BTreeMap::new();
        for (page_id, page) in elements["pages"].as_object().into_iter().flatten() {
            let name = string(&page["name"])?;
            let typed = layout.sheet(page_id)?;
            // A cell by the row and column the extraction gives it; none for
            // one an operation inserts.
            let address = |cell: &str| -> Option<String> {
                let (row, column) = elements::cell_axes(cell)?;
                let row: u32 = typed.rows.key(&row)?.strip_prefix('r')?.parse().ok()?;
                Some(format!("{}{row}", typed.columns.key(&column)?))
            };
            let value_of = |cell: &str| -> Value {
                let Some(at) = address(cell) else {
                    return Value::Null;
                };
                content
                    .cells
                    .get(&(name.to_owned(), at.clone()))
                    .map(|shown| shown.value.clone())
                    .or_else(|| values.get(&(name.to_owned(), at)).cloned())
                    .unwrap_or(Value::Null)
            };
            // The elements of the body, to number those it holds.
            let body = inspected
                .page_files
                .get(page_id)
                .and_then(|file| fs::read_to_string(dir.join(file)).ok())
                .and_then(|text| parse(&text, false).ok())
                .unwrap_or(Value::Null);
            let body = body["elements"].as_array().cloned().unwrap_or_default();
            let places = |kind: &str| -> Vec<usize> {
                (0..body.len())
                    .filter(|i| body[*i].get(kind).is_some())
                    .collect()
            };
            let (table_places, diagram_places, checkbox_places) =
                (places("table"), places("diagram"), places("checkbox"));
            let titles = &typed.fields["block_titles"];
            let mut structure = Structure::default();
            for element in page["element_metadata"].as_array().into_iter().flatten() {
                match element["type"].as_str() {
                    Some("table") => {
                        let number = structure.tables.len() + 1;
                        let meta = Meta::from_value(element.clone())?;
                        let read: HashMap<String, crate::document_body::CellReading> =
                            crate::document_body::table_reading(&meta, &value_of)?
                                .into_iter()
                                .map(|r| (r.id.clone(), r))
                                .collect();
                        let mut cells = vec![];
                        for cell in element["rows"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|row| row["cells"].as_array().into_iter().flatten())
                        {
                            let id = string(&cell["id"])?;
                            let Some(at) = address(id) else {
                                continue;
                            };
                            let reading = read.get(id);
                            cells.push(CellStructure {
                                address: at,
                                role: cell["role"].as_str().unwrap_or("unassigned").to_owned(),
                                headers: cell["headers"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|h| address(h.as_str()?))
                                    .collect(),
                                shown: reading.is_some(),
                                heading: reading.is_some_and(|r| r.heading),
                                key: reading.and_then(|r| r.key.clone()),
                                mark: reading.is_some_and(|r| r.mark),
                                category: reading.is_some_and(|r| r.category),
                                description: reading.and_then(|r| r.description).map(|d| d.name()),
                                nodes: reading.map(|r| r.nodes.clone()).unwrap_or_default(),
                            });
                        }
                        structure.tables.push(TableStructure {
                            number,
                            element: table_places.get(number - 1).copied(),
                            title: titles[format!("table-{number}")]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                            headings: element["headings"].as_u64().unwrap_or(0) as usize,
                            cells,
                        });
                    }
                    Some("text") if element.get("visual").is_none() => {
                        if let Some(at) = element["id"].as_str().and_then(address) {
                            structure.texts.push(at);
                        }
                    }
                    _ => {}
                }
            }
            for (number, &place) in diagram_places.iter().enumerate() {
                let diagram = &body[place]["diagram"];
                structure.diagrams.push(DiagramStructure {
                    number: number + 1,
                    element: place,
                    lines: diagram["lines"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|line| match line.as_str() {
                            Some(text) => text.to_owned(),
                            None => format!(
                                "{}（{}）",
                                line["line"].as_str().unwrap_or_default(),
                                plain_text(&line["text"])
                            ),
                        })
                        .collect(),
                    shapes: diagram["shapes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|shape| {
                            let key = shape["id"].as_str()?;
                            Some((
                                key.to_owned(),
                                format!("#{}", key.trim_start_matches('s')),
                                plain_text(&shape["text"]),
                            ))
                        })
                        .collect(),
                });
            }
            for &place in &checkbox_places {
                let checkbox = &body[place]["checkbox"];
                structure.checkboxes.push(CheckboxStructure {
                    element: place,
                    text: plain_text(&checkbox["text"]),
                    checked: checkbox["checked"].clone(),
                    linked_cell: checkbox["linked_cell"].as_str().map(str::to_owned),
                    at: checkbox["at"].as_str().map(str::to_owned),
                });
            }
            structures.insert(name.to_owned(), structure);
        }
        Ok(structures)
    }
}

/// The text of a shape or label as the body writes it: a value, or the list
/// of its paragraphs.
fn plain_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(paragraphs) => paragraphs
            .iter()
            .map(plain_text)
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}

/// The values and fonts the content of `inspected` gives the cells and
/// shapes of its original, each with the original's where it differs.
fn shown(inspected: &Inspection) -> Result<Content> {
    let mut cells = BTreeMap::new();
    for sheet in &inspected.extraction.sheets {
        for cell in &sheet.cells {
            cells.insert((string(&sheet["name"])?, cell.address()), cell);
        }
    }
    let mut shapes = BTreeMap::new();
    for sheet in &inspected.extraction.sheets {
        for drawing in sheet["drawings"].as_array().into_iter().flatten() {
            shapes.insert(
                (string(&sheet["name"])?, string(&drawing["id"])?),
                &drawing["text"],
            );
        }
    }
    // A font the original does not record is not compared.
    let fonts: BTreeSet<(String, String)> = super::value_edit::font_changes(inspected)
        .unwrap_or_default()
        .iter()
        .filter_map(|change| {
            let place = change["cell"].as_str().or(change["shape"].as_str())?;
            Some((change["sheet"].as_str()?.to_owned(), place.to_owned()))
        })
        .collect();
    let mut content = Content::default();
    for (index, entry) in inspected.entries().enumerate() {
        let entry = entry.json();
        let Some(field) = inspected.content.get(index) else {
            continue;
        };
        let value = field.value.clone();
        let font = field.font.clone();
        match (
            entry["writeback"].as_str(),
            mapping_target(&entry["target"])?,
        ) {
            (Some("formula"), Some(MappingTarget::Cell { sheet, cell })) => {
                let before = cells
                    .get(&(sheet, cell))
                    .and_then(|c| c.formula())
                    .map(|f| json!(format!("={f}")));
                let font_changed = fonts.contains(&(sheet.to_owned(), cell.to_owned()));
                content.cells.insert(
                    (sheet.to_owned(), cell.to_owned()),
                    Shown {
                        original: before.filter(|before| *before != value),
                        formula: true,
                        value,
                        font,
                        font_changed,
                    },
                );
            }
            (_, Some(MappingTarget::Cell { sheet, cell })) => {
                let before = cells
                    .get(&(sheet, cell))
                    .map_or(CellValue::Null, |c| c.value());
                let font_changed = fonts.contains(&(sheet.to_owned(), cell.to_owned()));
                content.cells.insert(
                    (sheet.to_owned(), cell.to_owned()),
                    Shown {
                        original: (before != value).then(|| before.to_json()),
                        formula: entry["writeback"] == "new_formula",
                        value,
                        font,
                        font_changed,
                    },
                );
            }
            (_, Some(MappingTarget::Shape { sheet, shape })) => {
                let before = shapes.get(&(sheet, shape)).copied().unwrap_or(&Value::Null);
                let font_changed = fonts.contains(&(sheet.to_owned(), shape.to_owned()));
                content.shapes.insert(
                    (sheet.to_owned(), shape.to_owned()),
                    Shown {
                        original: (*before != value).then(|| before.clone()),
                        formula: false,
                        value,
                        font,
                        font_changed,
                    },
                );
            }
            (
                _,
                Some(MappingTarget::InsertionRow {
                    sheet, insertion, ..
                }),
            )
            | (
                _,
                Some(MappingTarget::InsertionColumn {
                    sheet, insertion, ..
                }),
            ) if !value.is_null() => content.additions.push(Addition {
                sheet: sheet.to_owned(),
                operation: insertion.to_owned(),
                field: entry["field"].as_str().unwrap_or_default().to_owned(),
                value,
            }),
            _ => {}
        }
    }
    Ok(content)
}
