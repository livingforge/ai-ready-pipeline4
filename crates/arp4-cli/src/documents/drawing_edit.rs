//! A drawing object of an Excel worksheet moved, resized or given another
//! placement, recorded as a `place_drawing` operation after a trial write
//! of every pending change.
use super::format_edit::SheetOperation;
use super::sheet_edit::rejection;
use super::*;

pub struct DrawingRequest {
    pub sheet: String,
    /// The object's extraction ID, `<drawing part>#<cNvPr id>`.
    pub drawing: String,
    /// The cells of its new top left and bottom right corners, as the
    /// operations recorded before leave them.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Where in those cells the corners are, across and down (EMU); the
    /// cells' top left when none.
    pub offsets: Option<[(u64, u64); 2]>,
    /// `twoCell`, `oneCell` or `absolute`.
    pub placement: Option<String>,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Records the placement of a drawing object of an adopted workbook, or
    /// of the proposal of `id`. The connectors joined to what moves are
    /// named in the report, since they keep their own places.
    pub fn place_drawing(&self, id: &str, proposal: bool, edit: &DrawingRequest) -> Result<Value> {
        let header = SheetOperation {
            sheet: &edit.sheet,
            kind: "place_drawing",
            prefix: "place",
            id: edit.id.as_deref(),
            reason: &edit.reason,
            base: edit.base.as_deref(),
            dry_run: edit.dry_run,
        };
        let mut joined = vec![];
        let settings = |inspected: &Inspection, operation: &mut Value| -> Result<()> {
            let source = inspected
                .extraction
                .sheets
                .iter()
                .find(|sheet| sheet["name"] == edit.sheet.as_str());
            let drawings = source
                .and_then(|sheet| sheet["drawings"].as_array())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let Some(drawing) = drawings.iter().find(|d| d["id"] == edit.drawing.as_str()) else {
                return Err(Rejection(json!({"code":"target_not_found","message":format!("{} has no drawing {}; give an ID of the extraction's drawings",edit.sheet,edit.drawing)})).into());
            };
            if let Some(source) = source {
                let recorded = excel::parse_operations(
                    array(&inspected.mappings["operations"]).map_or(&[][..], Vec::as_slice),
                    &inspected.extraction.sheets,
                )?;
                let placements = excel::parse_place_operations(
                    array(&inspected.mappings["operations"]).map_or(&[][..], Vec::as_slice),
                    &inspected.extraction.sheets,
                )?;
                if super::visual_elements::deleted_drawings(source, &recorded, &placements)?
                    .contains(edit.drawing.as_str())
                {
                    return Err(Rejection(json!({"code":"target_not_found","message":format!("drawing {} goes with the rows or columns a recorded deletion takes, as Excel deletes an object that moves and sizes with its cells; take the deletion back (documents operations remove) to place it",edit.drawing)})).into());
                }
            }
            if let Some(group) = drawing["group"].as_str() {
                return Err(rejection(
                    "invalid_operation",
                    format!(
                        "drawing {} is inside the group {group}; place the group, which its objects move and size with",
                        edit.drawing
                    ),
                ));
            }
            // What moves: the object, and what its group holds.
            let mut moving = BTreeSet::from([edit.drawing.as_str()]);
            let mut grew = true;
            while grew {
                grew = false;
                for member in drawings {
                    if let (Some(group), Some(member)) =
                        (member["group"].as_str(), member["id"].as_str())
                        && moving.contains(group)
                    {
                        grew |= moving.insert(member);
                    }
                }
            }
            if edit.from.is_some() {
                for connector in drawings {
                    let ends = connector["connections"].as_array().into_iter().flatten();
                    if ends
                        .filter_map(|end| end["target"].as_str())
                        .any(|target| moving.contains(target))
                        && let Some(connector) = connector["id"].as_str()
                        && !moving.contains(connector)
                    {
                        joined.push(connector.to_owned());
                    }
                }
            }
            operation["drawing"] = json!(edit.drawing);
            for (key, value) in [
                ("from", &edit.from),
                ("to", &edit.to),
                ("placement", &edit.placement),
            ] {
                if let Some(value) = value {
                    operation[key] = json!(value);
                }
            }
            if let Some([a, b]) = edit.offsets {
                operation["from_offset"] = json!({"col":a.0,"row":a.1});
                operation["to_offset"] = json!({"col":b.0,"row":b.1});
            }
            Ok(())
        };
        let mut report = self.record_sheet_operation(
            id,
            proposal,
            &header,
            settings,
            |values, inspected| {
                excel::parse_place_operations(values, &inspected.extraction.sheets).map(|_| ())
            },
            format_edit::unchanged_content,
        )?;
        if !joined.is_empty() {
            report["joined_connectors"] = json!(joined);
        }
        Ok(report)
    }
}
