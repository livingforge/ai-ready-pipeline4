//! What Excel cells carry besides values and formats, changed as operations
//! in mappings.yml after a trial write of every pending change: links,
//! notes, data validations and conditional formats set on or cleared from a
//! range. The content stays as it is.
use super::format_edit::{SheetOperation, unchanged_content};
use super::sheet_edit::rejection;
use super::*;

pub struct ObjectRequest {
    pub sheet: String,
    /// The cells, as the operations recorded before leave them.
    pub range: String,
    /// The operation kind: `set_link`, `clear_links`, `set_note`, ...
    pub kind: String,
    /// What the operation sets besides its range, as mappings.yml records it.
    pub settings: Value,
    pub id: Option<String>,
    pub reason: String,
    pub base: Option<String>,
    pub dry_run: bool,
}

impl Store {
    /// Records the setting or clearing of links, notes, validations or
    /// conditional formats on a range of an adopted workbook, or of the
    /// proposal of `id`, once a trial write of every pending change with it
    /// succeeds.
    pub fn edit_cell_objects(
        &self,
        id: &str,
        proposal: bool,
        edit: &ObjectRequest,
    ) -> Result<Value> {
        let prefix = match edit.kind.as_str() {
            "set_link" | "clear_links" => "link",
            "set_note" | "clear_notes" => "note",
            "set_validation" | "clear_validations" => "validation",
            _ => "conditional",
        };
        let header = SheetOperation {
            sheet: &edit.sheet,
            kind: &edit.kind,
            prefix,
            id: edit.id.as_deref(),
            reason: &edit.reason,
            base: edit.base.as_deref(),
            dry_run: edit.dry_run,
        };
        let settings = |_: &Inspection, operation: &mut Value| -> Result<()> {
            operation["range"] = json!(edit.range);
            if let Some(settings) = edit.settings.as_object() {
                for (key, value) in settings {
                    operation[key] = value.clone();
                }
            }
            Ok(())
        };
        self.record_sheet_operation(
            id,
            proposal,
            &header,
            settings,
            |values, inspected| {
                excel::parse_cell_object_operations(values, &inspected.extraction.sheets)
                    .map(|_| ())
            },
            unchanged_content,
        )
    }
}

/// The refusal of a rule given wrong on the command line.
pub fn invalid_rule(message: impl Into<String>) -> anyhow::Error {
    rejection("invalid_rule", message)
}

/// A formula as a rule records it: without its `=`.
pub fn rule_formula(text: &str, option: &str) -> Result<String> {
    let formula = text.trim().trim_start_matches('=').trim();
    if formula.is_empty() {
        return Err(rejection(
            "invalid_rule",
            format!("{option} takes a value or formula"),
        ));
    }
    Ok(formula.to_owned())
}

/// The list formula of a validation listing `items` (comma-separated), as
/// Excel writes it: the items in quotes, at most 255 characters.
pub fn list_formula(items: &str) -> Result<String> {
    let formula = format!("\"{}\"", items.replace('"', "\"\""));
    if formula.chars().count() > 255 {
        return Err(rejection(
            "invalid_rule",
            "a validation list holds at most 255 characters; list the items in cells and give them with --source",
        ));
    }
    Ok(formula)
}
