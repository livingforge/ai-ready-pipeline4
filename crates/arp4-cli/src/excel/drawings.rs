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

pub(super) fn rewrite_drawing_anchors(
    original: &str,
    operations: &[StructuralOperation],
) -> Result<String> {
    if operations.is_empty() {
        return Ok(original.to_owned());
    }
    let doc = xml(original.as_bytes())?;
    let mut edits = vec![];
    for anchor in doc.descendants().filter(|node| {
        node.has_tag_name((XDR, "oneCellAnchor")) || node.has_tag_name((XDR, "twoCellAnchor"))
    }) {
        for marker_name in ["from", "to"] {
            let Some(marker) = child_ns(anchor, XDR, marker_name) else {
                continue;
            };
            let Some(col) = child_ns(marker, XDR, "col") else {
                continue;
            };
            let Some(row) = child_ns(marker, XDR, "row") else {
                continue;
            };
            for (node, is_row) in [(col, false), (row, true)] {
                let text = node.text().context("drawing anchor coordinate is empty")?;
                let position: u32 = text.parse()?;
                let position = position
                    .checked_add(1)
                    .context("drawing anchor coordinate overflow")?;
                let mapped = map_anchor_index(position, operations, is_row)?;
                let text_node = node
                    .children()
                    .find(Node::is_text)
                    .context("drawing anchor coordinate has no text")?;
                edits.push((text_node.range(), (mapped - 1).to_string()));
            }
        }
    }
    edits.sort_by_key(|(range, _)| range.start);
    for pair in edits.windows(2) {
        ensure!(
            pair[0].0.end <= pair[1].0.start,
            "overlapping drawing edits"
        );
    }
    let mut result = original.to_owned();
    for (range, replacement) in edits.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
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

pub(super) fn next_part(
    parts: &BTreeMap<String, Vec<u8>>,
    directory: &str,
    prefix: &str,
) -> String {
    let mut number = 1u32;
    loop {
        let candidate = format!("{directory}/{prefix}{number}.xml");
        if !parts.contains_key(&candidate) {
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

pub(super) fn next_media_name(
    parts: &BTreeMap<String, Vec<u8>>,
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
        match parts.get(&name) {
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

pub(super) fn picture_name(drawing: &str, requested: Option<&str>, asset: &str) -> Result<String> {
    let doc = xml(drawing.as_bytes())?;
    let names: BTreeSet<String> = doc
        .descendants()
        .filter(|node| node.has_tag_name((XDR, "cNvPr")))
        .filter_map(|node| node.attribute("name").map(str::to_owned))
        .collect();
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
        r#"<xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>{from_col}</xdr:col><xdr:colOff>{from_col_off}</xdr:colOff><xdr:row>{from_row}</xdr:row><xdr:rowOff>{from_row_off}</xdr:rowOff></xdr:from><xdr:to><xdr:col>{to_col}</xdr:col><xdr:colOff>{to_col_off}</xdr:colOff><xdr:row>{to_row}</xdr:row><xdr:rowOff>{to_row_off}</xdr:rowOff></xdr:to><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{doc_pr_id}" name="{name}"/><xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr></xdr:nvPicPr><xdr:blipFill><a:blip r:embed="{relationship_id}"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/></xdr:twoCellAnchor>"#,
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

pub(super) fn ensure_relationship_namespace(worksheet: &str) -> Result<String> {
    let (opening, _) = xml_opening(worksheet)?;
    if opening.contains("xmlns:r=") {
        return Ok(worksheet.to_owned());
    }
    let replacement = format!("{} xmlns:r=\"{}\">", opening.trim_end_matches('>'), REL);
    Ok(worksheet.replacen(opening, &replacement, 1))
}

pub(super) fn append_drawing_reference(worksheet: &str, relationship_id: &str) -> Result<String> {
    let worksheet = ensure_relationship_namespace(worksheet)?;
    let drawing = format!(r#"<drawing r:id="{}"/>"#, xml_attr(relationship_id));
    insert_before_root_children(
        &worksheet,
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

pub(super) fn update_drawing_relationships(
    parts: &BTreeMap<String, Vec<u8>>,
    patched: &mut BTreeMap<String, Vec<u8>>,
    drawing_part: &str,
    image_part: &str,
) -> Result<String> {
    let rels_part = relationships_part(drawing_part)?;
    let current = patched
        .get(&rels_part)
        .or_else(|| parts.get(&rels_part))
        .map(|bytes| std::str::from_utf8(bytes))
        .transpose()?;
    let id = relationship_id(current)?;
    let target = relative_target(drawing_part, image_part)?;
    let rels = append_relationship(
        current,
        &id,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
        &target,
    )?;
    patched.insert(rels_part, rels.into_bytes());
    Ok(id)
}

pub(super) fn apply_image_operations(
    parts: &BTreeMap<String, Vec<u8>>,
    patched: &mut BTreeMap<String, Vec<u8>>,
    sheets: &[Value],
    structural: &[StructuralOperation],
    images: &[ImageOperation],
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
            let mut available = parts.clone();
            for (name, value) in patched.iter() {
                available.insert(name.clone(), value.clone());
            }
            let drawing_part = next_part(&available, "xl/drawings", "drawing");
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
        let mut drawing = rewrite_drawing_anchors(drawing, &sheet_operations)?;
        let mut current_parts = parts.clone();
        for (name, value) in patched.iter() {
            current_parts.insert(name.clone(), value.clone());
        }
        let mut doc_pr_id = next_doc_pr_id(&drawing)?;
        for operation in sheet_images {
            let bytes = assets
                .get(&operation.asset)
                .with_context(|| format!("image asset missing: {}", operation.asset))?;
            validate_image_asset(&operation.asset, bytes)?;
            let media_part = next_media_name(&current_parts, &operation.asset, bytes)?;
            if !current_parts.contains_key(&media_part) {
                patched.insert(media_part.clone(), bytes.clone());
                current_parts.insert(media_part.clone(), bytes.clone());
            }
            let relationship_id =
                update_drawing_relationships(&current_parts, patched, &drawing_part, &media_part)?;
            let name = picture_name(&drawing, operation.name.as_deref(), &operation.asset)?;
            drawing = append_drawing_anchor(
                &drawing,
                &image_anchor_xml(operation, &relationship_id, doc_pr_id, &name),
            )?;
            doc_pr_id = doc_pr_id
                .checked_add(1)
                .context("drawing non-visual ID overflow")?;
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
