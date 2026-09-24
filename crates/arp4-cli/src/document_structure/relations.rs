//! References authored during structure interpretation; also checked in saved packets.
use super::*;

pub(super) fn validate(value: &Value) -> Result<()> {
    let elements = array(&value["elements"])?;
    let by_id: BTreeMap<_, _> = elements
        .iter()
        .map(|e| Ok((string(&e["id"])?, e)))
        .collect::<Result<_>>()?;
    ensure!(by_id.len() == elements.len(), "duplicate element ID");
    for element in elements {
        if let Some(descriptions) = element.get("descriptions") {
            for id in array(descriptions)? {
                let description = by_id
                    .get(string(id)?)
                    .context("unknown table description")?;
                ensure!(
                    description["kind"] == "text",
                    "table description must reference a text element"
                );
                ensure!(
                    description["sheet"] == element["sheet"],
                    "table description on different sheet"
                );
            }
        }
    }
    for visual in array(&value["visuals"])? {
        for evidence in array(&visual["evidence"])? {
            if array(&visual["sources"])?.contains(evidence) {
                ensure!(
                    visual["kind"] == "image",
                    "direct image evidence requires an image visual"
                );
                continue;
            }
            let region = array(&value["regions"])?
                .iter()
                .find(|r| r["id"] == *evidence)
                .context("unknown visual image evidence")?;
            ensure!(
                visual["sheet"].is_null() || visual["sheet"] == region["sheet"],
                "visual evidence on different sheet"
            );
        }
        let Some(graph) = visual.get("graph") else {
            continue;
        };
        let mut nodes = BTreeSet::new();
        for node in array(&graph["nodes"])? {
            ensure!(
                nodes.insert(string(&node["id"])?),
                "duplicate graph node ID"
            );
        }
        for edge in array(&graph["edges"])? {
            ensure!(
                nodes.contains(string(&edge["from"])?) && nodes.contains(string(&edge["to"])?),
                "unknown graph edge endpoint"
            );
        }
        let sources = array(&visual["sources"])?;
        let evidence = array(&visual["evidence"])?;
        if graph["reading"]["method"] == "vision" {
            ensure!(!evidence.is_empty(), "vision graph requires image evidence");
        }
        for item in array(&graph["nodes"])?
            .iter()
            .chain(array(&graph["edges"])?)
        {
            for source in array(&item["sources"])? {
                ensure!(
                    sources.contains(source) || evidence.contains(source),
                    "graph source outside its visual"
                );
            }
        }
    }
    Ok(())
}
