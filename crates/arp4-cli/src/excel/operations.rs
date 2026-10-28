use super::*;

pub(super) fn operation_header(
    value: &Value,
    sheet_names: &BTreeSet<String>,
    ids: &mut BTreeSet<String>,
) -> Result<(String, String)> {
    let id = string(&value["id"])?;
    identifier(id)?;
    ensure!(ids.insert(id.to_owned()), "duplicate Excel operation ID");
    let sheet = string(&value["sheet"])?;
    ensure!(sheet_names.contains(sheet), "unknown operation sheet");
    ensure!(
        !string(&value["reason"])?.trim().is_empty(),
        "operation reason required"
    );
    Ok((id.to_owned(), sheet.to_owned()))
}

pub(super) fn anchor_point(value: &Value, label: &str) -> Result<AnchorPoint> {
    let cell = string(&value["cell"])?;
    let (column, row) = coordinate(cell).with_context(|| format!("invalid {label} cell"))?;
    let column_offset = value["col_offset"].as_u64().unwrap_or(0);
    let row_offset = value["row_offset"].as_u64().unwrap_or(0);
    Ok(AnchorPoint {
        column,
        row,
        column_offset,
        row_offset,
    })
}

pub fn parse_image_operations(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
) -> Result<Vec<ImageOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet.fields()["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = vec![];
    // The row and column operations so far, counted as parse_operations
    // takes them.
    let mut preceding = 0;
    for value in values {
        if value["kind"] != "add_image" {
            preceding += usize::from(structural(value));
            continue;
        }
        let (id, sheet) = operation_header(value, &sheet_names, &mut ids)?;
        let asset = string(&value["asset"])?;
        ensure!(
            asset.starts_with("assets/")
                && !asset.contains('\\')
                && !asset
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == ".."),
            "image asset must be a project assets path"
        );
        let anchor = value["anchor"]
            .as_object()
            .context("image anchor required")?;
        let from = anchor_point(
            anchor.get("from").context("image anchor from required")?,
            "image anchor from",
        )?;
        let to = anchor_point(
            anchor.get("to").context("image anchor to required")?,
            "image anchor to",
        )?;
        let width_positive = to.column > from.column
            || (to.column == from.column && to.column_offset > from.column_offset);
        let height_positive =
            to.row > from.row || (to.row == from.row && to.row_offset > from.row_offset);
        ensure!(
            width_positive && height_positive,
            "image anchor must have positive width and height"
        );
        let name = value["name"].as_str().map(str::to_owned);
        if let Some(name) = &name {
            ensure!(!name.trim().is_empty(), "image name must be nonempty");
        }
        operations.push(ImageOperation {
            id,
            sheet,
            asset: asset.to_owned(),
            name,
            from,
            to,
            preceding,
        });
    }
    Ok(operations)
}

/// Whether `value` is a row or column operation, one parse_operations takes.
pub(super) fn structural(value: &Value) -> bool {
    !crate::document_source::is_presentation_operation(value)
        && !super::is_sheet_operation(value)
        && !matches!(
            value["kind"].as_str(),
            Some("add_image" | "format_cells" | "copy_cells" | "place_drawing")
        )
        && !value["kind"]
            .as_str()
            .is_some_and(|kind| super::cell_objects::KINDS.contains(&kind))
}

/// The `place_drawing` operations of `values`, in the order recorded.
pub fn parse_place_operations(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
) -> Result<Vec<PlaceOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet.fields()["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = vec![];
    let mut preceding = 0;
    for value in values {
        if value["kind"] != "place_drawing" {
            preceding += usize::from(structural(value));
            continue;
        }
        let (id, sheet) = operation_header(value, &sheet_names, &mut ids)?;
        let drawing = string(&value["drawing"])?.to_owned();
        drawing
            .rsplit_once('#')
            .context("drawing ID without its part")?;
        let cell = |name: &str| value[name].as_str().map(coordinate).transpose();
        let (from, to) = (cell("from")?, cell("to")?);
        ensure!(
            from.is_some() == to.is_some(),
            "a drawing is placed by both corners, from and to"
        );
        let offset = |name: &str| -> Option<(u64, u64)> {
            Some((value[name]["col"].as_u64()?, value[name]["row"].as_u64()?))
        };
        let offsets = match (offset("from_offset"), offset("to_offset")) {
            (Some(a), Some(b)) => Some([a, b]),
            (None, None) => None,
            _ => {
                bail!("a drawing is placed in its cells by both corners, from_offset and to_offset")
            }
        };
        if let (Some(from), Some(to)) = (from, to) {
            let [a, b] = offsets.unwrap_or_default();
            ensure!(
                (to.0, b.0) > (from.0, a.0) && (to.1, b.1) > (from.1, a.1),
                "the to corner must lie right of and below the from corner"
            );
        }
        operations.push(PlaceOperation {
            id,
            sheet,
            drawing,
            from,
            to,
            offsets,
            placement: value["placement"].as_str().map(str::to_owned),
            preceding,
        });
    }
    Ok(operations)
}

/// The `format_cells` operations of `values`, in the order recorded.
pub fn parse_format_operations(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
) -> Result<Vec<FormatOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet.fields()["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = vec![];
    let mut preceding = 0;
    for value in values {
        if value["kind"] != "format_cells" && value["kind"] != "copy_cells" {
            preceding += usize::from(structural(value));
            continue;
        }
        let (id, sheet) = operation_header(value, &sheet_names, &mut ids)?;
        let range = string(&value["range"])?;
        Area::parse(range)?;
        let from = value["from"].as_str().map(str::to_owned);
        if let Some(from) = &from {
            coordinate(from).context("invalid format source cell")?;
        }
        let flag = |name: &str| value[name].as_bool();
        let text = |name: &str| value[name].as_str().map(str::to_owned);
        let row_height = match &value["row_height"] {
            Value::Null => None,
            Value::String(text) if text == "auto" => Some(None),
            height => Some(Some(height.as_f64().context("row height must be points")?)),
        };
        let borders = value["borders"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|line| -> Result<BorderLine> {
                Ok(BorderLine {
                    edges: string(&line["edges"])?.to_owned(),
                    style: string(&line["style"])?.to_owned(),
                    color: line["color"].as_str().map(str::to_owned),
                })
            })
            .collect::<Result<_>>()?;
        operations.push(FormatOperation {
            id,
            sheet,
            range: range.to_owned(),
            from,
            alignment: Alignment {
                wrap: flag("wrap"),
                shrink: flag("shrink"),
                horizontal: text("horizontal"),
                vertical: text("vertical"),
            },
            look: CellLook {
                bold: flag("bold"),
                italic: flag("italic"),
                strike: flag("strike"),
                underline: text("underline"),
                fill: text("fill"),
                number_format: text("number_format"),
            },
            borders,
            copy_to: if value["kind"] == "copy_cells" {
                let to = string(&value["to"])?;
                coordinate(to).context("invalid copy destination cell")?;
                Some(to.to_owned())
            } else {
                None
            },
            row_height,
            column_width: value["column_width"].as_f64(),
            merge: flag("merge"),
            preceding,
        });
    }
    Ok(operations)
}

pub fn parse_operations(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
) -> Result<Vec<StructuralOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet.fields()["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = Vec::with_capacity(values.len());
    for value in values {
        // Slide operations name slides, not sheets, and sheet operations add
        // sheets; they are parsed on their own.
        if crate::document_source::is_presentation_operation(value)
            || super::is_sheet_operation(value)
        {
            continue;
        }
        let (id, sheet) = operation_header(value, &sheet_names, &mut ids)?;
        if let Some(operation) = cell_shift(value, &id, &sheet)? {
            operations.push(operation);
            continue;
        }
        let kind = match string(&value["kind"])? {
            "insert_rows" => OperationKind::InsertRows,
            "delete_rows" => OperationKind::DeleteRows,
            "insert_columns" => OperationKind::InsertColumns,
            "delete_columns" => OperationKind::DeleteColumns,
            "move_columns" => OperationKind::MoveColumns {
                to: u32::try_from(value["to"].as_u64().context("move to must be integer")?)?,
            },
            "add_image" | "format_cells" | "copy_cells" | "place_drawing" => continue,
            kind if super::cell_objects::KINDS.contains(&kind) => continue,
            other => bail!("unsupported Excel operation: {other}"),
        };
        let at = u32::try_from(
            value["at"]
                .as_u64()
                .context("operation at must be integer")?,
        )?;
        let count = u32::try_from(
            value["count"]
                .as_u64()
                .context("operation count must be integer")?,
        )?;
        let limit = if matches!(
            kind,
            OperationKind::InsertColumns
                | OperationKind::DeleteColumns
                | OperationKind::MoveColumns { .. }
        ) {
            16384
        } else {
            1048576
        };
        ensure!(
            (1..=limit).contains(&at)
                && (1..=limit).contains(&count)
                && at.checked_add(count - 1).is_some_and(|last| last <= limit),
            "Excel operation range outside bounds"
        );
        let style_from = value["style_from"]
            .as_u64()
            .map(u32::try_from)
            .transpose()?;
        if let OperationKind::MoveColumns { to } = kind {
            ensure!(
                to >= 1 && to.checked_add(count - 1).is_some_and(|last| last <= 16384) && to != at,
                "invalid move_columns destination"
            );
        }
        if let Some(style_from) = style_from {
            ensure!(
                kind == OperationKind::InsertRows,
                "style_from applies to insert_rows only; inserted columns take the format of the column to their left, as in Excel"
            );
            ensure!(
                (1..=limit).contains(&style_from),
                "style_from outside bounds"
            );
        }
        let style_rows = u32::try_from(value["style_rows"].as_u64().unwrap_or(1))?;
        let merges = value["merges"].as_bool().unwrap_or(false);
        if style_rows != 1 || merges {
            let style_from = style_from.context("style_rows and merges need style_from")?;
            ensure!(
                style_rows >= 1
                    && style_from
                        .checked_add(style_rows - 1)
                        .is_some_and(|last| last <= limit),
                "style rows outside bounds"
            );
            ensure!(
                count % style_rows == 0,
                "an insert_rows taking {style_rows} style rows inserts a multiple of {style_rows} rows"
            );
        }
        operations.push(StructuralOperation {
            id,
            sheet,
            kind,
            at,
            count,
            style_from,
            style_rows,
            merges,
            vba_unchanged: value["vba_unchanged"].as_bool().unwrap_or(false),
            band: None,
        });
    }
    Ok(operations)
}

/// An `insert_cells` or `delete_cells` operation as the row or column
/// operation it is within its band: cells shifted down or up are a row
/// insertion or deletion in the range's columns, cells shifted right or left
/// a column one in its rows. None for another kind.
fn cell_shift(value: &Value, id: &str, sheet: &str) -> Result<Option<StructuralOperation>> {
    let insertion = match value["kind"].as_str() {
        Some("insert_cells") => true,
        Some("delete_cells") => false,
        _ => return Ok(None),
    };
    let area = Area::parse(string(&value["range"])?)?;
    let (columns, rows) = (
        area.columns.context("cell range without columns")?,
        area.rows.context("cell range without rows")?,
    );
    let shift = string(&value["shift"])?;
    let (kind, (at, last), band) = match (insertion, shift) {
        (true, "down") => (OperationKind::InsertRows, rows, columns),
        (false, "up") => (OperationKind::DeleteRows, rows, columns),
        (true, "right") => (OperationKind::InsertColumns, columns, rows),
        (false, "left") => (OperationKind::DeleteColumns, columns, rows),
        _ => bail!(
            "{} shifts cells {}",
            if insertion {
                "insert_cells"
            } else {
                "delete_cells"
            },
            if insertion {
                "down or right"
            } else {
                "up or left"
            }
        ),
    };
    Ok(Some(StructuralOperation {
        id: id.to_owned(),
        sheet: sheet.to_owned(),
        kind,
        at,
        count: last - at + 1,
        style_from: None,
        style_rows: 1,
        merges: false,
        vba_unchanged: value["vba_unchanged"].as_bool().unwrap_or(false),
        band: Some(band),
    }))
}

/// The operations of `values` on `sheet` that format, copy, place or set
/// what cells carry on cells that, as the row and column operations recorded
/// after it leave them, reach the rows `rows` of the sheet all those leave:
/// the ID of each, with its cells so left.
pub fn recorded_on_rows(
    values: &[Value],
    sheets: &[impl crate::extraction::Fields],
    sheet: &str,
    rows: (u32, u32),
) -> Result<Vec<(String, String)>> {
    let structural = parse_operations(values, sheets)?;
    // The cells of each operation on the sheet, with the row and column
    // operations recorded before it.
    let mut recorded: Vec<(String, String, usize)> = vec![];
    let corners = |from: (u32, u32), to: (u32, u32)| -> Result<String> {
        Ok(format!(
            "{}{}:{}{}",
            column_name(from.0)?,
            from.1,
            column_name(to.0)?,
            to.1
        ))
    };
    for operation in parse_format_operations(values, sheets)? {
        if operation.sheet != sheet {
            continue;
        }
        if let Some(to) = &operation.copy_to {
            recorded.push((operation.id.clone(), to.clone(), operation.preceding));
        }
        recorded.push((operation.id, operation.range, operation.preceding));
    }
    for operation in parse_cell_object_operations(values, sheets)? {
        if operation.sheet != sheet {
            continue;
        }
        if let CellObjectKind::Copy { to } = &operation.kind {
            recorded.push((operation.id.clone(), to.clone(), operation.preceding));
        }
        recorded.push((operation.id, operation.range, operation.preceding));
    }
    for operation in parse_place_operations(values, sheets)? {
        if let (Some(from), Some(to)) = (operation.from, operation.to)
            && operation.sheet == sheet
        {
            recorded.push((operation.id, corners(from, to)?, operation.preceding));
        }
    }
    for operation in parse_image_operations(values, sheets)? {
        if operation.sheet != sheet {
            continue;
        }
        let range = corners(
            (operation.from.column, operation.from.row),
            (operation.to.column, operation.to.row),
        )?;
        recorded.push((operation.id, range, operation.preceding));
    }
    let mut found = vec![];
    for (id, range, preceding) in recorded {
        let later: Vec<&StructuralOperation> = structural[preceding.min(structural.len())..]
            .iter()
            .filter(|later| later.sheet == sheet)
            .collect();
        let Some(mapped) = map_sqref(&range, &later, false)? else {
            continue;
        };
        for part in mapped.split_whitespace() {
            if Area::parse(part)?
                .rows
                .is_none_or(|(first, last)| first <= rows.1 && rows.0 <= last)
            {
                found.push((id, mapped));
                break;
            }
        }
    }
    Ok(found)
}
