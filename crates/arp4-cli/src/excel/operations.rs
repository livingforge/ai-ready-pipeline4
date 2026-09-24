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

pub fn parse_image_operations(values: &[Value], sheets: &[Value]) -> Result<Vec<ImageOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = vec![];
    for value in values {
        if value["kind"] != "add_image" {
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
        });
    }
    Ok(operations)
}

pub fn parse_operations(values: &[Value], sheets: &[Value]) -> Result<Vec<StructuralOperation>> {
    let sheet_names: BTreeSet<String> = sheets
        .iter()
        .map(|sheet| string(&sheet["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    let mut operations = Vec::with_capacity(values.len());
    for value in values {
        let (id, sheet) = operation_header(value, &sheet_names, &mut ids)?;
        let kind = match string(&value["kind"])? {
            "insert_rows" => OperationKind::InsertRows,
            "delete_rows" => OperationKind::DeleteRows,
            "insert_columns" => OperationKind::InsertColumns,
            "delete_columns" => OperationKind::DeleteColumns,
            "add_image" => continue,
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
            OperationKind::InsertColumns | OperationKind::DeleteColumns
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
        if let Some(style_from) = style_from {
            ensure!(
                (1..=limit).contains(&style_from),
                "style_from outside bounds"
            );
        }
        operations.push(StructuralOperation {
            id,
            sheet,
            kind,
            at,
            count,
            style_from,
        });
    }
    Ok(operations)
}
