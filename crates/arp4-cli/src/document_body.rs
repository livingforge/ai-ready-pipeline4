//! Array bodies and their value-free layout metadata. Array positions are editing
//! slots, not identities: moving values by hand is an ordinary value edit.
use crate::data::{array, string};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

/// Separate editable values from identities, topology and source references.
/// The metadata keeps no place for the values, which [`join`] puts back
/// under the element's type or the cell's `value`; only a table cell holding
/// a formula keeps a null `formula`, the mark that its value is a formula.
pub fn split(elements: &Value) -> Result<(Value, Value)> {
    let mut metadata = elements.clone();
    let mut body = Vec::new();
    for e in metadata.as_array_mut().context("elements missing")? {
        let kind = string(&e["type"])?.to_owned();
        let value = match kind.as_str() {
            "text" | "formula" => e
                .as_object_mut()
                .unwrap()
                .remove(&kind)
                .context("element value missing")?,
            "table" => {
                let columns = array(&e["columns"])?.clone();
                let mut rows = Vec::new();
                for row in e["rows"].as_array_mut().context("table rows missing")? {
                    let mut values = vec![Value::Null; columns.len()];
                    for cell in row["cells"].as_array_mut().context("table cells missing")? {
                        let index = columns
                            .iter()
                            .position(|c| c["id"] == cell["column"])
                            .context("unknown table column")?;
                        let fields = cell.as_object_mut().unwrap();
                        values[index] = if fields.contains_key("formula") {
                            fields.insert("formula".into(), Value::Null)
                        } else {
                            fields.remove("value")
                        }
                        .context("cell value missing")?;
                    }
                    rows.push(json!(values));
                }
                json!(rows)
            }
            "blank" | "image" | "chart" | "drawing" => Value::Null,
            _ => bail!("unknown element type"),
        };
        body.push(json!({kind:value}));
    }
    Ok((json!(body), metadata))
}

/// Calls `visit` with each value of `body` and the element or table cell of
/// `metadata` it belongs to, in order, after checking that the body still has
/// the shape the metadata records. Missing table cells are null padding and
/// cannot be made writable by hand. Nothing is copied.
pub fn each(
    body: &Value,
    metadata: &Value,
    mut visit: impl FnMut(&Value, &Value) -> Result<()>,
) -> Result<()> {
    let body = array(body)?;
    let elements = array(metadata).context("element metadata missing")?;
    ensure!(
        body.len() == elements.len(),
        "element count changed; use ARP structural operations"
    );
    for (value, e) in body.iter().zip(elements) {
        let kind = string(&e["type"])?;
        let fields = value
            .as_object()
            .context("body element must be an object")?;
        ensure!(
            fields.len() == 1 && fields.contains_key(kind),
            "element kind changed; use ARP structural operations"
        );
        let value = &value[kind];
        match kind {
            "text" => visit(e, value)?,
            "formula" => {
                ensure!(value.is_string(), "formula must be a string");
                visit(e, value)?;
            }
            "table" => {
                let columns = array(&e["columns"])?;
                let rows = array(&e["rows"]).context("table metadata rows missing")?;
                let values = array(value)?;
                ensure!(
                    values.len() == rows.len(),
                    "table row count changed; use ARP row operations"
                );
                for (values, row) in values.iter().zip(rows) {
                    let values = array(values)?;
                    ensure!(
                        values.len() == columns.len(),
                        "table column count changed; use ARP column operations"
                    );
                    let mut present = vec![false; columns.len()];
                    for cell in array(&row["cells"]).context("table metadata cells missing")? {
                        let index = columns
                            .iter()
                            .position(|c| c["id"] == cell["column"])
                            .context("unknown table column")?;
                        ensure!(!present[index], "duplicate table slot");
                        present[index] = true;
                        ensure!(
                            cell.get("formula").is_none() || values[index].is_string(),
                            "formula must be a string"
                        );
                        visit(cell, &values[index])?;
                    }
                    ensure!(
                        values.iter().zip(present).all(|(v, p)| p || v.is_null()),
                        "unbound table slot must remain null; use ARP structural operations"
                    );
                }
            }
            "blank" | "image" | "chart" | "drawing" => {
                ensure!(value.is_null(), "non-text element must remain null")
            }
            _ => bail!("unknown element type"),
        }
    }
    Ok(())
}

/// Reconstruct the internal writer elements using only the recorded slots:
/// the metadata with every value of [`each`] in place.
pub fn join(body: &Value, metadata: &Value) -> Result<Value> {
    let mut values = vec![];
    each(body, metadata, |_, value| {
        values.push(value.clone());
        Ok(())
    })?;
    let mut values = values.into_iter();
    let mut result = metadata.clone();
    for e in result.as_array_mut().unwrap() {
        let mut put = |slot: &mut Value, key: &str| slot[key] = values.next().unwrap();
        match e["type"].as_str() {
            Some("text") => put(e, "text"),
            Some("formula") => put(e, "formula"),
            Some("table") => {
                for row in e["rows"].as_array_mut().unwrap() {
                    for cell in row["cells"].as_array_mut().unwrap() {
                        let key = if cell.get("formula").is_some() {
                            "formula"
                        } else {
                            "value"
                        };
                        put(cell, key);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_merged_table_preserves_slots_types_and_formula_identity() {
        let elements = json!([{"id":"t1","type":"table","columns":[{"id":"a"},{"id":"b"},{"id":"c"}],"rows":[
            {"id":"r1","cells":[{"id":"h","column":"a","role":"column_header","headers":[],"column_span":2,"value":"Merged header"},{"id":"literal","column":"c","role":"data","headers":[],"value":"=1+2"}]},
            {"id":"r2","cells":[{"id":"empty","column":"a","role":"data","headers":["h"],"value":null},{"id":"flag","column":"b","role":"data","headers":["h"],"value":false},{"id":"f","column":"c","role":"data","headers":[],"formula":"=1+2"}]}
        ]}]);
        let (mut body, metadata) = split(&elements).unwrap();
        assert_eq!(
            body,
            json!([{"table":[["Merged header",null,"=1+2"],[null,false,"=1+2"]]}])
        );
        assert!(
            !serde_json::to_string(&metadata)
                .unwrap()
                .contains("Merged header")
        );
        assert_eq!(join(&body, &metadata).unwrap(), elements);
        body[0]["table"][1][0] = json!(2.5);
        body[0]["table"][0][2] = json!("=3+4");
        let edited = join(&body, &metadata).unwrap();
        assert_eq!(edited[0]["rows"][1]["cells"][0]["value"], 2.5);
        assert_eq!(edited[0]["rows"][0]["cells"][1]["value"], "=3+4");
        assert!(edited[0]["rows"][0]["cells"][1].get("formula").is_none());
        body[0]["table"][0][1] = json!("cannot write merged padding");
        assert!(join(&body, &metadata).is_err());
        body[0]["table"][0][1] = Value::Null;
        body[0]["table"][1][2] = json!(12);
        assert!(join(&body, &metadata).is_err());
    }
}
