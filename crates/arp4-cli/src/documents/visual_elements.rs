//! Body references to captured visual objects. Facts, assets and interpretation
//! remain outside the body; no picture or chart data is duplicated into text.
use super::*;

pub(super) fn refresh(
    layout: &mut Value,
    extraction: &Value,
    operations: &[excel::StructuralOperation],
) -> Result<()> {
    for (si, sheet) in array(&extraction["sheets"])?.iter().enumerate() {
        let mut visuals = json!({});
        let operations: Vec<_> = operations
            .iter()
            .filter(|op| op.sheet == sheet["name"].as_str().unwrap_or(""))
            .cloned()
            .collect();
        for (di, drawing) in sheet["drawings"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let drawing_id = string(&drawing["id"])?;
            let id = elements::token("visual", &[drawing_id]);
            let kind = if drawing["kind"] == "picture" {
                "image"
            } else if drawing["chart_part"].is_string() {
                "chart"
            } else {
                "drawing"
            };
            // A slide object sits at a place on the slide, which no row operation moves.
            let slide = drawing["anchor"]["kind"] == "slide";
            let anchor = if slide {
                drawing["anchor"].clone()
            } else {
                excel::project_anchor(&drawing["anchor"], &operations)?
            };
            let order_basis = if anchor["from"]["row"].is_u64() && anchor["from"]["col"].is_u64() {
                "cell_anchor"
            } else {
                "unpositioned"
            };
            let mut reference = json!({"kind":kind,"source":format!("/sheets/{si}/drawings/{di}"),"drawing_id":drawing_id,"anchor":anchor,"order_basis":order_basis});
            if let Some(asset) = drawing["image"]["asset"].as_str() {
                let metadata = array(&extraction["assets"])?
                    .iter()
                    .find(|a| a["path"] == asset)
                    .context("visual asset missing")?;
                ensure!(
                    metadata["sha256"] == drawing["image"]["sha256"],
                    "visual asset hash mismatch"
                );
                reference["asset"] = json!(format!("assets/{asset}"));
            }
            if let Some(part) = drawing["chart_part"].as_str() {
                ensure!(
                    extraction["chart_parts"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|p| p["part"] == part)),
                    "visual chart part missing"
                );
                reference["chart_part"] = json!(part);
            }
            // Slide text is laid out with the slide's paragraphs, not as shape text.
            if !slide && drawing["text"].as_str().is_some_and(|s| !s.is_empty()) {
                let (_, raw) = drawing_id
                    .rsplit_once('#')
                    .context("drawing ID without part")?;
                reference["text_key"] = json!(format!("shape-{raw}"));
            }
            if let Some(group) = drawing["group"].as_str() {
                reference["parent"] = json!(elements::token("visual", &[group]));
            }
            ensure!(visuals.get(&id).is_none(), "duplicate visual ID");
            visuals[&id] = reference;
        }
        for v in visuals.as_object().unwrap().values() {
            if let Some(parent) = v["parent"].as_str() {
                ensure!(visuals.get(parent).is_some(), "visual parent missing");
            }
        }
        layout["sheets"][identity::page_id(sheet, si)]["visuals"] = visuals;
    }
    Ok(())
}

pub(super) fn position(visual: &Value) -> (u64, u64) {
    let marker = &visual["anchor"]["from"];
    match (marker["row"].as_u64(), marker["col"].as_u64()) {
        (Some(row), Some(column)) => (row.saturating_add(1), column.saturating_add(1)),
        _ => (u64::MAX, u64::MAX),
    }
}

pub(super) fn validate_references(page: &Value, sheet: &Value) -> Result<()> {
    let visuals = sheet["visuals"]
        .as_object()
        .context("visual bindings missing")?;
    let mut seen = BTreeSet::new();
    for element in array(&page["elements"])? {
        if let Some(reference) = element["ref"].as_str() {
            let visual = visuals.get(reference).context("unknown visual reference")?;
            ensure!(
                element["id"] == reference && element["type"] == visual["kind"],
                "visual reference kind or identity mismatch"
            );
            ensure!(seen.insert(reference), "duplicate visual reference");
        }
        if let Some(reference) = element["visual"].as_str() {
            let visual = visuals
                .get(reference)
                .context("text references unknown visual")?;
            let binding = &sheet["bindings"][string(&element["id"])?];
            ensure!(
                binding["block"] == "shapes" && binding["row"] == visual["text_key"],
                "visual text binding mismatch"
            );
        }
    }
    ensure!(
        seen.len() == visuals.len(),
        "visual reference coverage mismatch"
    );
    Ok(())
}
