//! Array bodies and their value-free layout metadata. Array positions are editing
//! slots, not identities: moving values by hand is an ordinary value edit.
use crate::data::{array, string};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

/// Separate editable values from identities, topology and source references.
pub fn split(elements: &Value) -> Result<(Value, Value)> {
    let mut metadata = elements.clone();
    let mut body = Vec::new();
    for e in metadata.as_array_mut().context("elements missing")? {
        let kind = string(&e["type"])?.to_owned();
        let value = match kind.as_str() {
            "text" | "formula" => e
                .as_object_mut()
                .unwrap()
                .insert(kind.clone(), Value::Null)
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
                        let key = if cell.get("formula").is_some() {
                            "formula"
                        } else {
                            "value"
                        };
                        values[index] = cell
                            .as_object_mut()
                            .unwrap()
                            .insert(key.into(), Value::Null)
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

/// Reconstruct the internal writer elements using only the recorded slots.
/// Missing table cells are null padding and cannot be made writable by hand.
pub fn join(body: &Value, metadata: &Value) -> Result<Value> {
    let body = array(body)?;
    let mut result = metadata.clone();
    let elements = result.as_array_mut().context("element metadata missing")?;
    ensure!(
        body.len() == elements.len(),
        "element count changed; use ARP structural operations"
    );
    for (value, e) in body.iter().zip(elements) {
        let kind = string(&e["type"])?.to_owned();
        let fields = value
            .as_object()
            .context("body element must be an object")?;
        ensure!(
            fields.len() == 1 && fields.contains_key(&kind),
            "element kind changed; use ARP structural operations"
        );
        let value = &value[&kind];
        match kind.as_str() {
            "text" => e["text"] = value.clone(),
            "formula" => {
                ensure!(value.is_string(), "formula must be a string");
                e["formula"] = value.clone();
            }
            "table" => {
                let columns = array(&e["columns"])?.clone();
                let rows = e["rows"]
                    .as_array_mut()
                    .context("table metadata rows missing")?;
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
                    for cell in row["cells"]
                        .as_array_mut()
                        .context("table metadata cells missing")?
                    {
                        let index = columns
                            .iter()
                            .position(|c| c["id"] == cell["column"])
                            .context("unknown table column")?;
                        ensure!(!present[index], "duplicate table slot");
                        present[index] = true;
                        let key = if cell.get("formula").is_some() {
                            "formula"
                        } else {
                            "value"
                        };
                        ensure!(
                            key != "formula" || values[index].is_string(),
                            "formula must be a string"
                        );
                        cell[key] = values[index].clone();
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
