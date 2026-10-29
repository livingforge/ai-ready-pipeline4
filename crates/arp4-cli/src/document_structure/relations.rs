//! References authored during structure interpretation; also checked in saved packets.
use super::*;

/// An element as its references read it, held typed or as JSON: its members
/// and the IDs of its cells.
pub(super) trait Node {
    fn members(&self) -> &Value;
    fn members_mut(&mut self) -> &mut Value;
    /// The IDs of its cells; none when its cells are not a list.
    fn cell_ids(&self) -> Option<Vec<&str>>;
}

impl Node for Value {
    fn members(&self) -> &Value {
        self
    }
    fn members_mut(&mut self) -> &mut Value {
        self
    }
    fn cell_ids(&self) -> Option<Vec<&str>> {
        Some(
            self["cells"]
                .as_array()?
                .iter()
                .filter_map(|c| c["id"].as_str())
                .collect(),
        )
    }
}

impl Node for Element {
    fn members(&self) -> &Value {
        &self.fields
    }
    fn members_mut(&mut self) -> &mut Value {
        &mut self.fields
    }
    fn cell_ids(&self) -> Option<Vec<&str>> {
        Some(self.cells.iter().map(|c| &*c.id).collect())
    }
}

/// Keeps each table's descriptions only to text elements still on its sheet,
/// and drops the list when none is left. Returns the IDs of the tables that
/// lost one.
pub(super) fn prune_descriptions<T: Node>(elements: &mut [T]) -> Vec<String> {
    let texts: BTreeSet<(String, String)> = elements
        .iter()
        .map(Node::members)
        .filter(|e| e["kind"] == "text")
        .filter_map(|e| {
            Some((
                e["sheet"].as_str()?.to_owned(),
                e["id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let mut pruned = vec![];
    for element in elements.iter_mut() {
        let element = element.members_mut();
        let sheet = element["sheet"].as_str().unwrap_or_default().to_owned();
        let Some(descriptions) = element
            .get_mut("descriptions")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let before = descriptions.len();
        descriptions.retain(|id| {
            id.as_str()
                .is_some_and(|id| texts.contains(&(sheet.clone(), id.to_owned())))
        });
        let (after, empty) = (descriptions.len(), descriptions.is_empty());
        if empty {
            element.as_object_mut().unwrap().remove("descriptions");
        }
        if after != before {
            pruned.push(element["id"].as_str().unwrap_or_default().to_owned());
        }
    }
    pruned
}

/// Keeps each table's parent only while it is still a table on the same sheet,
/// and its context only to cells that table still holds. Returns the IDs of the
/// elements whose parent was dropped.
pub(super) fn prune_parents<T: Node>(elements: &mut [T]) -> Vec<String> {
    // Only the tables named as parents are read, as each holds a set of its
    // cell IDs.
    let named: BTreeSet<String> = elements
        .iter()
        .filter_map(|e| e.members().get("parent")?["element"].as_str())
        .map(str::to_owned)
        .collect();
    let tables: BTreeMap<String, (Value, BTreeSet<String>)> = elements
        .iter()
        .filter(|e| {
            e.members()["kind"] == "table"
                && e.members()["id"]
                    .as_str()
                    .is_some_and(|id| named.contains(id))
        })
        .filter_map(|e| {
            let cells = e.cell_ids()?.into_iter().map(str::to_owned).collect();
            let e = e.members();
            Some((e["id"].as_str()?.to_owned(), (e["sheet"].clone(), cells)))
        })
        .collect();
    let mut dropped = vec![];
    for element in elements.iter_mut() {
        let element = element.members_mut();
        let Some(parent) = element.get("parent") else {
            continue;
        };
        let found = parent["element"]
            .as_str()
            .and_then(|id| tables.get(id))
            .filter(|(sheet, _)| *sheet == element["sheet"] && parent["element"] != element["id"]);
        match found {
            Some((_, cells)) => {
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

/// Checks the references of `elements` and of the visuals and regions of
/// `structure`, the other members of their interpretation.
pub(super) fn validate_elements<T: Node>(elements: &[T], structure: &Value) -> Result<()> {
    let value = structure;
    let by_id: BTreeMap<_, _> = elements
        .iter()
        .map(|e| Ok((string(&e.members()["id"])?, e)))
        .collect::<Result<_>>()?;
    ensure!(by_id.len() == elements.len(), "duplicate element ID");
    for node in elements {
        let element = node.members();
        if let Some(descriptions) = element.get("descriptions") {
            for id in array(descriptions)? {
                let description = by_id
                    .get(string(id)?)
                    .context("unknown table description")?
                    .members();
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
            let container = *by_id
                .get(string(&parent["element"])?)
                .context("unknown parent table")?;
            let ids = container.cell_ids().context("expected array")?;
            let container = container.members();
            ensure!(
                container["kind"] == "table" && container["id"] != element["id"],
                "table parent must reference another table"
            );
            ensure!(
                container["sheet"] == element["sheet"],
                "table parent on different sheet"
            );
            let cells: BTreeSet<_> = ids.into_iter().collect();
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
        let mut current: &Value = element.members();
        while let Some(parent) = current.get("parent") {
            let id = string(&parent["element"])?;
            ensure!(seen.insert(id), "cyclic table parent relationship");
            current = by_id[id].members();
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

    fn validate(value: &Value) -> Result<()> {
        validate_elements(array(&value["elements"])?, value)
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
