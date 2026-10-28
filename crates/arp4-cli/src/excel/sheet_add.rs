//! Worksheets added to a workbook: an empty one, as Insert Sheet makes it,
//! or a copy of another, as Move or Copy with Create a copy makes it. A new
//! sheet takes the name its operation gives and goes after or before a
//! sheet, or last. Its page is named by the operation ID (`sheet-<ID>`), its
//! part `xl/worksheets/sheet-<ID>.xml` and its cells `c-<ID>-<address>`.
//!
//! A copy holds the cells, formats, rows and columns, merges, rules, links,
//! notes, drawings and print settings of the sheet copied, as the original
//! holds them; its formulas, rules and names naming the sheet copied name
//! the copy, and the workbook's names referring to that sheet are given to
//! the copy too, as Excel copies them. Its tables take names of their own,
//! as Excel names them, which its formulas name, and its charts parts of
//! their own, whose series name the copy. Sheets with pivot tables,
//! threaded comments, controls or embedded objects are not copied.
use super::cell_objects::Writing;
use super::*;
use crate::extraction::Fields;

const KINDS: [&str; 4] = ["add_sheet", "copy_sheet", "delete_sheet", "rename_sheet"];
const WORKSHEET: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml";
/// The relationships of a worksheet a copy shares with the sheet copied.
const SHARED: [&str; 2] = ["/hyperlink", "/image"];

pub fn is_sheet_operation(value: &Value) -> bool {
    value["kind"]
        .as_str()
        .is_some_and(|kind| KINDS.contains(&kind))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetOperation {
    pub id: String,
    pub name: String,
    /// The sheet copied; none for an empty sheet.
    pub from: Option<String>,
    /// The sheet the new one goes next to, and whether after it; none for last.
    pub next_to: Option<(String, bool)>,
}

/// A worksheet of the original deleted, or renamed (`name`), which export
/// does after every other edit: the other edits name the sheet by its name
/// in the original.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetChange {
    pub id: String,
    pub sheet: String,
    /// The new name; none for a deletion.
    pub name: Option<String>,
}

/// The page of the sheet the operation `id` adds.
pub fn sheet_page(id: &str) -> String {
    format!("sheet-{id}")
}

/// The part the sheet the operation `id` adds is written in.
pub fn sheet_part(id: &str) -> String {
    format!("xl/worksheets/sheet-{id}.xml")
}

/// The drawing part of the drawings of the copy the operation `id` makes.
pub fn copied_drawing_part(id: &str) -> String {
    format!("xl/drawings/drawing-{id}.xml")
}

/// The part a chart, or a part of a chart, `part` of the sheet the
/// operation `id` copies is copied to: beside it, named for the operation.
pub fn copied_chart_part(id: &str, part: &str) -> String {
    match part.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{id}.{extension}"),
        None => format!("{part}-{id}"),
    }
}

/// The chart parts of the extraction (`chart_parts`, each with its part,
/// formulas and XML) the copies `operations` make of the sheets `sheets`
/// add: those of the charts the sheets copied hold, their series naming
/// the copies.
pub fn copied_charts(
    chart_parts: &[Value],
    sheets: &[impl Fields],
    operations: &[SheetOperation],
) -> Result<Vec<Value>> {
    let mut added = vec![];
    for operation in operations {
        let Some(from) = &operation.from else {
            continue;
        };
        let Some(source) = sheets
            .iter()
            .map(Fields::fields)
            .find(|fields| fields["name"] == from.as_str() && fields["added_by"].is_null())
        else {
            continue;
        };
        for part in source["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|drawing| drawing["chart_part"].as_str())
        {
            let Some(chart) = chart_parts.iter().find(|chart| chart["part"] == part) else {
                continue;
            };
            let rename = |formula: &str| rename_sheet(formula, from, &operation.name);
            let formulas = chart["formulas"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(rename)
                .collect::<Result<Vec<_>>>()?;
            let text = rewrite_texts(string(&chart["xml"])?, &["f"], rename)?;
            added.push(json!({"part":copied_chart_part(&operation.id, part),"formulas":formulas,"xml":text}));
        }
    }
    Ok(added)
}

/// Why Excel does not take `name` as a sheet's name, if it does not.
pub fn sheet_name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        Some("a sheet name holds a character other than spaces")
    } else if name.chars().count() > 31 {
        Some("a sheet name is at most 31 characters")
    } else if name.contains([':', '\\', '/', '?', '*', '[', ']']) {
        Some("a sheet name holds none of : \\ / ? * [ ]")
    } else if name.starts_with('\'') || name.ends_with('\'') {
        Some("a sheet name neither starts nor ends with '")
    } else if name.eq_ignore_ascii_case("History") {
        Some("Excel keeps the name History for itself")
    } else {
        None
    }
}

/// Whether `a` and `b` name the same sheet, as Excel compares names.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// The sheet operations of `values` that add sheets, in the order recorded,
/// checked against the worksheets `sheets` of the extraction (the sheets
/// they add left out) with the others ([`parse_sheet_changes`]).
pub fn parse_sheet_operations(
    values: &[Value],
    sheets: &[impl Fields],
) -> Result<Vec<SheetOperation>> {
    Ok(parse_all(values, sheets)?.0)
}

/// The sheet operations of `values` that delete or rename sheets of the
/// original, in the order recorded.
pub fn parse_sheet_changes(values: &[Value], sheets: &[impl Fields]) -> Result<Vec<SheetChange>> {
    Ok(parse_all(values, sheets)?.1)
}

/// The sheet operations of `values`: those that add sheets and those that
/// delete or rename sheets of the original. A name a sheet of the original
/// has, or one an operation gives, is given to no other sheet: export adds
/// sheets before it deletes and renames them.
fn parse_all(
    values: &[Value],
    sheets: &[impl Fields],
) -> Result<(Vec<SheetOperation>, Vec<SheetChange>)> {
    let originals: Vec<&Value> = sheets
        .iter()
        .map(Fields::fields)
        .filter(|fields| fields["added_by"].is_null())
        .collect();
    // The sheets there are as the operations come, by their names in the
    // original or as added, and every name taken.
    let mut names: Vec<String> = originals
        .iter()
        .map(|fields| string(&fields["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    // A view of the extraction the operations leave (its sheets have their
    // pages) lacks the sheets they delete already.
    let view = sheets.iter().any(|sheet| !sheet.fields()["page"].is_null());
    let mut gone = BTreeSet::new();
    if view {
        for value in values
            .iter()
            .filter(|value| value["kind"] == "delete_sheet")
        {
            let sheet = string(&value["sheet"])?;
            if !names.iter().any(|name| name == sheet) {
                names.push(sheet.to_owned());
                gone.insert(sheet.to_owned());
            }
        }
    }
    let mut taken = names.clone();
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    let mut visible = originals
        .iter()
        .filter(|fields| {
            fields["state"]
                .as_str()
                .is_none_or(|state| state == "visible")
        })
        .count();
    let mut operations = vec![];
    let mut changes = vec![];
    for value in values.iter().filter(|value| is_sheet_operation(value)) {
        let id = string(&value["id"])?;
        identifier(id)?;
        ensure!(
            !string(&value["reason"])?.trim().is_empty(),
            "operation reason required"
        );
        let kind = value["kind"].as_str().unwrap_or_default();
        let new_name = |name: &str, taken: &[String]| -> Result<()> {
            if let Some(problem) = sheet_name_problem(name) {
                bail!("{kind} {id}: {problem}");
            }
            ensure!(
                !taken.iter().any(|other| same_name(other, name)),
                "{kind} {id}: the workbook has, or an operation gives, a sheet the name {name}; a name the pending operations free is given to no other sheet before export"
            );
            Ok(())
        };
        if matches!(kind, "delete_sheet" | "rename_sheet") {
            let sheet = string(&value["sheet"])?;
            ensure!(
                !deleted.contains(sheet),
                "{kind} {id}: {sheet} is deleted by an earlier operation"
            );
            if kind == "delete_sheet" && gone.contains(sheet) {
                deleted.insert(sheet.to_owned());
                names.retain(|other| other != sheet);
                changes.push(SheetChange {
                    id: id.to_owned(),
                    sheet: sheet.to_owned(),
                    name: None,
                });
                continue;
            }
            let original = originals
                .iter()
                .find(|fields| fields["name"] == sheet)
                .with_context(|| format!("{kind} {id}: {sheet} is no worksheet of the original"))?;
            let name = if kind == "rename_sheet" {
                let name = string(&value["name"])?;
                new_name(name, &taken)?;
                taken.push(name.to_owned());
                Some(name.to_owned())
            } else {
                if original["state"]
                    .as_str()
                    .is_none_or(|state| state == "visible")
                {
                    ensure!(
                        visible > 1,
                        "{kind} {id}: {sheet} is the last visible sheet; a workbook keeps one"
                    );
                    visible -= 1;
                }
                deleted.insert(sheet.to_owned());
                names.retain(|other| other != sheet);
                None
            };
            changes.push(SheetChange {
                id: id.to_owned(),
                sheet: sheet.to_owned(),
                name,
            });
            continue;
        }
        let name = string(&value["name"])?;
        new_name(name, &taken)?;
        taken.push(name.to_owned());
        visible += 1;
        let from = match value["kind"].as_str() {
            Some("copy_sheet") => {
                let from = string(&value["from"])?;
                ensure!(
                    !deleted.contains(from),
                    "copy_sheet {id}: {from} is deleted by an earlier operation"
                );
                if !gone.contains(from) {
                    let source = originals
                        .iter()
                        .find(|fields| fields["name"] == from)
                        .with_context(|| {
                            format!("copy_sheet {id}: {from} is no worksheet of the original")
                        })?;
                    ensure!(
                        source["comments"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .all(|comment| comment["kind"] == "note"),
                        "copy_sheet {id}: {from} holds threaded comments, which ARP does not copy; copy it in Excel"
                    );
                }
                Some(from.to_owned())
            }
            _ => None,
        };
        let next_to = match (value["after"].as_str(), value["before"].as_str()) {
            (Some(after), None) => Some((after.to_owned(), true)),
            (None, Some(before)) => Some((before.to_owned(), false)),
            (None, None) => None,
            _ => bail!("{} {id} takes one of after and before", value["kind"]),
        };
        if let Some((sheet, _)) = &next_to {
            ensure!(
                names.iter().any(|other| other == sheet),
                "{} {id}: {sheet} is no sheet of the workbook",
                value["kind"]
            );
        }
        names.push(name.to_owned());
        operations.push(SheetOperation {
            id: id.to_owned(),
            name: name.to_owned(),
            from,
            next_to,
        });
    }
    Ok((operations, changes))
}

/// The tables each copy `operations` make takes, by operation ID: the
/// table's name and the copy's. As Excel names a table it copies, the copy
/// is named by the table's name and the number of tables in the workbook
/// and 2, after `_` where the two would read as a cell, the number counting
/// up while another table has the name.
pub fn copied_tables(
    sheets: &[impl Fields],
    operations: &[SheetOperation],
) -> Result<BTreeMap<String, Vec<(String, String)>>> {
    let originals: Vec<&Value> = sheets
        .iter()
        .map(Fields::fields)
        .filter(|fields| fields["added_by"].is_null())
        .collect();
    let mut taken: BTreeSet<String> = originals
        .iter()
        .flat_map(|fields| fields["tables"].as_array().into_iter().flatten())
        .filter_map(|table| table["name"].as_str().map(str::to_lowercase))
        .collect();
    let mut copied = BTreeMap::new();
    for operation in operations {
        let Some(from) = &operation.from else {
            continue;
        };
        // A sheet a later operation deletes is not in a view; its copy is.
        let Some(source) = originals
            .iter()
            .find(|fields| fields["name"] == from.as_str())
        else {
            continue;
        };
        let mut names = vec![];
        for table in source["tables"].as_array().into_iter().flatten() {
            let name = string(&table["name"])?;
            let count = u32::try_from(taken.len())?;
            let new = (count + 2..)
                .map(|n| {
                    let plain = format!("{name}{n}");
                    if reads_as_reference(&plain) {
                        format!("{name}_{n}")
                    } else {
                        plain
                    }
                })
                .find(|candidate| !taken.contains(&candidate.to_lowercase()))
                .expect("a free table name");
            taken.insert(new.to_lowercase());
            names.push((name.to_owned(), new));
        }
        copied.insert(operation.id.clone(), names);
    }
    Ok(copied)
}

/// `formula` of the copy of the sheet `from` named `to`, whose tables take
/// the names `tables` give them: naming the copy and its tables where it
/// names the sheet and its tables.
pub fn copied_formula(
    formula: &str,
    from: &str,
    to: &str,
    tables: &[(String, String)],
) -> Result<String> {
    let renamed: BTreeMap<String, String> = tables
        .iter()
        .map(|(old, new)| (old.to_lowercase(), new.clone()))
        .collect();
    Ok(rename_tables(&rename_sheet(formula, from, to)?, &renamed))
}

/// The worksheets `sheets` with those `operations` add after them, each
/// with its page: an empty sheet, or a copy of the cells and the other
/// members of the sheet it copies, its formulas naming the copy where they
/// name that sheet. A sheet already in `sheets` is not added again.
pub fn sheet_view<S: Fields>(sheets: &[S], operations: &[SheetOperation]) -> Result<Vec<S>> {
    let tables = copied_tables(sheets, operations)?;
    let mut view: Vec<S> = vec![];
    for (index, sheet) in sheets.iter().enumerate() {
        let mut sheet = sheet.clone();
        if sheet.fields()["page"].is_null() {
            sheet.fields_mut()["page"] = json!(format!("sheet-{}", index + 1));
        }
        view.push(sheet);
    }
    for operation in operations {
        let page = sheet_page(&operation.id);
        if view
            .iter()
            .any(|sheet| sheet.fields()["page"] == page.as_str())
        {
            continue;
        }
        let mut sheet = match &operation.from {
            Some(from) => {
                let source = view
                    .iter()
                    .find(|sheet| {
                        sheet.fields()["name"] == from.as_str()
                            && sheet.fields()["added_by"].is_null()
                    })
                    .with_context(|| format!("{from} is no worksheet of the original"))?;
                let names = tables.get(&operation.id).map_or(&[][..], Vec::as_slice);
                let mut copy = source.copied(&operation.id, &|formula| {
                    copied_formula(formula, from, &operation.name, names)
                })?;
                for (table, (_, new)) in copy.fields_mut()["tables"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                    .zip(names)
                {
                    table["name"] = json!(new);
                }
                // The drawings and charts are the copy's own.
                for drawing in copy.fields_mut()["drawings"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                {
                    if let Some(chart) = drawing["chart_part"].as_str() {
                        drawing["chart_part"] = json!(copied_chart_part(&operation.id, chart));
                    }
                }
                let drawings = &copy.fields()["drawings"];
                if let Some(part) = drawings[0]["part"].as_str() {
                    let text = serde_json::to_string(drawings)?
                        .replace(
                            &format!("\"{part}#"),
                            &format!("\"{}#", copied_drawing_part(&operation.id)),
                        )
                        .replace(
                            &format!("\"{part}\""),
                            &format!("\"{}\"", copied_drawing_part(&operation.id)),
                        );
                    copy.fields_mut()["drawings"] = serde_json::from_str(&text)?;
                }
                copy
            }
            None => S::with_cells(
                json!({"state":"visible","hidden_rows":[],"hidden_columns":[],"merges":[],
                    "drawings":[],"tables":[],"comments":[],"links":[],"validations":[],
                    "conditional_formats":[],"computed":[]}),
                &json!([]),
            )?,
        };
        let fields = sheet.fields_mut();
        fields["name"] = json!(operation.name);
        fields["part"] = json!(sheet_part(&operation.id));
        fields["page"] = json!(page);
        fields["added_by"] = json!(operation.id);
        if fields["state"] != "visible" {
            fields["state"] = json!("visible");
        }
        view.push(sheet);
    }
    Ok(view)
}

/// The names of the worksheets `sheets` of the original in the order the
/// sheets `operations` add leave them.
pub fn sheet_order(sheets: &[impl Fields], operations: &[SheetOperation]) -> Result<Vec<String>> {
    let mut order: Vec<String> = sheets
        .iter()
        .map(Fields::fields)
        .filter(|fields| fields["added_by"].is_null())
        .map(|fields| string(&fields["name"]).map(str::to_owned))
        .collect::<Result<_>>()?;
    for operation in operations {
        let at = match &operation.next_to {
            Some((sheet, after)) => order
                .iter()
                .position(|name| name == sheet)
                .map_or(order.len(), |at| at + usize::from(*after)),
            None => order.len(),
        };
        order.insert(at, operation.name.clone());
    }
    Ok(order)
}

/// The content type the package `writing` gives `part`.
fn content_type_of(writing: &Writing<'_>, part: &str) -> Result<Option<String>> {
    let types = writing
        .text("[Content_Types].xml")?
        .context("package without content types")?;
    let doc = xml(types.as_bytes())?;
    let name = format!("/{part}");
    let extension = part.rsplit_once('.').map(|(_, e)| e);
    let mut by_extension = None;
    for node in doc.root_element().children().filter(Node::is_element) {
        if node.has_tag_name((CONTENT_TYPES, "Override"))
            && node.attribute("PartName") == Some(&name)
        {
            return Ok(node.attribute("ContentType").map(str::to_owned));
        }
        if node.has_tag_name((CONTENT_TYPES, "Default"))
            && node
                .attribute("Extension")
                .zip(extension)
                .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            by_extension = node.attribute("ContentType").map(str::to_owned);
        }
    }
    Ok(by_extension)
}

/// Writes `bytes` as the new part `part` with the content type of `like`.
fn add_part(writing: &mut Writing<'_>, part: &str, bytes: Vec<u8>, like: &str) -> Result<()> {
    ensure!(!writing.exists(part), "part {part} exists already");
    let declared = |writing: &Writing<'_>| content_type_of(writing, part);
    let wanted =
        content_type_of(writing, like)?.with_context(|| format!("{like} has no content type"))?;
    if declared(writing)?.as_deref() != Some(&wanted) {
        let extension = part.rsplit_once('.').map_or("", |(_, e)| e).to_owned();
        writing.content_type(&extension, &wanted, Some(&format!("/{part}")))?;
    }
    writing.removed.remove(part);
    writing.patched.insert(part.to_owned(), bytes);
    Ok(())
}

/// The first `{directory}/{prefix}{n}.{extension}` the package lacks.
fn free_part(writing: &Writing<'_>, directory: &str, prefix: &str, extension: &str) -> String {
    (1u32..)
        .map(|n| format!("{directory}/{prefix}{n}.{extension}"))
        .find(|part| !writing.exists(part))
        .expect("a free part name")
}

/// `text` with the content of every element named in `names` rewritten by
/// `rewrite`.
fn rewrite_texts(
    text: &str,
    names: &[&str],
    rewrite: impl Fn(&str) -> Result<String>,
) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let mut edits = vec![];
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && names.contains(&n.tag_name().name()))
    {
        for child in node.children().filter(Node::is_text) {
            let before = child.text().unwrap_or_default();
            let after = rewrite(before)?;
            if after != before {
                edits.push((child.range(), xml_text(&after)));
            }
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = text.to_owned();
    for (range, replacement) in edits {
        out.replace_range(range, &replacement);
    }
    Ok(out)
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The VML drawing `text` with its shapes renumbered into the block `block`
/// of shape IDs, as Excel numbers the notes of each sheet apart.
fn renumbered_vml(text: &str, block: u32) -> Result<String> {
    static IDMAP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(<o:idmap\b[^>]*\bdata=")[^"]*""#).unwrap()
    });
    static SHAPE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"_x0000_s(\d+)").unwrap());
    let text = IDMAP.replace_all(text, format!("${{1}}{block}\""));
    let mut failed = None;
    let text = SHAPE.replace_all(&text, |captures: &regex::Captures<'_>| {
        match captures[1].parse::<u32>() {
            Ok(number) => format!("_x0000_s{}", block * 1024 + number % 1024),
            Err(error) => {
                failed = Some(error);
                captures[0].to_owned()
            }
        }
    });
    if let Some(error) = failed {
        bail!("invalid VML shape ID: {error}");
    }
    Ok(text.into_owned())
}

/// The highest block of shape IDs the VML drawings of the package use.
fn vml_block(writing: &Writing<'_>) -> Result<u32> {
    static DATA: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"<o:idmap\b[^>]*\bdata="([^"]*)""#).unwrap()
    });
    let mut parts: BTreeSet<&String> = writing.book.parts.keys().collect();
    parts.extend(writing.patched.keys());
    let mut highest = 0;
    for part in parts {
        if !part.to_ascii_lowercase().ends_with(".vml") {
            continue;
        }
        let Some(text) = writing.text(part)? else {
            continue;
        };
        for captures in DATA.captures_iter(&text) {
            for number in captures[1].split(',') {
                if let Ok(number) = number.trim().parse::<u32>() {
                    highest = highest.max(number);
                }
            }
        }
    }
    Ok(highest)
}

/// The worksheet part `text` copied for the sheet `name` from the sheet
/// `from`: not selected, its formulas, rules and validations naming the copy
/// and its tables (`tables`) where they name the sheet copied and its.
fn copied_worksheet(
    text: &str,
    from: &str,
    name: &str,
    tables: &[(String, String)],
) -> Result<String> {
    static SELECTED: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\s+tabSelected="(1|true)""#).unwrap());
    static UID: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\s+xr:uid="[^"]*""#).unwrap());
    let text = SELECTED.replace_all(text, "");
    let text = UID.replace_all(&text, "");
    rewrite_texts(
        &text,
        &["f", "formula", "formula1", "formula2"],
        |formula| copied_formula(formula, from, name, tables),
    )
}

/// The relationships of the copy of the worksheet part `source`, written as
/// `part`, with the parts they target copied for it: comments, notes,
/// drawings, tables (named as `tables` names them) and printer settings.
/// Links and images are shared.
fn copy_relationships(
    writing: &mut Writing<'_>,
    source: &str,
    part: &str,
    id: &str,
    tables: &[(String, String)],
    sheets: (&str, &str),
) -> Result<()> {
    let Some(rels) = writing.text(&relationships_part(source)?)? else {
        return Ok(());
    };
    let doc = xml(rels.as_bytes())?;
    let mut copied = rels.clone();
    let mut edits = vec![];
    for rel in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        let kind = rel.attribute("Type").context("relationship type missing")?;
        let target = rel
            .attribute("Target")
            .context("relationship target missing")?;
        if rel.attribute("TargetMode") == Some("External")
            || SHARED.iter().any(|s| kind.ends_with(s))
        {
            continue;
        }
        let target_part = relationship_target(source, target)?;
        let bytes = writing
            .book
            .parts
            .get(&target_part)
            .with_context(|| format!("{target_part} is missing"))?
            .clone();
        let new_part = if kind.ends_with("/comments") {
            free_part(writing, "xl", "comments", "xml")
        } else if kind.ends_with("/vmlDrawing") {
            free_part(writing, "xl/drawings", "vmlDrawing", "vml")
        } else if kind.ends_with("/printerSettings") {
            let extension = target_part.rsplit_once('.').map_or("bin", |(_, e)| e);
            free_part(writing, "xl/printerSettings", "printerSettings", extension)
        } else if kind.ends_with("/drawing") {
            copied_drawing_part(id)
        } else if kind.ends_with("/table") {
            free_part(writing, "xl/tables", "table", "xml")
        } else {
            let name = kind.rsplit('/').next().unwrap_or(kind);
            bail!("the sheet holds a {name} part, which ARP does not copy; copy it in Excel");
        };
        let bytes = if kind.ends_with("/vmlDrawing") {
            let block = vml_block(writing)? + 1;
            renumbered_vml(std::str::from_utf8(&bytes)?, block)?.into_bytes()
        } else if kind.ends_with("/table") {
            copied_table(writing, std::str::from_utf8(&bytes)?, tables)?.into_bytes()
        } else {
            bytes
        };
        if kind.ends_with("/drawing") {
            copy_drawing_relationships(writing, &target_part, &new_part, id, sheets)?;
        }
        add_part(writing, &new_part, bytes, &target_part)?;
        let opening = opening_range(&rels, rel)?;
        let relative = relative_target(part, &new_part)?;
        edits.push((
            opening.clone(),
            set_xml_attribute(&rels[opening], "Target", &relative)?,
        ));
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, replacement) in edits {
        copied.replace_range(range, &replacement);
    }
    writing.set(&relationships_part(part)?, copied);
    Ok(())
}

/// The table part `text` copied with the name `tables` give it and an ID
/// no table of the package has.
fn copied_table(writing: &Writing<'_>, text: &str, tables: &[(String, String)]) -> Result<String> {
    static UID: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r#"\s+xr\d*:uid="[^"]*""#).unwrap());
    let doc = xml(text.as_bytes())?;
    let root = doc.root_element();
    let name = root
        .attribute("displayName")
        .context("table without its name")?;
    let new = tables
        .iter()
        .find(|(old, _)| old.eq_ignore_ascii_case(name))
        .map(|(_, new)| new)
        .with_context(|| format!("no name for the copy of the table {name}"))?;
    let mut highest = 0u64;
    let mut parts: BTreeSet<&String> = writing.book.parts.keys().collect();
    parts.extend(writing.patched.keys());
    for part in parts {
        if !part.starts_with("xl/tables/") || !part.ends_with(".xml") {
            continue;
        }
        if let Some(other) = writing.text(part)?
            && let Ok(other) = xml(other.as_bytes())
            && let Some(id) = other
                .root_element()
                .attribute("id")
                .and_then(|id| id.parse().ok())
        {
            highest = highest.max(id);
        }
    }
    let opening = opening_range(text, root)?;
    let mut tag = text[opening.clone()].to_owned();
    tag = set_xml_attribute(&tag, "id", &(highest + 1).to_string())?;
    tag = set_xml_attribute(&tag, "name", new)?;
    tag = set_xml_attribute(&tag, "displayName", new)?;
    let mut out = text.to_owned();
    out.replace_range(opening, &tag);
    Ok(UID.replace_all(&out, "").into_owned())
}

/// The relationships of the drawing part `source` given to its copy `part`
/// for the copy the operation `id` makes of the sheet `sheets.0` named
/// `sheets.1`: images and links shared, charts copied.
fn copy_drawing_relationships(
    writing: &mut Writing<'_>,
    source: &str,
    part: &str,
    id: &str,
    sheets: (&str, &str),
) -> Result<()> {
    let Some(rels) = writing.text(&relationships_part(source)?)? else {
        return Ok(());
    };
    let doc = xml(rels.as_bytes())?;
    let mut edits = vec![];
    for rel in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        let kind = rel.attribute("Type").context("relationship type missing")?;
        if rel.attribute("TargetMode") == Some("External")
            || SHARED.iter().any(|s| kind.ends_with(s))
        {
            continue;
        }
        if !kind.ends_with("/chart") {
            let name = kind.rsplit('/').next().unwrap_or(kind);
            bail!(
                "the sheet's drawing holds a {name}, which ARP does not copy; copy the sheet in Excel"
            );
        }
        let target = rel
            .attribute("Target")
            .context("relationship target missing")?;
        let chart = relationship_target(source, target)?;
        let copied = copied_chart_part(id, &chart);
        copy_chart(writing, &chart, &copied, id, sheets)?;
        let opening = opening_range(&rels, rel)?;
        edits.push((
            opening.clone(),
            set_xml_attribute(&rels[opening], "Target", &relative_target(part, &copied)?)?,
        ));
    }
    let mut copied = rels.clone();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, replacement) in edits {
        copied.replace_range(range, &replacement);
    }
    writing.set(&relationships_part(part)?, copied);
    Ok(())
}

/// The chart part `source` copied as `part` for the copy the operation `id`
/// makes of the sheet `sheets.0` named `sheets.1`, its series naming the
/// copy, with its style, colors and shapes beside it; its pictures and links
/// are shared.
fn copy_chart(
    writing: &mut Writing<'_>,
    source: &str,
    part: &str,
    id: &str,
    (from, name): (&str, &str),
) -> Result<()> {
    let text = writing
        .text(source)?
        .with_context(|| format!("{source} is missing"))?;
    let text = rewrite_texts(&text, &["f"], |formula| rename_sheet(formula, from, name))?;
    add_part(writing, part, text.into_bytes(), source)?;
    let Some(rels) = writing.text(&relationships_part(source)?)? else {
        return Ok(());
    };
    let doc = xml(rels.as_bytes())?;
    let mut edits = vec![];
    for rel in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        let kind = rel.attribute("Type").context("relationship type missing")?;
        if rel.attribute("TargetMode") == Some("External")
            || SHARED.iter().any(|s| kind.ends_with(s))
        {
            continue;
        }
        let own = [
            "/chartStyle",
            "/chartColorStyle",
            "/chartUserShapes",
            "/themeOverride",
        ];
        if !own.iter().any(|own| kind.ends_with(own)) {
            let name = kind.rsplit('/').next().unwrap_or(kind);
            bail!(
                "the sheet's chart holds a {name}, which ARP does not copy; copy the sheet in Excel"
            );
        }
        let target = rel
            .attribute("Target")
            .context("relationship target missing")?;
        let target = relationship_target(source, target)?;
        let copied = copied_chart_part(id, &target);
        let bytes = writing
            .book
            .parts
            .get(&target)
            .with_context(|| format!("{target} is missing"))?
            .clone();
        if kind.ends_with("/chartUserShapes") {
            copy_drawing_relationships(writing, &target, &copied, id, (from, name))?;
        }
        add_part(writing, &copied, bytes, &target)?;
        let opening = opening_range(&rels, rel)?;
        edits.push((
            opening.clone(),
            set_xml_attribute(&rels[opening], "Target", &relative_target(part, &copied)?)?,
        ));
    }
    let mut copied = rels.clone();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, replacement) in edits {
        copied.replace_range(range, &replacement);
    }
    writing.set(&relationships_part(part)?, copied);
    Ok(())
}

/// The defined names of the workbook `text` for a sheet put at `index`: the
/// sheets' own names from there on moved one place, and for a copy of the
/// sheet at `source` (its place before), its own names and the workbook's
/// names referring to it, given to the copy and naming it.
fn names_for(text: &str, index: usize, copy: Option<(usize, &str, &str)>) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let Some(list) = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name((NS, "definedNames")))
    else {
        return Ok(text.to_owned());
    };
    let mut edits = vec![];
    let mut added = String::new();
    let names: Vec<Node<'_, '_>> = list
        .children()
        .filter(|n| n.has_tag_name((NS, "definedName")))
        .collect();
    let local = |node: &Node<'_, '_>| {
        node.attribute("localSheetId")
            .and_then(|id| id.parse::<usize>().ok())
    };
    for node in &names {
        if let Some(sheet) = local(node)
            && sheet >= index
        {
            let opening = opening_range(text, *node)?;
            edits.push((
                opening.clone(),
                set_xml_attribute(&text[opening], "localSheetId", &(sheet + 1).to_string())?,
            ));
        }
    }
    if let Some((source, from, name)) = copy {
        let raw = |node: &Node<'_, '_>| &text[node.range()];
        for node in &names {
            let formula = node.text().unwrap_or_default();
            let renamed = rename_sheet(formula, from, name)?;
            let label = node.attribute("name").unwrap_or_default();
            let own = local(node) == Some(source);
            let referring = local(node).is_none()
                && renamed != formula
                && !names.iter().any(|other| {
                    local(other) == Some(source) && other.attribute("name") == Some(label)
                });
            if !own && !referring {
                continue;
            }
            let element = raw(node);
            let (opening, _) = xml_opening(element)?;
            let opening = set_xml_attribute(opening, "localSheetId", &index.to_string())?;
            let tag = element_tag(&opening)?;
            added.push_str(&format!("{opening}{}</{tag}>", xml_text(&renamed)));
        }
    }
    edits.sort_by_key(|(range, _): &(std::ops::Range<usize>, String)| {
        std::cmp::Reverse(range.start)
    });
    let mut out = text.to_owned();
    for (range, replacement) in edits {
        out.replace_range(range, &replacement);
    }
    if !added.is_empty() {
        let doc = xml(out.as_bytes())?;
        let list = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name((NS, "definedNames")))
            .context("defined names missing")?;
        let raw = &out[list.range()];
        let close = list.range().start + raw.rfind("</").context("invalid definedNames")?;
        out.insert_str(close, &added);
    }
    Ok(out)
}

/// Parts written by name, and parts removed.
type Parts = (BTreeMap<String, Vec<u8>>, BTreeSet<String>);

/// The parts of the workbook `book` with the sheets `operations` add: their
/// worksheet parts and what those relate to, and the workbook's list of
/// sheets, names and relationships.
pub(super) fn add_sheets(book: &Workbook, operations: &[SheetOperation]) -> Result<Parts> {
    static ROW_HEIGHT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"<(?:\w+:)?sheetFormatPr\b[^>]*\bdefaultRowHeight="([^"]+)""#).unwrap()
    });
    let tables = copied_tables(&book.sheets, operations)?;
    let mut writing = Writing {
        book,
        patched: BTreeMap::new(),
        removed: BTreeSet::new(),
    };
    let workbook_part = "xl/workbook.xml";
    for operation in operations {
        let part = sheet_part(&operation.id);
        let text = writing
            .text(workbook_part)?
            .context("workbook part missing")?;
        let doc = xml(text.as_bytes())?;
        let sheets_node = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name((NS, "sheets")))
            .context("workbook without sheets")?;
        let listed: Vec<Node<'_, '_>> = sheets_node
            .children()
            .filter(|n| n.has_tag_name((NS, "sheet")))
            .collect();
        let named = |name: &str| {
            listed.iter().position(|n| {
                n.attribute("name")
                    .is_some_and(|other| same_name(other, name))
            })
        };
        ensure!(
            named(&operation.name).is_none(),
            "the workbook has a sheet named {} already",
            operation.name
        );
        let index = match &operation.next_to {
            Some((sheet, after)) => {
                named(sheet).with_context(|| format!("{sheet} is no sheet of the workbook"))?
                    + usize::from(*after)
            }
            None => listed.len(),
        };
        let sheet_id = listed
            .iter()
            .filter_map(|n| n.attribute("sheetId").and_then(|id| id.parse::<u64>().ok()))
            .max()
            .unwrap_or(0)
            + 1;
        // The worksheet part.
        let first_part = book
            .sheets
            .first()
            .map(|sheet| string(&sheet["part"]))
            .transpose()?
            .context("workbook without worksheets")?
            .to_owned();
        let copy = match &operation.from {
            Some(from) => {
                let source = book
                    .sheets
                    .iter()
                    .position(|sheet| sheet["name"] == from.as_str())
                    .with_context(|| format!("{from} is no worksheet of the workbook"))?;
                let source_part = string(&book.sheets[source]["part"])?.to_owned();
                let source_text = writing.text(&source_part)?.context("worksheet missing")?;
                let names = tables.get(&operation.id).map_or(&[][..], Vec::as_slice);
                let copied = copied_worksheet(&source_text, from, &operation.name, names)?;
                copy_relationships(
                    &mut writing,
                    &source_part,
                    &part,
                    &operation.id,
                    names,
                    (from, &operation.name),
                )?;
                add_part(&mut writing, &part, copied.into_bytes(), &source_part)?;
                let source_index = named(from).context("copied sheet missing")?;
                Some((source_index, from.as_str()))
            }
            None => {
                let first = writing.text(&first_part)?.context("worksheet missing")?;
                let height = ROW_HEIGHT
                    .captures(&first)
                    .map(|c| c[1].to_owned())
                    .unwrap_or_else(|| "15".to_owned());
                let empty = format!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="{NS}" xmlns:r="{REL}"><dimension ref="A1"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="{height}"/><sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#
                );
                add_part(&mut writing, &part, empty.into_bytes(), &first_part)?;
                if content_type_of(&writing, &part)?.is_none() {
                    writing.content_type("xml", WORKSHEET, Some(&format!("/{part}")))?;
                }
                None
            }
        };
        // The workbook's relationship to it, its place among the sheets and
        // its names.
        let relationship = writing.relate(
            workbook_part,
            &format!("{REL}/worksheet"),
            &relative_target(workbook_part, &part)?,
            false,
        )?;
        let text = writing
            .text(workbook_part)?
            .context("workbook part missing")?;
        let doc = xml(text.as_bytes())?;
        let sheets_node = doc
            .root_element()
            .children()
            .find(|n| n.has_tag_name((NS, "sheets")))
            .context("workbook without sheets")?;
        let listed: Vec<Node<'_, '_>> = sheets_node
            .children()
            .filter(|n| n.has_tag_name((NS, "sheet")))
            .collect();
        let prefix = element_tag(&text[sheets_node.range()])?
            .strip_suffix("sheets")
            .context("invalid sheets tag")?
            .to_owned();
        let element = format!(
            r#"<{prefix}sheet name="{}" sheetId="{sheet_id}" r:id="{}"/>"#,
            xml_attr(&operation.name),
            xml_attr(&relationship)
        );
        let at = match listed.get(index) {
            Some(next) => next.range().start,
            None => {
                let raw = &text[sheets_node.range()];
                sheets_node.range().start + raw.rfind("</").context("invalid sheets")?
            }
        };
        let mut updated = text.clone();
        updated.insert_str(at, &element);
        // The views count sheets by place.
        let doc = xml(updated.as_bytes())?;
        let mut edits = vec![];
        for view in doc
            .descendants()
            .filter(|n| n.has_tag_name((NS, "workbookView")))
        {
            let opening = opening_range(&updated, view)?;
            let mut tag = updated[opening.clone()].to_owned();
            for attribute in ["activeTab", "firstSheet"] {
                if let Some(place) = view
                    .attribute(attribute)
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|place| *place >= index)
                {
                    tag = set_xml_attribute(&tag, attribute, &(place + 1).to_string())?;
                }
            }
            edits.push((opening, tag));
        }
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        for (range, replacement) in edits {
            updated.replace_range(range, &replacement);
        }
        let copy = copy.map(|(source, from)| (source, from, operation.name.as_str()));
        let updated = names_for(&updated, index, copy)?;
        writing.set(workbook_part, updated);
    }
    let Writing {
        patched, removed, ..
    } = writing;
    Ok((patched, removed))
}

/// The workbook part's sheets, by name, with the parts they are written
/// in, in order.
fn listed_sheets(writing: &Writing<'_>) -> Result<Vec<(String, String)>> {
    let workbook = "xl/workbook.xml";
    let text = writing.text(workbook)?.context("workbook part missing")?;
    let rels = writing
        .text(&relationships_part(workbook)?)?
        .context("workbook relationships missing")?;
    let doc = xml(text.as_bytes())?;
    let mut listed = vec![];
    for sheet in doc.descendants().filter(|n| n.has_tag_name((NS, "sheet"))) {
        let name = sheet.attribute("name").context("sheet without its name")?;
        let id = sheet
            .attribute((REL, "id"))
            .context("sheet without its part")?;
        let (_, target) = relationship(&rels, id)?.context("sheet relationship missing")?;
        listed.push((name.to_owned(), relationship_target(workbook, &target)?));
    }
    Ok(listed)
}

/// Every part of the package `writing` holding formulas that may name a
/// sheet: the worksheets and the charts.
fn formula_parts(writing: &Writing<'_>) -> Result<Vec<String>> {
    let mut parts: Vec<String> = listed_sheets(writing)?
        .into_iter()
        .map(|(_, part)| part)
        .collect();
    let mut all: BTreeSet<&String> = writing.book.parts.keys().collect();
    all.extend(writing.patched.keys());
    parts.extend(
        all.into_iter()
            .filter(|part| part.starts_with("xl/charts/chart") && part.ends_with(".xml"))
            .filter(|part| !writing.removed.contains(*part))
            .cloned(),
    );
    Ok(parts)
}

/// Rewrites the formulas of every worksheet, chart and defined name of the
/// package `writing` by `rewrite`.
fn rewrite_workbook_formulas(
    writing: &mut Writing<'_>,
    rewrite: &dyn Fn(&str) -> Result<String>,
) -> Result<()> {
    for part in formula_parts(writing)? {
        let Some(text) = writing.text(&part)? else {
            continue;
        };
        let names: &[&str] = if part.starts_with("xl/charts/") {
            &["f"]
        } else {
            &["f", "formula", "formula1", "formula2"]
        };
        let updated = rewrite_texts(&text, names, rewrite)?;
        if updated != text {
            writing.set(&part, updated);
        }
    }
    let workbook = "xl/workbook.xml";
    let text = writing.text(workbook)?.context("workbook part missing")?;
    let updated = rewrite_texts(&text, &["definedName"], rewrite)?;
    if updated != text {
        writing.set(workbook, updated);
    }
    Ok(())
}

/// Refuses a change of the sheet `sheet` a pivot cache reads from.
fn ensure_no_pivot_source(writing: &Writing<'_>, sheet: &str) -> Result<()> {
    let mut all: BTreeSet<&String> = writing.book.parts.keys().collect();
    all.extend(writing.patched.keys());
    for part in all {
        if !part.starts_with("xl/pivotCache/") || !part.ends_with(".xml") {
            continue;
        }
        let Some(text) = writing.text(part)? else {
            continue;
        };
        let doc = xml(text.as_bytes())?;
        ensure!(
            !doc.descendants()
                .filter(|n| n.tag_name().name() == "worksheetSource")
                .any(|n| n
                    .attribute("sheet")
                    .is_some_and(|other| same_name(other, sheet))),
            "a pivot table reads from {sheet}; rename or delete it in Excel"
        );
    }
    Ok(())
}

/// Renames the sheet `sheet` `name`, as Excel renames a sheet: the formulas,
/// validations, conditional formats, chart series and names naming it name
/// it so; links to it are left as they are, as Excel leaves them.
fn rename_in(writing: &mut Writing<'_>, sheet: &str, name: &str) -> Result<()> {
    ensure_no_pivot_source(writing, sheet)?;
    let workbook = "xl/workbook.xml";
    let text = writing.text(workbook)?.context("workbook part missing")?;
    let doc = xml(text.as_bytes())?;
    let node = doc
        .descendants()
        .filter(|n| n.has_tag_name((NS, "sheet")))
        .find(|n| {
            n.attribute("name")
                .is_some_and(|other| same_name(other, sheet))
        })
        .with_context(|| format!("{sheet} is no sheet of the workbook"))?;
    let opening = opening_range(&text, node)?;
    let tag = set_xml_attribute(&text[opening.clone()], "name", name)?;
    let mut updated = text.clone();
    updated.replace_range(opening, &tag);
    writing.set(workbook, updated);
    rewrite_workbook_formulas(writing, &|formula| rename_sheet(formula, sheet, name))
}

/// Deletes the sheet `sheet` with the parts only it uses, as Excel deletes a
/// sheet: the formulas, validations, conditional formats, chart series and
/// names naming it, or its tables, read `#REF!`, its own names go, and the
/// names of the sheets after it are moved one place; links are left as
/// they are, as Excel leaves them.
fn delete_in(writing: &mut Writing<'_>, sheet: &str) -> Result<()> {
    ensure_no_pivot_source(writing, sheet)?;
    let workbook = "xl/workbook.xml";
    let listed = listed_sheets(writing)?;
    let index = listed
        .iter()
        .position(|(name, _)| same_name(name, sheet))
        .with_context(|| format!("{sheet} is no sheet of the workbook"))?;
    let part = listed[index].1.clone();
    // The tables it holds, which other formulas no longer find.
    let mut tables = BTreeSet::new();
    if let Some(rels) = writing.text(&relationships_part(&part)?)? {
        let doc = xml(rels.as_bytes())?;
        for rel in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
            .filter(|n| n.attribute("Type").is_some_and(|t| t.ends_with("/table")))
        {
            let target = relationship_target(&part, rel.attribute("Target").unwrap_or_default())?;
            if let Some(table) = writing.text(&target)? {
                let table = xml(table.as_bytes())?;
                if let Some(name) = table.root_element().attribute("displayName") {
                    tables.insert(name.to_lowercase());
                }
            }
        }
    }
    // The workbook without it: its entry and relationship, its own names
    // and the places of the others, the views and the calculation chain.
    let text = writing.text(workbook)?.context("workbook part missing")?;
    let doc = xml(text.as_bytes())?;
    let mut edits = vec![];
    let mut relationship = None;
    for node in doc.descendants().filter(Node::is_element) {
        if node.has_tag_name((NS, "sheet"))
            && node
                .attribute("name")
                .is_some_and(|other| same_name(other, sheet))
        {
            relationship = node.attribute((REL, "id")).map(str::to_owned);
            edits.push((node.range(), String::new()));
        } else if node.has_tag_name((NS, "definedName"))
            && let Some(local) = node
                .attribute("localSheetId")
                .and_then(|v| v.parse::<usize>().ok())
        {
            if local == index {
                edits.push((node.range(), String::new()));
            } else if local > index {
                let opening = opening_range(&text, node)?;
                edits.push((
                    opening.clone(),
                    set_xml_attribute(&text[opening], "localSheetId", &(local - 1).to_string())?,
                ));
            }
        } else if node.has_tag_name((NS, "workbookView")) {
            let opening = opening_range(&text, node)?;
            let mut tag = text[opening.clone()].to_owned();
            for attribute in ["activeTab", "firstSheet"] {
                if let Some(place) = node
                    .attribute(attribute)
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|place| *place >= index && *place > 0)
                {
                    let place = if place > index || place + 1 == listed.len() {
                        place - 1
                    } else {
                        place
                    };
                    tag = set_xml_attribute(&tag, attribute, &place.to_string())?;
                }
            }
            edits.push((opening, tag));
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut updated = text.clone();
    for (range, replacement) in edits {
        updated.replace_range(range, &replacement);
    }
    writing.set(workbook, updated);
    let rels_part = relationships_part(workbook)?;
    let rels = writing
        .text(&rels_part)?
        .context("workbook relationships missing")?;
    let doc = xml(rels.as_bytes())?;
    let mut edits = vec![];
    for rel in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        let calc_chain = rel
            .attribute("Type")
            .is_some_and(|t| t.ends_with("/calcChain"));
        if rel.attribute("Id") == relationship.as_deref() || calc_chain {
            edits.push((rel.range(), String::new()));
            if calc_chain {
                remove_part(writing, "xl/calcChain.xml")?;
            }
        }
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut rels = rels.clone();
    for (range, replacement) in edits {
        rels.replace_range(range, &replacement);
    }
    writing.set(&rels_part, rels);
    // The parts only it uses, those only they use, and so on.
    let mut gone = BTreeSet::from([part]);
    loop {
        let referred = referrers(writing)?;
        let more: Vec<String> = referred
            .iter()
            .filter(|(target, from)| {
                !gone.contains(*target) && from.iter().all(|from| gone.contains(from))
            })
            .map(|(target, _)| target.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        gone.extend(more);
    }
    for part in &gone {
        remove_part(writing, part)?;
        let rels = relationships_part(part)?;
        if writing.exists(&rels) {
            remove_part(writing, &rels)?;
        }
    }
    rewrite_workbook_formulas(writing, &|formula| {
        delete_sheet_references(formula, sheet, &tables)
    })
}

/// The internal parts each relationship part of the package `writing`
/// targets, with the parts relating to them.
fn referrers(writing: &Writing<'_>) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut all: BTreeSet<&String> = writing.book.parts.keys().collect();
    all.extend(writing.patched.keys());
    let mut referred: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for rels_part in all.into_iter().filter(|p| p.ends_with(".rels")) {
        let Some(rels) = writing.text(rels_part)? else {
            continue;
        };
        // The part these relationships are of: `dir/_rels/name.rels` → `dir/name`.
        let Some((directory, file)) = rels_part.rsplit_once("_rels/") else {
            continue;
        };
        let source = format!("{directory}{}", file.trim_end_matches(".rels"));
        let doc = xml(rels.as_bytes())?;
        for rel in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
            .filter(|n| n.attribute("TargetMode") != Some("External"))
        {
            let target = rel.attribute("Target").unwrap_or_default();
            if let Ok(target) = relationship_target(&source, target) {
                referred.entry(target).or_default().insert(source.clone());
            }
        }
    }
    Ok(referred)
}

/// Takes the part `part` out of the package `writing`, with its content type.
fn remove_part(writing: &mut Writing<'_>, part: &str) -> Result<()> {
    writing.patched.remove(part);
    if writing.book.parts.contains_key(part) {
        writing.removed.insert(part.to_owned());
    }
    let types = writing
        .text("[Content_Types].xml")?
        .context("package without content types")?;
    let doc = xml(types.as_bytes())?;
    let name = format!("/{part}");
    if let Some(node) = doc.root_element().children().find(|n| {
        n.has_tag_name((CONTENT_TYPES, "Override")) && n.attribute("PartName") == Some(&name)
    }) {
        let mut updated = types.clone();
        updated.replace_range(node.range(), "");
        writing.set("[Content_Types].xml", updated);
    }
    Ok(())
}

/// Deletes and renames the sheets `changes` name in the workbook at `path`,
/// which the other edits wrote, returning the parts changed and removed.
pub(super) fn change_sheets(path: &Path, changes: &[SheetChange]) -> Result<Vec<String>> {
    if changes.is_empty() {
        return Ok(vec![]);
    }
    let book = Workbook::open(path)?;
    let mut writing = Writing {
        book: &book,
        patched: BTreeMap::new(),
        removed: BTreeSet::new(),
    };
    for change in changes {
        match &change.name {
            Some(name) => rename_in(&mut writing, &change.sheet, name)?,
            None => delete_in(&mut writing, &change.sheet)?,
        }
    }
    let Writing {
        patched, removed, ..
    } = writing;
    let staged = path.with_extension(format!(
        "changed.{}",
        path.extension()
            .context("workbook without an extension")?
            .to_string_lossy()
    ));
    write_archive_without(&book.raw, &staged, &patched, &removed)?;
    let read = Workbook::open(&staged)?;
    for change in changes {
        let present = |name: &str| read.sheets.iter().any(|sheet| sheet["name"] == name);
        match &change.name {
            Some(name) => ensure!(present(name), "the sheet {} was not renamed", change.sheet),
            None => ensure!(
                !present(&change.sheet),
                "the sheet {} was not deleted",
                change.sheet
            ),
        }
    }
    fs::rename(&staged, path)?;
    let mut parts: Vec<String> = patched.into_keys().collect();
    parts.extend(removed);
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_names_follow_excel() {
        assert!(sheet_name_problem("一覧 (2)").is_none());
        assert!(sheet_name_problem("a/b").is_some());
        assert!(sheet_name_problem("'a").is_some());
        assert!(sheet_name_problem(&"x".repeat(32)).is_some());
        assert!(sheet_name_problem("history").is_some());
    }

    #[test]
    fn copies_name_themselves_where_formulas_name_the_sheet_copied() {
        assert_eq!(
            rename_sheet("Src!A1*2+Other!B2+'src'!C3", "Src", "Src (2)").unwrap(),
            "'Src (2)'!A1*2+Other!B2+'Src (2)'!C3"
        );
        assert_eq!(
            rename_sheet("A1+一覧!B2", "一覧", "一覧2").unwrap(),
            "A1+一覧2!B2"
        );
    }

    #[test]
    fn copied_tables_are_named_as_excel_names_them() {
        let sheet = |name: &str, tables: &[&str]| json!({"name":name,"tables":tables.iter().map(|t| json!({"name":t})).collect::<Vec<_>>()});
        let copy = |id: &str| SheetOperation {
            id: id.to_owned(),
            name: format!("copy {id}"),
            from: Some("S".to_owned()),
            next_to: None,
        };
        let names = |tables: &[&str]| {
            copied_tables(&[sheet("S", tables)], &[copy("a"), copy("b")]).unwrap()
        };
        let pairs = |list: &[(&str, &str)]| {
            list.iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect::<Vec<_>>()
        };
        let data = names(&["Data", "Other"]);
        assert_eq!(data["a"], pairs(&[("Data", "Data4"), ("Other", "Other5")]));
        assert_eq!(data["b"], pairs(&[("Data", "Data6"), ("Other", "Other7")]));
        assert_eq!(names(&["Tab1"])["a"], pairs(&[("Tab1", "Tab1_3")]));
        assert_eq!(names(&["Data2"])["a"], pairs(&[("Data2", "Data23")]));
    }

    #[test]
    fn notes_of_a_copy_take_their_own_shape_ids() {
        let vml = r#"<o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="1"/></o:shapelayout><v:shape id="_x0000_s1025" o:spid="_x0000_s1026"/>"#;
        assert_eq!(
            renumbered_vml(vml, 3).unwrap(),
            r#"<o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="3"/></o:shapelayout><v:shape id="_x0000_s3073" o:spid="_x0000_s3074"/>"#
        );
    }
}
