use super::*;

pub fn validate_image_asset(path: &str, bytes: &[u8]) -> Result<()> {
    ensure!(
        path.starts_with("assets/") && path.to_ascii_lowercase().ends_with(".png"),
        "only PNG image assets are supported"
    );
    let filename = path
        .rsplit('/')
        .next()
        .context("image asset filename required")?;
    ensure!(
        filename
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character)),
        "image asset filename contains unsupported characters"
    );
    ensure!(
        bytes.len() <= 64 * 1024 * 1024,
        "image asset exceeds size budget"
    );
    ensure!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image asset is not a PNG"
    );
    Ok(())
}

/// Moves cell-anchored drawings with their cells as Excel does, by the object's
/// placement (`editAs`): `absolute` stays where it is, `oneCell` moves with its
/// top-left cell and keeps its size, and the default `twoCell` moves and sizes
/// with the cells under both corners, and goes with the rows or columns a
/// deletion takes whole (see [`deleted_with_cells`]).
/// The objects whose `cNvPr` IDs `placed` holds are moved but not deleted:
/// their placements decide (see [`drawing_deleted`]).
pub(super) fn rewrite_drawing_anchors(
    original: &str,
    operations: &[StructuralOperation],
    placed: &BTreeSet<String>,
) -> Result<String> {
    rewrite_anchors(original, operations, Some(placed))
}

/// [`rewrite_drawing_anchors`] for an object a placement (`place_drawing`)
/// moves: it is not deleted with the cells it ends in, as the deletions it
/// would go with are refused by the original's anchor, which the content
/// follows too.
pub(super) fn move_placed_anchors(
    original: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    rewrite_anchors(original, operations, None)
}

/// [`rewrite_drawing_anchors`], deleting the objects the deletions take
/// but those `kept` names when it is given, else none.
fn rewrite_anchors(
    original: &str,
    operations: &[StructuralOperation],
    kept: Option<&BTreeSet<String>>,
) -> Result<String> {
    if operations.is_empty() {
        return Ok(original.to_owned());
    }
    let doc = xml(original.as_bytes())?;
    let mut edits = vec![];
    for anchor in doc.descendants().filter(|node| {
        node.has_tag_name((XDR, "oneCellAnchor")) || node.has_tag_name((XDR, "twoCellAnchor"))
    }) {
        let placement = if anchor.has_tag_name((XDR, "oneCellAnchor")) {
            "oneCell"
        } else {
            anchor.attribute("editAs").unwrap_or("twoCell")
        };
        let placed = || {
            anchor
                .descendants()
                .find(|n| n.has_tag_name((XDR, "cNvPr")))
                .and_then(|n| n.attribute("id"))
                .is_some_and(|id| kept.is_some_and(|kept| kept.contains(id)))
        };
        if kept.is_some()
            && !placed()
            && let Some(from) = marker_corner(anchor, "from")?
            && deleted_with_cells(placement, from, marker_corner(anchor, "to")?, operations)?
        {
            edits.push((anchor.range(), String::new()));
            continue;
        }
        move_marker_anchor(anchor, placement, operations, &mut edits)?;
    }
    splice(original, edits, "drawing")
}

/// The corner of the marker `name` (`from`, `to`) of a cell anchor.
fn marker_corner(anchor: Node<'_, '_>, name: &str) -> Result<Option<Corner>> {
    let Some(marker) = anchor
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
    else {
        return Ok(None);
    };
    let mut corner = [(0, false); 2];
    for (axis, (index, offset)) in [("col", "colOff"), ("row", "rowOff")]
        .into_iter()
        .enumerate()
    {
        let (Some(index), Some(offset)) =
            (child_ns(marker, XDR, index), child_ns(marker, XDR, offset))
        else {
            return Ok(None);
        };
        corner[axis] = (
            index
                .text()
                .context("anchor coordinate is empty")?
                .trim()
                .parse()?,
            offset.text().is_some_and(|text| text.trim() == "0"),
        );
    }
    Ok(Some(corner))
}

/// A corner of an anchor on one axis: its 0-based column or row index and
/// whether its offset into that cell is 0.
pub(super) type Corner = [(u32, bool); 2];

/// Where an anchor's corners go when rows and columns move, by placement:
/// `absolute` stays, `oneCell` moves with its top-left cell and keeps its
/// size, and `twoCell` moves and sizes with the cells under both corners.
/// Each corner is [column, row]; the result gives each new index and whether
/// its offset resets to 0 because its cell was deleted.
pub(super) fn move_corners(
    placement: &str,
    from: Corner,
    to: Option<Corner>,
    operations: &[StructuralOperation],
) -> Result<(Corner, Option<Corner>)> {
    let unchanged = |corner: Corner| corner.map(|(index, _)| (index, false));
    if placement == "absolute" {
        return Ok((unchanged(from), to.map(unchanged)));
    }
    let position = |index: u32| index.checked_add(1).context("anchor coordinate overflow");
    // Each corner as it goes: its index, whether its offset is 0, and whether
    // an operation deleted its cell.
    let mut from_now = from.map(|(index, _)| (index, false, false));
    let mut to_now = to.map(|corner| corner.map(|(index, at_edge)| (index, at_edge, false)));
    for operation in operations {
        let axis = usize::from(operation.row_operation());
        // Cells shifted in a band move an object the band holds across, and
        // leave alone one it misses or holds only part of, as Excel does.
        if let Some((first, last)) = operation.band {
            let across = 1 - axis;
            let low = from_now[across].0 + 1;
            let high = match to_now {
                Some(to) if to[across].1 && to[across].0 > from_now[across].0 => to[across].0,
                Some(to) => to[across].0 + 1,
                None => low,
            };
            if low < first || high > last {
                continue;
            }
        }
        let whole = StructuralOperation {
            band: None,
            ..operation.clone()
        };
        let whole = std::slice::from_ref(&whole);
        let start = position(from_now[axis].0)?;
        let (mapped, deleted) = map_anchor_index(start, whole, axis == 1, false)?;
        let shift = i64::from(mapped) - i64::from(start);
        from_now[axis] = (mapped - 1, from_now[axis].1, from_now[axis].2 || deleted);
        if let Some(to) = &mut to_now {
            let (index, at_edge, was_deleted) = to[axis];
            let end = position(index)?;
            to[axis] = if placement == "oneCell" {
                let moved = i64::from(end) + shift;
                (
                    u32::try_from(moved.max(1)).context("anchor coordinate overflow")? - 1,
                    at_edge,
                    was_deleted,
                )
            } else {
                let (mapped, deleted) = map_anchor_index(end, whole, axis == 1, at_edge)?;
                (mapped - 1, at_edge, was_deleted || deleted)
            };
        }
    }
    let corner =
        |corner: [(u32, bool, bool); 2]| corner.map(|(index, _, deleted)| (index, deleted));
    Ok((corner(from_now), to_now.map(corner)))
}

/// Whether `operations` delete an object with the rows or columns holding
/// it, as Excel does (checked against Excel): one that moves and sizes with
/// its cells (`twoCell`) goes with the rows or columns a deletion takes
/// whole, its far corner at most at the edge after them; one that only
/// moves or stays keeps its size and is never deleted.
pub(super) fn deleted_with_cells(
    placement: &str,
    from: Corner,
    to: Option<Corner>,
    operations: &[StructuralOperation],
) -> Result<bool> {
    let (true, Some(mut to)) = (placement == "twoCell", to) else {
        return Ok(false);
    };
    let mut from = from;
    for operation in operations {
        if operation.whole()
            && matches!(
                operation.kind,
                OperationKind::DeleteRows | OperationKind::DeleteColumns
            )
        {
            let axis = usize::from(operation.row_operation());
            // 0-based, as the markers count.
            let first = operation.at - 1;
            let end = first + operation.count;
            let (start, _) = from[axis];
            let (stop, at_edge) = to[axis];
            if start >= first && (stop < end || stop == end && at_edge) {
                return Ok(true);
            }
        }
        let (moved_from, moved_to) =
            move_corners(placement, from, Some(to), std::slice::from_ref(operation))?;
        let moved_to = moved_to.context("corner without its pair")?;
        from = [0, 1].map(|axis| (moved_from[axis].0, from[axis].1 || moved_from[axis].1));
        to = [0, 1].map(|axis| (moved_to[axis].0, to[axis].1 || moved_to[axis].1));
    }
    Ok(false)
}

/// [`deleted_with_cells`] for an anchor of the extraction (`anchor` of a
/// drawing): an absolute anchor and a slide's never are.
pub(crate) fn anchor_deleted(anchor: &Value, operations: &[StructuralOperation]) -> Result<bool> {
    if anchor["kind"] != "twoCellAnchor" || anchor.get("to").is_none() {
        return Ok(false);
    }
    let corner = |key: &str| -> Result<Corner> {
        let mut out = [(0, false); 2];
        for (axis, name) in ["col", "row"].iter().enumerate() {
            out[axis] = (
                u32::try_from(
                    anchor[key][name]
                        .as_u64()
                        .context("invalid drawing marker")?,
                )?,
                anchor[key][format!("{name}Off")].as_i64().unwrap_or(0) == 0,
            );
        }
        Ok(out)
    };
    deleted_with_cells(
        anchor["edit_as"].as_str().unwrap_or("twoCell"),
        corner("from")?,
        Some(corner("to")?),
        operations,
    )
}

/// Whether the drawing `id` of the sheet `sheet`, whose extraction anchor is
/// `anchor`, goes with the cells the row and column `operations` (of every
/// sheet, in the order recorded) delete, as Excel deletes an object that
/// moves and sizes with its cells: the placements among them set its
/// corners and placement as they come, so a deletion takes it where it is
/// then.
pub(crate) fn drawing_deleted(
    sheet: &str,
    id: &str,
    anchor: &Value,
    operations: &[StructuralOperation],
    placements: &[PlaceOperation],
) -> Result<bool> {
    let placed: Vec<&PlaceOperation> = placements.iter().filter(|p| p.drawing == id).collect();
    if placed.is_empty() {
        let own: Vec<StructuralOperation> = operations
            .iter()
            .filter(|operation| operation.sheet == sheet)
            .cloned()
            .collect();
        return anchor_deleted(anchor, &own);
    }
    let mut current = anchor.clone();
    for (index, operation) in operations.iter().enumerate() {
        for placement in placed.iter().filter(|p| p.preceding == index) {
            current = placed_anchor(&current, placement);
        }
        if operation.sheet != sheet {
            continue;
        }
        let step = std::slice::from_ref(operation);
        if anchor_deleted(&current, step)? {
            return Ok(true);
        }
        current = project_anchor(&current, step)?;
    }
    Ok(false)
}

/// The extraction anchor `anchor` with the corners and placement
/// `placement` sets.
fn placed_anchor(anchor: &Value, placement: &PlaceOperation) -> Value {
    let mut anchor = anchor.clone();
    if let (Some(from), Some(to)) = (placement.from, placement.to) {
        let [a, b] = placement.offsets.unwrap_or_default();
        anchor["from"] = json!({"col":from.0 - 1,"row":from.1 - 1,"colOff":a.0,"rowOff":a.1});
        anchor["to"] = json!({"col":to.0 - 1,"row":to.1 - 1,"colOff":b.0,"rowOff":b.1});
    }
    if let Some(edit_as) = &placement.placement {
        anchor["edit_as"] = json!(edit_as);
    }
    anchor
}

/// Project captured DrawingML anchors with exactly the same placement rules as
/// native writeback. Coordinates and offsets remain in their original units;
/// an offset of 0 is left out, as an extracted anchor leaves it out.
pub(crate) fn project_anchor(anchor: &Value, operations: &[StructuralOperation]) -> Result<Value> {
    let mut result = anchor.clone();
    if anchor["kind"] == "absoluteAnchor" {
        return Ok(result);
    }
    let corner = |key: &str| -> Result<Corner> {
        let mut out = [(0, false); 2];
        for (axis, name) in ["col", "row"].iter().enumerate() {
            out[axis] = (
                u32::try_from(
                    anchor[key][name]
                        .as_u64()
                        .context("invalid drawing marker")?,
                )?,
                anchor[key][format!("{name}Off")].as_i64().unwrap_or(0) == 0,
            );
        }
        Ok(out)
    };
    let placement = if anchor["kind"] == "oneCellAnchor" {
        "oneCell"
    } else {
        anchor["edit_as"].as_str().unwrap_or("twoCell")
    };
    let from = corner("from")?;
    let to = anchor.get("to").map(|_| corner("to")).transpose()?;
    let (from, to) = move_corners(placement, from, to, operations)?;
    for (key, point) in [("from", Some(from)), ("to", to)] {
        if let Some(point) = point {
            for (axis, name) in ["col", "row"].iter().enumerate() {
                result[key][name] = json!(point[axis].0);
                if point[axis].1
                    && let Some(marker) = result[key].as_object_mut()
                {
                    marker.remove(&format!("{name}Off"));
                }
            }
        }
    }
    Ok(result)
}

/// Moves an anchor whose `from` and `to` markers hold XDR `col`, `colOff`,
/// `row` and `rowOff` (drawings, form controls and embedded objects), adding
/// the edits of their text.
pub(super) fn move_marker_anchor(
    anchor: Node<'_, '_>,
    placement: &str,
    operations: &[StructuralOperation],
    edits: &mut Vec<(std::ops::Range<usize>, String)>,
) -> Result<()> {
    type Marker<'a, 'input> = [(Node<'a, 'input>, Node<'a, 'input>); 2];
    let marker = |name: &str| -> Option<Marker<'_, '_>> {
        let marker = anchor
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == name)?;
        let part = |index: &str, offset: &str| {
            child_ns(marker, XDR, index).zip(child_ns(marker, XDR, offset))
        };
        Some([part("col", "colOff")?, part("row", "rowOff")?])
    };
    let Some(from) = marker("from") else {
        return Ok(());
    };
    let to = marker("to");
    let corner = |marker: &Marker<'_, '_>| -> Result<Corner> {
        let mut corner = [(0, false); 2];
        for (axis, (index, offset)) in marker.iter().enumerate() {
            corner[axis] = (
                index
                    .text()
                    .context("anchor coordinate is empty")?
                    .trim()
                    .parse()?,
                offset.text().is_some_and(|text| text.trim() == "0"),
            );
        }
        Ok(corner)
    };
    let (moved_from, moved_to) = move_corners(
        placement,
        corner(&from)?,
        to.as_ref().map(corner).transpose()?,
        operations,
    )?;
    let mut set = |node: Node<'_, '_>, value: String| -> Result<()> {
        let text = node
            .children()
            .find(Node::is_text)
            .context("anchor coordinate has no text")?;
        if text.text() != Some(value.as_str()) {
            edits.push((text.range(), value));
        }
        Ok(())
    };
    for (marker, moved) in std::iter::once((from, moved_from)).chain(to.zip(moved_to)) {
        for ((index, offset), (new_index, reset)) in marker.into_iter().zip(moved) {
            set(index, new_index.to_string())?;
            if reset {
                set(offset, "0".to_owned())?;
            }
        }
    }
    Ok(())
}

/// A shape can show a cell's value (`textlink="$B$5"` or `Data!$B$5`); the link
/// follows the cell like a formula of the sheet holding the drawing.
pub(super) fn rewrite_text_links(original: &str, sheet: &str, moves: &Moves<'_>) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let mut attributes = AttributeEdits::default();
    for node in doc.descendants() {
        let Some(link) = node.attribute("textlink").filter(|link| !link.is_empty()) else {
            continue;
        };
        let rewritten = moves.rewrite(link, Some(sheet))?;
        if rewritten != link {
            attributes.set(original, node, "textlink", &rewritten)?;
        }
    }
    apply_edits(original, attributes.into_edits().collect(), vec![])
}

pub(super) fn worksheet_drawing(
    parts: &BTreeMap<String, Vec<u8>>,
    worksheet_part: &str,
    worksheet_xml: &str,
) -> Result<Option<(String, String)>> {
    let doc = xml(worksheet_xml.as_bytes())?;
    let Some(drawing) = doc
        .root_element()
        .children()
        .find(|node| node.has_tag_name((NS, "drawing")))
    else {
        return Ok(None);
    };
    let relationship_id = drawing
        .attribute((REL, "id"))
        .context("worksheet drawing relationship ID missing")?;
    let rels_part = relationships_part(worksheet_part)?;
    let rels = parts
        .get(&rels_part)
        .context("worksheet drawing relationships missing")?;
    let rels = std::str::from_utf8(rels)?;
    let (kind, target) =
        relationship(rels, relationship_id)?.context("worksheet drawing relationship missing")?;
    ensure!(
        kind.ends_with("/drawing"),
        "worksheet drawing relationship has wrong type"
    );
    let drawing_part = relationship_target(worksheet_part, &target)?;
    ensure!(
        drawing_part.starts_with("xl/drawings/") && parts.contains_key(&drawing_part),
        "worksheet drawing target is missing or outside drawings"
    );
    Ok(Some((
        drawing_part.clone(),
        relationships_part(&drawing_part)?,
    )))
}

/// The first `{directory}/{prefix}{n}.xml` for which `exists` is false.
pub(super) fn next_part(exists: impl Fn(&str) -> bool, directory: &str, prefix: &str) -> String {
    let mut number = 1u32;
    loop {
        let candidate = format!("{directory}/{prefix}{number}.xml");
        if !exists(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

pub(super) fn ensure_content_type(
    original: &str,
    extension: &str,
    content_type: &str,
    part_name: Option<&str>,
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    ensure!(
        doc.root_element().has_tag_name((CONTENT_TYPES, "Types")),
        "invalid content types part"
    );
    let exists = if let Some(part_name) = part_name {
        doc.root_element().children().any(|node| {
            node.has_tag_name((CONTENT_TYPES, "Override"))
                && node.attribute("PartName") == Some(part_name)
        })
    } else {
        doc.root_element().children().any(|node| {
            node.has_tag_name((CONTENT_TYPES, "Default"))
                && node
                    .attribute("Extension")
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
    };
    if exists {
        return Ok(original.to_owned());
    }
    let element = if let Some(part_name) = part_name {
        format!(
            r#"<Override PartName="{}" ContentType="{}"/>"#,
            xml_attr(part_name),
            xml_attr(content_type)
        )
    } else {
        format!(
            r#"<Default Extension="{}" ContentType="{}"/>"#,
            xml_attr(extension),
            xml_attr(content_type)
        )
    };
    append_xml_child(original, &element)
}

/// A media part name for `bytes`: one `part` gives no bytes for, or these bytes.
pub(super) fn next_media_name<'a>(
    part: impl Fn(&str) -> Option<&'a Vec<u8>>,
    requested: &str,
    bytes: &[u8],
) -> Result<String> {
    let file = requested
        .rsplit('/')
        .next()
        .context("image asset filename missing")?;
    ensure!(
        file.len() >= 4 && file[file.len() - 4..].eq_ignore_ascii_case(".png"),
        "image asset must be PNG"
    );
    let stem = &file[..file.len() - 4];
    for suffix in 0..u32::MAX {
        let name = if suffix == 0 {
            format!("xl/media/{file}")
        } else {
            format!("xl/media/{stem}-{suffix}.png")
        };
        match part(&name) {
            None => return Ok(name),
            Some(existing) if existing == bytes => return Ok(name),
            Some(_) => {}
        }
    }
    bail!("image media name space exhausted")
}

pub(super) fn next_doc_pr_id(drawing: &str) -> Result<u32> {
    let doc = xml(drawing.as_bytes())?;
    let mut max = 0u32;
    for node in doc
        .descendants()
        .filter(|node| node.has_tag_name((XDR, "cNvPr")))
    {
        let id: u32 = node
            .attribute("id")
            .context("drawing non-visual ID missing")?
            .parse()?;
        max = max.max(id);
    }
    max.checked_add(1).context("drawing non-visual ID overflow")
}

/// The names of the drawing objects in `drawing`, a drawing part or anchors of one.
pub(super) fn picture_names(drawing: &str) -> Result<BTreeSet<String>> {
    let doc = xml(drawing.as_bytes())?;
    Ok(doc
        .descendants()
        .filter(|node| node.has_tag_name((XDR, "cNvPr")))
        .filter_map(|node| node.attribute("name").map(str::to_owned))
        .collect())
}

/// The name of a picture added beside the drawing objects named `names`.
pub(super) fn picture_name(
    names: &BTreeSet<String>,
    requested: Option<&str>,
    asset: &str,
) -> Result<String> {
    let default = asset
        .rsplit('/')
        .next()
        .context("image asset filename missing")?;
    let base = requested.unwrap_or(default);
    ensure!(!base.trim().is_empty(), "image name must be nonempty");
    if requested.is_some() {
        ensure!(!names.contains(base), "duplicate drawing name");
        return Ok(base.to_owned());
    }
    for suffix in 0..u32::MAX {
        let candidate = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base} ({suffix})")
        };
        if !names.contains(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("drawing name space exhausted")
}

pub(super) fn image_anchor_xml(
    operation: &ImageOperation,
    relationship_id: &str,
    doc_pr_id: u32,
    name: &str,
) -> String {
    let from_col = operation.from.column - 1;
    let from_row = operation.from.row - 1;
    let to_col = operation.to.column - 1;
    let to_row = operation.to.row - 1;
    format!(
        r#"<xdr:twoCellAnchor xmlns:xdr="{XDR}" xmlns:a="{DRAWING}" xmlns:r="{REL}" editAs="oneCell"><xdr:from><xdr:col>{from_col}</xdr:col><xdr:colOff>{from_col_off}</xdr:colOff><xdr:row>{from_row}</xdr:row><xdr:rowOff>{from_row_off}</xdr:rowOff></xdr:from><xdr:to><xdr:col>{to_col}</xdr:col><xdr:colOff>{to_col_off}</xdr:colOff><xdr:row>{to_row}</xdr:row><xdr:rowOff>{to_row_off}</xdr:rowOff></xdr:to><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{doc_pr_id}" name="{name}"/><xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="{relationship_id}"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/></xdr:twoCellAnchor>"#,
        from_col_off = operation.from.column_offset,
        from_row_off = operation.from.row_offset,
        to_col_off = operation.to.column_offset,
        to_row_off = operation.to.row_offset,
        name = xml_attr(name),
        relationship_id = xml_attr(relationship_id),
    )
}

pub(super) fn new_drawing() -> String {
    format!(r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{DRAWING}" xmlns:r="{REL}"></xdr:wsDr>"#)
}

/// Adds `<drawing r:id>` to a worksheet. The element takes the worksheet's own
/// prefix and declares the relationships namespace itself, so the root element
/// (and the XML declaration before it) stay as written.
pub(super) fn append_drawing_reference(worksheet: &str, relationship_id: &str) -> Result<String> {
    let doc = xml(worksheet.as_bytes())?;
    let root = doc.root_element();
    let prefix = element_tag(&worksheet[root.range()])?
        .strip_suffix("worksheet")
        .context("invalid worksheet tag")?;
    let drawing = format!(
        r#"<{prefix}drawing xmlns:r="{REL}" r:id="{}"/>"#,
        xml_attr(relationship_id)
    );
    insert_before_root_children(
        worksheet,
        &drawing,
        &[
            (NS, "legacyDrawing"),
            (NS, "legacyDrawingHF"),
            (NS, "picture"),
            (NS, "oleObjects"),
            (NS, "controls"),
            (NS, "webPublishItems"),
            (NS, "tableParts"),
            (NS, "extLst"),
        ],
    )
}

pub(super) fn append_drawing_anchor(drawing: &str, anchor: &str) -> Result<String> {
    insert_before_root_children(drawing, anchor, &[(XDR, "extLst")])
}

/// The relationships of a drawing part as pictures are added to it, read
/// once: each picture takes the first ID not in use, and the relationships
/// added are written in after those there, in order, as one after the other.
struct DrawingRelationships {
    part: String,
    current: Option<String>,
    used: BTreeSet<String>,
    next: u32,
    added: String,
}

impl DrawingRelationships {
    fn read(
        parts: &BTreeMap<String, Vec<u8>>,
        patched: &BTreeMap<String, Vec<u8>>,
        drawing_part: &str,
    ) -> Result<Self> {
        let part = relationships_part(drawing_part)?;
        let current = patched
            .get(&part)
            .or_else(|| parts.get(&part))
            .map(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
            .transpose()?;
        let used = match &current {
            Some(rels) => relationship_ids(rels)?,
            None => BTreeSet::new(),
        };
        Ok(Self {
            part,
            current,
            used,
            next: 1,
            added: String::new(),
        })
    }

    /// The ID of a relationship of the drawing to the picture `image_part`.
    fn add(&mut self, drawing_part: &str, image_part: &str) -> Result<String> {
        let target = relative_target(drawing_part, image_part)?;
        // What `append_relationship` checks of the part, which pictures added
        // do not change.
        if self.added.is_empty()
            && let Some(rels) = &self.current
        {
            ensure!(
                xml(rels.as_bytes())?
                    .root_element()
                    .has_tag_name((PKG_REL, "Relationships")),
                "invalid relationships part"
            );
            ensure!(rels.contains("</"), "relationships root has no closing tag");
        }
        let id = loop {
            ensure!(self.next < u32::MAX, "relationship ID space exhausted");
            let candidate = format!("rId{}", self.next);
            self.next += 1;
            if !self.used.contains(&candidate) {
                break candidate;
            }
        };
        self.used.insert(id.clone());
        self.added.push_str(&format!(
            r#"<Relationship Id="{}" Type="{}" Target="{}"/>"#,
            xml_attr(&id),
            xml_attr("http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"),
            xml_attr(&target)
        ));
        Ok(id)
    }

    fn write(self, patched: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
        if self.added.is_empty() {
            return Ok(());
        }
        let mut output = self
            .current
            .unwrap_or_else(|| format!(r#"<Relationships xmlns="{PKG_REL}"></Relationships>"#));
        let end = output
            .rfind("</")
            .context("relationships root has no closing tag")?;
        output.insert_str(end, &self.added);
        patched.insert(self.part, output.into_bytes());
        Ok(())
    }
}

pub(super) fn apply_image_operations(
    parts: &BTreeMap<String, Vec<u8>>,
    patched: &mut BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    structural: &[StructuralOperation],
    images: &[ImageOperation],
    placements: &[PlaceOperation],
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for sheet in sheets {
        let sheet_name = string(&sheet["name"])?;
        let worksheet_part = string(&sheet["part"])?;
        let sheet_operations: Vec<_> = structural
            .iter()
            .filter(|operation| operation.sheet == sheet_name)
            .cloned()
            .collect();
        let sheet_images: Vec<_> = images
            .iter()
            .filter(|operation| operation.sheet == sheet_name)
            .collect();
        if sheet_operations.is_empty() && sheet_images.is_empty() {
            continue;
        }
        let worksheet_xml = patched
            .get(worksheet_part)
            .or_else(|| parts.get(worksheet_part))
            .context("worksheet part missing")?;
        let worksheet_xml = std::str::from_utf8(worksheet_xml)?.to_owned();
        let existing = worksheet_drawing(parts, worksheet_part, &worksheet_xml)?;
        if existing.is_none() && sheet_images.is_empty() {
            continue;
        }
        let drawing_part = if let Some(existing) = existing {
            existing.0
        } else {
            let drawing_part = next_part(
                |name| patched.contains_key(name) || parts.contains_key(name),
                "xl/drawings",
                "drawing",
            );
            let drawing_rels_part = relationships_part(&drawing_part)?;
            patched.insert(drawing_part.clone(), new_drawing().into_bytes());
            patched.insert(
                drawing_rels_part.clone(),
                format!(r#"<Relationships xmlns="{PKG_REL}"></Relationships>"#).into_bytes(),
            );
            let worksheet_rels_part = relationships_part(worksheet_part)?;
            let current = patched
                .get(&worksheet_rels_part)
                .or_else(|| parts.get(&worksheet_rels_part))
                .map(|bytes| std::str::from_utf8(bytes))
                .transpose()?;
            let relationship_id = relationship_id(current)?;
            let target = relative_target(worksheet_part, &drawing_part)?;
            let relationships = append_relationship(
                current,
                &relationship_id,
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing",
                &target,
            )?;
            patched.insert(worksheet_rels_part, relationships.into_bytes());
            patched.insert(
                worksheet_part.to_owned(),
                append_drawing_reference(&worksheet_xml, &relationship_id)?.into_bytes(),
            );
            drawing_part
        };
        let drawing_bytes = patched
            .get(&drawing_part)
            .or_else(|| parts.get(&drawing_part))
            .context("drawing part missing")?;
        let drawing = std::str::from_utf8(drawing_bytes)?;
        // The objects placements move, which they delete or keep.
        let placed: BTreeSet<String> = placements
            .iter()
            .filter_map(|placement| placement.drawing.rsplit_once('#'))
            .filter(|(part, _)| *part == drawing_part)
            .map(|(_, number)| number.to_owned())
            .collect();
        let mut drawing = rewrite_drawing_anchors(drawing, &sheet_operations, &placed)?;
        let mut doc_pr_id = next_doc_pr_id(&drawing)?;
        // The drawing's relationships and the names of its objects, read when
        // the first picture is added; the pictures go in after the objects
        // there, together.
        let mut relationships: Option<DrawingRelationships> = None;
        let mut names: Option<BTreeSet<String>> = None;
        let mut anchors = String::new();
        for operation in sheet_images {
            let bytes = assets
                .get(&operation.asset)
                .with_context(|| format!("image asset missing: {}", operation.asset))?;
            validate_image_asset(&operation.asset, bytes)?;
            let media_part = next_media_name(
                |name| patched.get(name).or_else(|| parts.get(name)),
                &operation.asset,
                bytes,
            )?;
            if !patched.contains_key(&media_part) && !parts.contains_key(&media_part) {
                patched.insert(media_part.clone(), bytes.clone());
            }
            let relationship_id = match &mut relationships {
                Some(relationships) => relationships,
                None => {
                    relationships.insert(DrawingRelationships::read(parts, patched, &drawing_part)?)
                }
            }
            .add(&drawing_part, &media_part)?;
            let names = match &mut names {
                Some(names) => names,
                None => names.insert(picture_names(&drawing)?),
            };
            let name = picture_name(names, operation.name.as_deref(), &operation.asset)?;
            // Like a picture Excel inserts, it moves with its top left cell
            // when rows and columns change after it.
            let later: Vec<_> = structural[operation.preceding..]
                .iter()
                .filter(|later| later.sheet == sheet_name)
                .cloned()
                .collect();
            let anchor = rewrite_drawing_anchors(
                &image_anchor_xml(operation, &relationship_id, doc_pr_id, &name),
                &later,
                &BTreeSet::new(),
            )?;
            // The name as the drawing reads it back.
            names.extend(picture_names(&anchor)?);
            anchors.push_str(&anchor);
            doc_pr_id = doc_pr_id
                .checked_add(1)
                .context("drawing non-visual ID overflow")?;
        }
        if let Some(relationships) = relationships {
            relationships.write(patched)?;
        }
        if !anchors.is_empty() {
            drawing = append_drawing_anchor(&drawing, &anchors)?;
        }
        let drawing_part_name = format!("/{drawing_part}");
        patched.insert(drawing_part, drawing.into_bytes());
        let content_types = patched
            .get("[Content_Types].xml")
            .or_else(|| parts.get("[Content_Types].xml"))
            .context("content types part missing")?;
        let content_types = std::str::from_utf8(content_types)?;
        let content_types = ensure_content_type(content_types, "png", "image/png", None)?;
        let content_types = ensure_content_type(
            &content_types,
            "xml",
            "application/vnd.openxmlformats-officedocument.drawing+xml",
            Some(&drawing_part_name),
        )?;
        patched.insert("[Content_Types].xml".into(), content_types.into_bytes());
    }
    Ok(())
}
