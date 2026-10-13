//! References authored during structure interpretation; also checked in saved packets.
use super::*;

/// Keeps each table's parent only while it is still a table on the same sheet,
/// and its context only to cells that table still holds. Returns the IDs of the
/// elements whose parent was dropped.
pub(super) fn prune_parents(elements: &mut [Value]) -> Vec<String> {
    let tables: BTreeMap<String, (Value, BTreeSet<String>)> = elements
        .iter()
        .filter(|e| e["kind"] == "table")
        .filter_map(|e| {
            let cells = e["cells"]
                .as_array()?
                .iter()
                .filter_map(|c| c["id"].as_str().map(str::to_owned))
                .collect();
            Some((e["id"].as_str()?.to_owned(), (e["sheet"].clone(), cells)))
        })
        .collect();
    let mut dropped = vec![];
    for element in elements.iter_mut() {
        let Some(parent) = element.get("parent") else {
            continue;
        };
        let found = parent["element"]
            .as_str()
            .and_then(|id| tables.get(id))
            .filter(|(sheet, _)| *sheet == element["sheet"] && parent["element"] != element["id"]);
        match found {
            Some((_, cells)) => {
                let cells = cells.clone();
                if let Some(context) = element["parent"]["context"].as_array_mut() {
                    context.retain(|id| id.as_str().is_some_and(|id| cells.contains(id)));
                }
            }
            None => {
                element.as_object_mut().unwrap().remove("parent");
                dropped.push(element["id"].as_str().unwrap_or_default().to_owned());
            }
        }
    }
    dropped
}

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
        if let Some(parent) = element.get("parent") {
            let container = by_id
                .get(string(&parent["element"])?)
                .context("unknown parent table")?;
            ensure!(
                container["kind"] == "table" && container["id"] != element["id"],
                "table parent must reference another table"
            );
            ensure!(
                container["sheet"] == element["sheet"],
                "table parent on different sheet"
            );
            let cells: BTreeSet<_> = array(&container["cells"])?
                .iter()
                .filter_map(|cell| cell["id"].as_str())
                .collect();
            for id in array(&parent["context"])? {
                ensure!(
                    cells.contains(string(id)?),
                    "table context must reference a cell of its parent"
                );
            }
        }
    }
    // A table cannot be nested in itself through its ancestors.
    for element in elements {
        let mut seen = BTreeSet::new();
        let mut current: &Value = element;
        while let Some(parent) = current.get("parent") {
            let id = string(&parent["element"])?;
            ensure!(seen.insert(id), "cyclic table parent relationship");
            current = by_id[id];
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

#[cfg(test)]
mod tests {
    use super::*;

    fn table(id: &str, cells: &[&str], parent: Option<Value>) -> Value {
        let cells: Vec<_> = cells
            .iter()
            .map(|id| json!({"id":id,"address":"A1","role":"column_header","headers":[],"text_state":"read"}))
            .collect();
        let mut element = json!({"id":id,"kind":"table","sheet":"S","cells":cells,"evidence":[],
            "reading":{"method":"parser","actor":"arp4","reason":"test"}});
        if let Some(parent) = parent {
            element["parent"] = parent;
        }
        element
    }

    fn structure(elements: Vec<Value>) -> Value {
        json!({"elements":elements,"visuals":[],"regions":[]})
    }

    #[test]
    fn parents_must_be_other_tables_of_the_sheet_holding_the_context() {
        let outer = table("outer", &["o1", "o2"], None);
        let inner = |context: Value| {
            table(
                "inner",
                &["i1"],
                Some(json!({"element":"outer","context":context})),
            )
        };
        validate(&structure(vec![outer.clone(), inner(json!(["o1"]))])).unwrap();
        let error = validate(&structure(vec![outer.clone(), inner(json!(["i1"]))])).unwrap_err();
        assert!(error.to_string().contains("cell of its parent"), "{error}");
        let mut elsewhere = outer.clone();
        elsewhere["sheet"] = json!("T");
        let error = validate(&structure(vec![elsewhere, inner(json!([]))])).unwrap_err();
        assert!(error.to_string().contains("different sheet"), "{error}");
        let error = validate(&structure(vec![inner(json!([]))])).unwrap_err();
        assert!(error.to_string().contains("unknown parent"), "{error}");
        let mut circular = outer.clone();
        circular["parent"] = json!({"element":"inner","context":[]});
        let error = validate(&structure(vec![circular, inner(json!([]))])).unwrap_err();
        assert!(error.to_string().contains("cyclic"), "{error}");
    }

    #[test]
    fn pruning_drops_parents_that_are_gone_and_context_they_lost() {
        let mut elements = vec![
            table("outer", &["o1"], None),
            table(
                "inner",
                &["i1"],
                Some(json!({"element":"outer","context":["o1","o2"]})),
            ),
            table(
                "orphan",
                &["x1"],
                Some(json!({"element":"gone","context":["g1"]})),
            ),
        ];
        assert_eq!(prune_parents(&mut elements), ["orphan"]);
        assert_eq!(elements[1]["parent"]["context"], json!(["o1"]));
        assert!(elements[2].get("parent").is_none());
    }
}
