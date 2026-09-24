use super::*;

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
pub fn column_number(name: &str) -> Result<u32> {
    coordinate(&format!("{name}1")).map(|(column, _)| column)
}
pub fn column_name(mut column: u32) -> Result<String> {
    ensure!((1..=16384).contains(&column), "Excel column outside bounds");
    let mut out = String::new();
    while column > 0 {
        let digit = ((column - 1) % 26) as u8;
        out.push(char::from(b'A' + digit));
        column = (column - 1) / 26;
    }
    Ok(out.chars().rev().collect())
}

pub(super) fn transform_index(
    mut position: u32,
    operations: &[StructuralOperation],
    row: bool,
) -> Option<u32> {
    for operation in operations {
        if operation.row_operation() != row {
            continue;
        }
        match operation.kind {
            OperationKind::InsertRows | OperationKind::InsertColumns => {
                if position >= operation.at {
                    position = position.checked_add(operation.count)?;
                }
            }
            OperationKind::DeleteRows | OperationKind::DeleteColumns => {
                let end = operation.at.checked_add(operation.count)?;
                if (operation.at..end).contains(&position) {
                    return None;
                }
                if position >= end {
                    position -= operation.count;
                }
            }
        }
    }
    Some(position)
}

pub(super) fn map_anchor_index(
    mut position: u32,
    operations: &[StructuralOperation],
    row: bool,
) -> Result<u32> {
    for operation in operations {
        if operation.row_operation() != row {
            continue;
        }
        match operation.kind {
            OperationKind::InsertRows | OperationKind::InsertColumns => {
                position = position
                    .checked_add(if position >= operation.at {
                        operation.count
                    } else {
                        0
                    })
                    .context("drawing anchor coordinate overflow")?;
            }
            OperationKind::DeleteRows | OperationKind::DeleteColumns => {
                let end = operation
                    .at
                    .checked_add(operation.count)
                    .context("drawing operation range overflow")?;
                if (operation.at..end).contains(&position) {
                    position = operation.at;
                } else if position >= end {
                    position -= operation.count;
                }
            }
        }
    }
    Ok(position)
}

pub fn map_coordinate(
    sheet: &str,
    address: &str,
    operations: &[StructuralOperation],
) -> Result<Option<String>> {
    let (column, row) = coordinate(address)?;
    let mut mapped_column = column;
    let mut mapped_row = row;
    for operation in operations
        .iter()
        .filter(|operation| operation.sheet == sheet)
    {
        if operation.row_operation() {
            mapped_row = match operation.kind {
                OperationKind::InsertRows => {
                    transform_index(mapped_row, std::slice::from_ref(operation), true)
                        .context("row coordinate overflow")?
                }
                OperationKind::DeleteRows => {
                    let Some(value) =
                        transform_index(mapped_row, std::slice::from_ref(operation), true)
                    else {
                        return Ok(None);
                    };
                    value
                }
                _ => unreachable!(),
            };
        } else {
            mapped_column = match operation.kind {
                OperationKind::InsertColumns => {
                    transform_index(mapped_column, std::slice::from_ref(operation), false)
                        .context("column coordinate overflow")?
                }
                OperationKind::DeleteColumns => {
                    let Some(value) =
                        transform_index(mapped_column, std::slice::from_ref(operation), false)
                    else {
                        return Ok(None);
                    };
                    value
                }
                _ => unreachable!(),
            };
        }
    }
    Ok(Some(format!(
        "{}{}",
        column_name(mapped_column)?,
        mapped_row
    )))
}

pub fn resolve_insertion(
    sheet: &str,
    insertion: &str,
    offset: u32,
    column: Option<&str>,
    row: Option<u32>,
    operations: &[StructuralOperation],
) -> Result<String> {
    let operation = operations
        .iter()
        .find(|operation| operation.id == insertion)
        .context("unknown insertion operation")?;
    ensure!(operation.sheet == sheet, "insertion sheet mismatch");
    ensure!(
        operation.insertion(),
        "target requires an insertion operation"
    );
    ensure!(
        offset < operation.count,
        "insertion offset outside operation"
    );
    let target_row = operation.row_operation();
    ensure!(
        (target_row && column.is_some() && row.is_none())
            || (!target_row && row.is_some() && column.is_none()),
        "insertion target must specify column for row insertion or row for column insertion"
    );
    let mut mapped_column = if target_row {
        column.map(column_number).transpose()?.unwrap()
    } else {
        operation.at
    };
    let mut mapped_row = if target_row {
        operation.at
    } else {
        row.unwrap()
    };
    let mut activated = false;
    for candidate in operations
        .iter()
        .filter(|candidate| candidate.sheet == sheet)
    {
        if candidate.id == insertion {
            if target_row {
                mapped_row = mapped_row
                    .checked_add(offset)
                    .context("inserted row coordinate overflow")?;
            } else {
                mapped_column = mapped_column
                    .checked_add(offset)
                    .context("inserted column coordinate overflow")?;
            }
            activated = true;
            continue;
        }
        if candidate.row_operation() {
            mapped_row = transform_index(mapped_row, std::slice::from_ref(candidate), true)
                .context("insertion target was deleted or overflowed")?;
        } else {
            mapped_column = transform_index(mapped_column, std::slice::from_ref(candidate), false)
                .context("insertion target was deleted or overflowed")?;
        }
    }
    ensure!(activated, "insertion operation not found");
    Ok(format!("{}{}", column_name(mapped_column)?, mapped_row))
}
