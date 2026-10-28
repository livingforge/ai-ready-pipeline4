//! Drawing objects moved, resized or given another placement
//! (`place_drawing`). An object's anchor is followed from the original
//! through the row and column operations in the order recorded: those before
//! a placement move it by its old placement, the placement sets its corners
//! or placement, and those after move it by the new one, as Excel moves an
//! object whose placement is changed between edits. The anchor then replaces
//! the one the other edits wrote.
use super::*;

/// Writes `placements` into the workbook at `path`, which the other edits of
/// `original` and the row and column operations `structural` wrote, and
/// returns the parts they change.
pub(super) fn apply_placements(
    original: &Workbook,
    path: &Path,
    structural: &[StructuralOperation],
    placements: &[PlaceOperation],
) -> Result<Vec<String>> {
    if placements.is_empty() {
        return Ok(vec![]);
    }
    let written = Workbook::open(path)?;
    let mut by_object: BTreeMap<&str, Vec<&PlaceOperation>> = BTreeMap::new();
    for placement in placements {
        by_object
            .entry(placement.drawing.as_str())
            .or_default()
            .push(placement);
    }
    let mut patched: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (id, operations) in by_object {
        let (part, number) = id.rsplit_once('#').context("drawing ID without its part")?;
        // A deletion after a placement takes the object where it is then.
        let sheet = &operations[0].sheet;
        let extracted = original
            .sheets
            .iter()
            .find(|s| s["name"] == sheet.as_str())
            .and_then(|s| s["drawings"].as_array())
            .and_then(|drawings| drawings.iter().find(|d| d["id"] == id))
            .with_context(|| format!("drawing {id} is missing"))?;
        let deleted = drawing_deleted(sheet, id, &extracted["anchor"], structural, placements)?;
        let source = std::str::from_utf8(
            original
                .parts
                .get(part)
                .with_context(|| format!("drawing part {part} is missing"))?,
        )?;
        let doc = xml(source.as_bytes())?;
        let anchor = top_level_anchor(&doc, number, id)?;
        let (root, _) = xml_opening(&source[doc.root_element().range()])?;
        let root_tag = element_tag(root)?;
        let mut text = format!("{root}{}</{root_tag}>", &source[anchor.range()]);
        let sheet = &operations[0].sheet;
        let own = |from: usize, to: usize| -> Vec<StructuralOperation> {
            structural[from..to]
                .iter()
                .filter(|operation| &operation.sheet == sheet)
                .cloned()
                .collect()
        };
        let mut done = 0;
        for operation in operations {
            text = move_placed_anchors(&text, &own(done, operation.preceding))?;
            done = operation.preceding;
            text = place(&text, operation)?;
        }
        text = move_placed_anchors(&text, &own(done, structural.len()))?;
        let target = match patched.get(part) {
            Some(bytes) => String::from_utf8(bytes.clone())?,
            None => std::str::from_utf8(
                written
                    .parts
                    .get(part)
                    .with_context(|| format!("drawing part {part} is missing"))?,
            )?
            .to_owned(),
        };
        let replaced = if deleted {
            remove_anchor(&target, number, id)?
        } else {
            replace_anchor(&target, &text, number, id)?
        };
        patched.insert(part.to_owned(), replaced.into_bytes());
    }
    patched.retain(|part, bytes| written.parts.get(part) != Some(bytes));
    if patched.is_empty() {
        return Ok(vec![]);
    }
    let staged = path.with_extension(format!(
        "placed.{}",
        path.extension()
            .context("workbook without an extension")?
            .to_string_lossy()
    ));
    write_archive_without(&written.raw, &staged, &patched, &BTreeSet::new())?;
    Workbook::open(&staged)?;
    fs::rename(&staged, path)?;
    Ok(patched.into_keys().collect())
}

/// The anchor of the object whose `cNvPr` id is `number`, refused for an
/// object inside a group, which moves with its group.
fn top_level_anchor<'a>(doc: &'a Document<'a>, number: &str, id: &str) -> Result<Node<'a, 'a>> {
    let properties = doc
        .descendants()
        .find(|n| n.has_tag_name((XDR, "cNvPr")) && n.attribute("id") == Some(number))
        .with_context(|| format!("drawing {id} is missing"))?;
    let anchor = properties
        .ancestors()
        .find(|n| {
            n.has_tag_name((XDR, "twoCellAnchor"))
                || n.has_tag_name((XDR, "oneCellAnchor"))
                || n.has_tag_name((XDR, "absoluteAnchor"))
        })
        .with_context(|| format!("drawing {id} has no anchor"))?;
    // The object is the element whose non-visual properties hold the ID.
    let object = properties
        .parent_element()
        .and_then(|n| n.parent_element())
        .context("drawing object")?;
    // A group is placed itself; its own element is no group holding it.
    ensure!(
        !object
            .ancestors()
            .skip(1)
            .take_while(|n| *n != anchor)
            .any(|n| n.has_tag_name((XDR, "grpSp"))),
        "drawing {id} is inside a group; place the group, which its objects move and size with"
    );
    Ok(anchor)
}

/// The drawing `text` holding one anchor, with the corners and placement of
/// `operation` set.
fn place(text: &str, operation: &PlaceOperation) -> Result<String> {
    let doc = xml(text.as_bytes())?;
    let anchor = doc.root_element().first_element_child().context("anchor")?;
    let prefix = element_tag(&text[anchor.range()])?
        .rsplit_once(':')
        .map(|(prefix, _)| format!("{prefix}:"))
        .unwrap_or_default();
    let two_cell = anchor.has_tag_name((XDR, "twoCellAnchor"));
    let mut edits = vec![];
    if let Some(placement) = &operation.placement {
        ensure!(
            two_cell,
            "drawing {} is anchored to one cell or to the sheet; Excel sets the placement of a two-cell anchor",
            operation.drawing
        );
        // Excel leaves out the placement that moves and sizes with the cells.
        let opening = opening_range(text, anchor)?;
        let tag = if placement == "twoCell" {
            remove_xml_attribute(&text[opening.clone()], "editAs")?
        } else {
            set_xml_attribute(&text[opening.clone()], "editAs", placement)?
        };
        edits.push((opening, tag));
    }
    if let (Some(from), Some(to)) = (operation.from, operation.to) {
        ensure!(
            two_cell,
            "drawing {} is anchored to one cell or to the sheet; place a two-cell anchor by its corners",
            operation.drawing
        );
        let offsets = operation.offsets.unwrap_or_default();
        for (corner, (name, (column, row))) in [("from", from), ("to", to)].into_iter().enumerate()
        {
            let (column_offset, row_offset) = offsets[corner];
            let marker = anchor
                .children()
                .find(|n| n.has_tag_name((XDR, name)))
                .with_context(|| format!("anchor without {name}"))?;
            edits.push((
                marker.range(),
                format!(
                    "<{prefix}{name}><{prefix}col>{}</{prefix}col><{prefix}colOff>{column_offset}</{prefix}colOff><{prefix}row>{}</{prefix}row><{prefix}rowOff>{row_offset}</{prefix}rowOff></{prefix}{name}>",
                    column - 1,
                    row - 1
                ),
            ));
        }
    }
    splice(text, edits, "placement")
}

/// The drawing `target` without the anchor of object `number`.
fn remove_anchor(target: &str, number: &str, id: &str) -> Result<String> {
    let doc = xml(target.as_bytes())?;
    let anchor = top_level_anchor(&doc, number, id)?;
    splice(target, vec![(anchor.range(), String::new())], "anchor")
}

/// The drawing `target` with the anchor of object `number` taking the
/// placement and corners of the one anchor `placed` holds.
fn replace_anchor(target: &str, placed: &str, number: &str, id: &str) -> Result<String> {
    let doc = xml(target.as_bytes())?;
    let anchor = top_level_anchor(&doc, number, id)?;
    let placed_doc = xml(placed.as_bytes())?;
    let new = placed_doc
        .root_element()
        .first_element_child()
        .context("anchor")?;
    let mut edits = vec![];
    let opening = opening_range(target, anchor)?;
    let mut tag = target[opening.clone()].to_owned();
    tag = match new.attribute("editAs") {
        Some(placement) => set_xml_attribute(&tag, "editAs", placement)?,
        None => remove_xml_attribute(&tag, "editAs")?,
    };
    edits.push((opening, tag));
    for name in ["from", "to"] {
        let (Some(old), Some(marker)) = (
            anchor.children().find(|n| n.has_tag_name((XDR, name))),
            new.children().find(|n| n.has_tag_name((XDR, name))),
        ) else {
            continue;
        };
        edits.push((old.range(), placed[marker.range()].to_owned()));
    }
    splice(target, edits, "anchor")
}
