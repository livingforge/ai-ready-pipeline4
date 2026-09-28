//! Worksheet objects that hold cell positions outside the cell data: notes and
//! threaded comments (the cell they belong to), VML shapes (notes, form
//! controls: anchor, note cell, linked cells), form controls and embedded
//! objects (anchor) and form control properties (linked cells). Row/column
//! operations move them as Excel does; a note or comment on a deleted cell is
//! deleted with it.
use super::*;
use std::sync::LazyLock;

/// The VML drawing of a worksheet's form controls and notes, as opposed to
/// the one of its header and footer (`legacyDrawingHF`).
fn vml_drawing(
    parts: &BTreeMap<String, Vec<u8>>,
    worksheet_part: &str,
    worksheet: &str,
) -> Result<Option<String>> {
    let doc = xml(worksheet.as_bytes())?;
    let Some(id) =
        child(doc.root_element(), "legacyDrawing").and_then(|n| n.attribute((REL, "id")))
    else {
        return Ok(None);
    };
    let rels = relationships_part(worksheet_part)?;
    let doc = xml(parts
        .get(&rels)
        .context("worksheet relationships missing")?)?;
    let target = doc
        .descendants()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
        .find(|n| n.attribute("Id") == Some(id))
        .and_then(|n| n.attribute("Target"))
        .context("VML drawing relationship missing")?;
    Ok(Some(relationship_target(worksheet_part, target)?))
}

/// Moves each element's cell reference (`ref`) of a comments part (`comment`)
/// or threaded comments part (`threadedComment`); one on a deleted cell is
/// removed.
fn move_comment_refs(
    original: &str,
    element: &str,
    sheet: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let mut attributes = AttributeEdits::default();
    let mut removals = vec![];
    for node in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == element)
    {
        let Some(reference) = node.attribute("ref") else {
            continue;
        };
        match map_coordinate(sheet, reference, operations)? {
            Some(moved) if moved != reference => attributes.set(original, node, "ref", &moved)?,
            Some(_) => {}
            None => removals.push(node.range()),
        }
    }
    apply_edits(original, attributes.into_edits().collect(), removals)
}

/// Moves the anchors of form controls (`controlPr`) and embedded objects
/// (`objectPr`) in a worksheet: `moveWithCells` and `sizeWithCells` give
/// their placement.
fn move_object_anchors(worksheet: &str, operations: &[StructuralOperation]) -> Result<String> {
    let doc = xml(worksheet.as_bytes())?;
    let mut edits = vec![];
    for anchor in doc.descendants().filter(|n| {
        n.has_tag_name((NS, "anchor"))
            && n.parent_element().is_some_and(|p| {
                p.has_tag_name((NS, "controlPr")) || p.has_tag_name((NS, "objectPr"))
            })
    }) {
        let on = |name: &str| matches!(anchor.attribute(name), Some("1" | "true"));
        let placement = match (on("moveWithCells"), on("sizeWithCells")) {
            (true, true) => "twoCell",
            (true, false) => "oneCell",
            _ => "absolute",
        };
        move_marker_anchor(anchor, placement, operations, &mut edits)?;
    }
    splice(worksheet, edits, "anchor")
}

fn xml_unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Rewrites a VML drawing, read as text since Excel writes HTML-like
/// markup (`<br>`) into it. Linked cells and ranges (`FmlaLink`, `FmlaRange`,
/// `FmlaTxbx`) follow `moves` as formulas of `sheet`; with `operations` on
/// `sheet`, anchors move and a note follows its cell or goes with it. In VML
/// Excel writes `MoveWithCells` and `SizeWithCells` for an object that does
/// *not* move or size with cells.
fn rewrite_vml(
    original: &str,
    sheet: &str,
    operations: &[StructuralOperation],
    moves: &Moves<'_>,
) -> Result<String> {
    static CLIENT_DATA: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?s)<x:ClientData\b[^>]*>.*?</x:ClientData>").unwrap()
    });
    static OBJECT_TYPE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"ObjectType\s*=\s*["']([^"']*)["']"#).unwrap());
    static FIELD: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"<x:(Anchor|Row|Column|FmlaLink|FmlaRange|FmlaTxbx)>([^<]*)</x:(?:Anchor|Row|Column|FmlaLink|FmlaRange|FmlaTxbx)>",
        )
        .unwrap()
    });
    static FLAG: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"<x:(MoveWithCells|SizeWithCells)\s*(?:/>|>\s*([^<]*)</x:(?:MoveWithCells|SizeWithCells)>)").unwrap()
    });
    let mut edits = vec![];
    for block in CLIENT_DATA.find_iter(original) {
        let text = block.as_str();
        let base = block.start();
        let note = OBJECT_TYPE.captures(text).is_some_and(|c| &c[1] == "Note");
        let flag = |name: &str| {
            FLAG.captures_iter(text).any(|c| {
                &c[1] == name
                    && c.get(2)
                        .is_none_or(|v| !v.as_str().trim().eq_ignore_ascii_case("false"))
            })
        };
        let fields: Vec<_> = FIELD.captures_iter(text).collect();
        let value = |name: &str| fields.iter().find(|c| &c[1] == name).and_then(|c| c.get(2));
        for captures in &fields {
            let name = &captures[1];
            if !name.starts_with("Fmla") {
                continue;
            }
            let field = captures.get(2).unwrap();
            let formula = xml_unescape(field.as_str());
            let rewritten = moves.rewrite(formula.trim(), Some(sheet))?;
            if rewritten != formula.trim() {
                edits.push((
                    base + field.start()..base + field.end(),
                    xml_attr(&rewritten),
                ));
            }
        }
        if operations.is_empty() {
            continue;
        }
        if note && let (Some(row), Some(column)) = (value("Row"), value("Column")) {
            let (row_index, column_index): (u32, u32) = (
                row.as_str().trim().parse()?,
                column.as_str().trim().parse()?,
            );
            let cell = format!("{}{}", column_name(column_index + 1)?, row_index + 1);
            let Some(moved) = map_coordinate(sheet, &cell, operations)? else {
                // The note goes with its cell, and so does its shape.
                let start = original[..base]
                    .rfind("<v:shape")
                    .context("VML note without shape")?;
                let end = base
                    + block.len()
                    + original[block.end()..]
                        .find("</v:shape>")
                        .context("VML note shape not closed")?
                    + "</v:shape>".len();
                edits.retain(|(range, _)| range.end <= start || range.start >= end);
                edits.push((start..end, String::new()));
                continue;
            };
            let (new_column, new_row) = coordinate(&moved)?;
            for (field, index) in [(row, new_row - 1), (column, new_column - 1)] {
                if field.as_str().trim() != index.to_string() {
                    edits.push((base + field.start()..base + field.end(), index.to_string()));
                }
            }
        }
        if let Some(anchor) = value("Anchor") {
            let numbers = anchor
                .as_str()
                .split(',')
                .map(|n| n.trim().parse::<u32>())
                .collect::<Result<Vec<_>, _>>()
                .context("invalid VML anchor")?;
            let [c1, o1, r1, o2, c2, o3, r2, o4] = numbers[..] else {
                bail!("invalid VML anchor");
            };
            let placement = match (flag("MoveWithCells"), flag("SizeWithCells")) {
                (true, _) => "absolute",
                (false, true) => "oneCell",
                (false, false) => "twoCell",
            };
            let (from, to) = move_corners(
                placement,
                [(c1, o1 == 0), (r1, o2 == 0)],
                Some([(c2, o3 == 0), (r2, o4 == 0)]),
                operations,
            )?;
            let to = to.context("VML anchor without end")?;
            let offset = |value: u32, reset: bool| if reset { 0 } else { value };
            let moved = [
                from[0].0,
                offset(o1, from[0].1),
                from[1].0,
                offset(o2, from[1].1),
                to[0].0,
                offset(o3, to[0].1),
                to[1].0,
                offset(o4, to[1].1),
            ];
            if moved[..] != numbers[..] {
                let indent =
                    &anchor.as_str()[..anchor.as_str().len() - anchor.as_str().trim_start().len()];
                let text = moved.map(|n| n.to_string()).join(", ");
                edits.push((
                    base + anchor.start()..base + anchor.end(),
                    format!("{indent}{text}"),
                ));
            }
        }
    }
    splice(original, edits, "VML drawing")
}

/// Moves the cells that notes, comments, VML shapes, form controls and
/// embedded objects hold, on every sheet, reading worksheets as `patched`
/// leaves them.
pub(super) fn relocate_sheet_objects(
    parts: &BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    operations: &[StructuralOperation],
    moves: &Moves<'_>,
    patched: &mut BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for sheet in sheets {
        let name = string(&sheet["name"])?;
        let part = string(&sheet["part"])?;
        let own: Vec<StructuralOperation> = operations
            .iter()
            .filter(|o| o.sheet == name)
            .cloned()
            .collect();
        let worksheet = std::str::from_utf8(patched.get(part).unwrap_or(&parts[part]))?.to_owned();
        if !own.is_empty() {
            for (kind, element) in [
                ("/comments", "comment"),
                ("/threadedComment", "threadedComment"),
            ] {
                for comments in related_parts(parts, part, kind)? {
                    let original = std::str::from_utf8(&parts[&comments])?;
                    let moved = move_comment_refs(original, element, name, &own)?;
                    if moved != original {
                        patched.insert(comments, moved.into_bytes());
                    }
                }
            }
            let moved = move_object_anchors(&worksheet, &own)?;
            if moved != worksheet {
                patched.insert(part.to_owned(), moved.into_bytes());
            }
        }
        if let Some(vml) = vml_drawing(parts, part, &worksheet)? {
            let original = std::str::from_utf8(&parts[&vml])?;
            let rewritten = rewrite_vml(original, name, &own, moves)?;
            if rewritten != original {
                patched.insert(vml, rewritten.into_bytes());
            }
        }
        for properties in related_parts(parts, part, "/ctrlProp")? {
            let original = std::str::from_utf8(&parts[&properties])?;
            let doc = xml(original.as_bytes())?;
            let root = doc.root_element();
            let mut attributes = AttributeEdits::default();
            for attribute in ["fmlaLink", "fmlaRange", "fmlaTxbx"] {
                if let Some(formula) = root.attribute(attribute) {
                    let rewritten = moves.rewrite(formula, Some(name))?;
                    if rewritten != formula {
                        attributes.set(original, root, attribute, &rewritten)?;
                    }
                }
            }
            let rewritten = apply_edits(original, attributes.into_edits().collect(), vec![])?;
            if rewritten != original {
                patched.insert(properties, rewritten.into_bytes());
            }
        }
    }
    Ok(())
}
