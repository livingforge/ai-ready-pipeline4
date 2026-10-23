//! A layout in memory. A large sheet has a table cell of element metadata
//! for each cell, and a cell held as a JSON object took several times its
//! members; the cells are typed, their common members in members of their
//! own, and the rest of each row, table, element, sheet and layout stays JSON.
//! Each reads and writes as the JSON it stands for.
use anyhow::{Context, Result, bail};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::ops::{Index, IndexMut};

static NULL: Value = Value::Null;

/// Defines a struct holding the members `$key` of a JSON object in members
/// of their own and its other members as JSON, read and written by member
/// name as the object is: a JSON object of a few members took the memory of
/// a dozen.
macro_rules! slotted {
    ($(#[$doc:meta])* $name:ident ($what:literal) { $($slot:ident: $key:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq)]
        pub(crate) struct $name {
            $($slot: Option<Value>,)+
            members: Map<String, Value>,
        }

        impl $name {
            fn slot(&self, key: &str) -> Option<&Option<Value>> {
                Some(match key {
                    $($key => &self.$slot,)+
                    _ => return None,
                })
            }

            fn slot_mut(&mut self, key: &str) -> Option<&mut Option<Value>> {
                Some(match key {
                    $($key => &mut self.$slot,)+
                    _ => return None,
                })
            }

            pub(crate) fn get(&self, key: &str) -> Option<&Value> {
                match self.slot(key) {
                    Some(slot) => slot.as_ref(),
                    None => self.members.get(key),
                }
            }

            pub(crate) fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
                let key = key.into();
                match self.slot_mut(&key) {
                    Some(slot) => slot.replace(value),
                    None => self.members.insert(key, value),
                }
            }

            pub(crate) fn remove(&mut self, key: &str) -> Option<Value> {
                match self.slot_mut(key) {
                    Some(slot) => slot.take(),
                    None => self.members.remove(key),
                }
            }

            // Not every slotted kind reads every accessor.
            #[allow(dead_code)]
            pub(crate) fn contains_key(&self, key: &str) -> bool {
                self.get(key).is_some()
            }

            /// The members, in the order of their names.
            pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
                let mut members: Vec<(&str, &Value)> = [$(($key, &self.$slot)),+]
                    .into_iter()
                    .filter_map(|(key, slot)| Some((key, slot.as_ref()?)))
                    .chain(self.members.iter().map(|(k, v)| (k.as_str(), v)))
                    .collect();
                members.sort_by(|a, b| a.0.cmp(b.0));
                members.into_iter()
            }

            #[allow(dead_code)]
            pub(crate) fn is_empty(&self) -> bool {
                $(self.$slot.is_none() &&)+ self.members.is_empty()
            }

            pub(crate) fn from_map(map: Map<String, Value>) -> Self {
                let mut item = Self::default();
                for (key, value) in map {
                    item.insert(key, value);
                }
                item
            }

            pub(crate) fn from_value(value: Value) -> Result<Self> {
                let Value::Object(map) = value else {
                    bail!(concat!($what, " must be an object"));
                };
                Ok(Self::from_map(map))
            }

            pub(crate) fn into_map(self) -> Map<String, Value> {
                let mut map = self.members;
                $(if let Some(value) = self.$slot {
                    map.insert($key.into(), value);
                })+
                map
            }

            pub(crate) fn to_value(&self) -> Value {
                Value::Object(self.clone().into_map())
            }
        }

        impl Index<&str> for $name {
            type Output = Value;
            fn index(&self, key: &str) -> &Value {
                self.get(key).unwrap_or(&NULL)
            }
        }

        impl IndexMut<&str> for $name {
            /// The member `key`, null when there was none, as a JSON object gives it.
            fn index_mut(&mut self, key: &str) -> &mut Value {
                if self.slot(key).is_some() {
                    return self.slot_mut(key).unwrap().get_or_insert(Value::Null);
                }
                self.members.entry(key).or_insert(Value::Null)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let members: Vec<_> = self.iter().collect();
                let mut map = serializer.serialize_map(Some(members.len()))?;
                for (key, value) in members {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    };
}

slotted! {
    /// A table cell of element metadata: its ID, logical column, font and,
    /// while a page is encoded, value in members of their own, and its other
    /// members (`label`, `category`, `mark`, `formula`, spans, roles and
    /// headers) as JSON.
    MetaCell ("table cell") { column: "column", font: "font", id: "id", value: "value" }
}

impl MetaCell {
    /// The cell with the ID `id` and no other member.
    pub(crate) fn with_id(id: impl Into<String>) -> Self {
        Self {
            id: Some(Value::String(id.into())),
            ..Self::default()
        }
    }
}

slotted! {
    /// A text or formula element of element metadata: its ID, type, font,
    /// visual and, while a page is encoded, text or formula in members of
    /// their own, and its other members as JSON. A page of a Word document
    /// has one for each paragraph.
    MetaText ("element") {
        font: "font",
        formula: "formula",
        id: "id",
        text: "text",
        kind: "type",
        visual: "visual",
    }
}

/// A record of a table of element metadata: its cells, and its other members
/// (`id`, `parent`) as JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MetaRow {
    pub(crate) fields: Map<String, Value>,
    pub(crate) cells: Vec<MetaCell>,
}

impl MetaRow {
    pub(crate) fn new(cells: Vec<MetaCell>) -> Self {
        Self {
            fields: Map::new(),
            cells,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        debug_assert!(key != "cells", "record cells are typed");
        self.fields.get(key)
    }

    pub(crate) fn from_value(value: Value) -> Result<Self> {
        let Value::Object(mut fields) = value else {
            bail!("table record must be an object");
        };
        let Some(Value::Array(cells)) = fields.remove("cells") else {
            bail!("table cells missing");
        };
        Ok(Self {
            fields,
            cells: cells
                .into_iter()
                .map(MetaCell::from_value)
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut row = self.fields.clone();
        row.insert(
            "cells".into(),
            Value::Array(self.cells.iter().map(MetaCell::to_value).collect()),
        );
        Value::Object(row)
    }
}

impl Index<&str> for MetaRow {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "cells", "record cells are typed");
        self.fields.get(key).unwrap_or(&NULL)
    }
}

impl IndexMut<&str> for MetaRow {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "cells", "record cells are typed");
        self.fields.entry(key).or_insert(Value::Null)
    }
}

impl Serialize for MetaRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_with(serializer, &self.fields, "cells", &self.cells)
    }
}

/// An element of the element metadata of a page: a table, with its records
/// typed, a text or formula element, typed, or another element as JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Meta {
    Table { fields: Value, rows: Vec<MetaRow> },
    Text(MetaText),
    Other(Value),
}

impl Meta {
    /// The table of `fields`, an object without records, and `rows`.
    pub(crate) fn table(fields: Value, rows: Vec<MetaRow>) -> Self {
        debug_assert!(fields.get("rows").is_none(), "table records are typed");
        Self::Table { fields, rows }
    }

    /// The members of a table besides its records, or of an element other
    /// than text and formulas, as JSON; none for a text or formula element.
    pub(crate) fn fields(&self) -> Option<&Value> {
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => Some(fields),
            Self::Text(_) => None,
        }
    }

    pub(crate) fn fields_mut(&mut self) -> Option<&mut Value> {
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => Some(fields),
            Self::Text(_) => None,
        }
    }

    pub(crate) fn is_table(&self) -> bool {
        matches!(self, Self::Table { .. })
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        debug_assert!(key != "rows" || !self.is_table(), "table records are typed");
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => fields.get(key),
            Self::Text(text) => text.get(key),
        }
    }

    /// Removes the member `key`, which a table's records are not.
    pub(crate) fn remove(&mut self, key: &str) -> Option<Value> {
        debug_assert!(key != "rows" || !self.is_table(), "table records are typed");
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => fields.as_object_mut()?.remove(key),
            Self::Text(text) => text.remove(key),
        }
    }

    /// The records of a table, none for another element.
    pub(crate) fn records(&self) -> Option<&[MetaRow]> {
        match self {
            Self::Table { rows, .. } => Some(rows),
            Self::Text(_) | Self::Other(_) => None,
        }
    }

    pub(crate) fn records_mut(&mut self) -> Option<&mut Vec<MetaRow>> {
        match self {
            Self::Table { rows, .. } => Some(rows),
            Self::Text(_) | Self::Other(_) => None,
        }
    }

    /// The records of a table.
    pub(crate) fn rows(&self) -> Result<&[MetaRow]> {
        self.records().context("table rows missing")
    }

    pub(crate) fn rows_mut(&mut self) -> Result<&mut Vec<MetaRow>> {
        self.records_mut().context("table rows missing")
    }

    /// The element given as JSON: a table with its records typed, and a text
    /// or formula element typed.
    pub(crate) fn from_value(mut value: Value) -> Result<Self> {
        match value["type"].as_str() {
            Some("table") => {}
            Some("text" | "formula") => return Ok(Self::Text(MetaText::from_value(value)?)),
            _ => return Ok(Self::Other(value)),
        }
        let fields = value.as_object_mut().context("element must be an object")?;
        let Some(Value::Array(rows)) = fields.remove("rows") else {
            bail!("table rows missing");
        };
        Ok(Self::Table {
            rows: rows
                .into_iter()
                .map(MetaRow::from_value)
                .collect::<Result<_>>()?,
            fields: value,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Table { fields, rows } => {
                let mut table = fields.clone();
                table["rows"] = Value::Array(rows.iter().map(MetaRow::to_value).collect());
                table
            }
            Self::Text(text) => text.to_value(),
            Self::Other(value) => value.clone(),
        }
    }

    /// [`Meta::to_value`] of an element moved out.
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Text(text) => Value::Object(text.into_map()),
            Self::Other(value) => value,
            table @ Self::Table { .. } => table.to_value(),
        }
    }
}

impl Index<&str> for Meta {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "rows" || !self.is_table(), "table records are typed");
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => &fields[key],
            Self::Text(text) => &text[key],
        }
    }
}

impl IndexMut<&str> for Meta {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "rows" || !self.is_table(), "table records are typed");
        match self {
            Self::Table { fields, .. } | Self::Other(fields) => &mut fields[key],
            Self::Text(text) => &mut text[key],
        }
    }
}

impl Serialize for Meta {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Table { fields, rows } => serialize_with(
                serializer,
                fields.as_object().unwrap_or(&Map::new()),
                "rows",
                rows,
            ),
            Self::Text(text) => text.serialize(serializer),
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

/// The element metadata of a page given as JSON.
pub(crate) fn metadata_from(value: Value) -> Result<Vec<Meta>> {
    let Value::Array(elements) = value else {
        bail!("element metadata missing");
    };
    elements.into_iter().map(Meta::from_value).collect()
}

/// The element metadata of a page as JSON.
pub(crate) fn metadata_value(metadata: &[Meta]) -> Value {
    Value::Array(metadata.iter().map(Meta::to_value).collect())
}

/// A sheet of a layout: its element metadata, and its other members
/// (`name`, `rows`, `columns`, `formulas`, `fonts`, `visuals` and such) as
/// JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutSheet {
    pub(crate) fields: Value,
    pub(crate) metadata: Vec<Meta>,
}

impl LayoutSheet {
    pub(crate) fn from_value(mut value: Value) -> Result<Self> {
        let fields = value
            .as_object_mut()
            .context("layout sheet must be an object")?;
        let metadata = metadata_from(
            fields
                .remove("element_metadata")
                .context("element metadata missing")?,
        )?;
        Ok(Self {
            fields: value,
            metadata,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut sheet = self.fields.clone();
        sheet["element_metadata"] = metadata_value(&self.metadata);
        sheet
    }
}

impl Index<&str> for LayoutSheet {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "element_metadata", "element metadata is typed");
        &self.fields[key]
    }
}

impl IndexMut<&str> for LayoutSheet {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "element_metadata", "element metadata is typed");
        &mut self.fields[key]
    }
}

/// A layout: its sheets by page ID, and its other members (`schema_version`,
/// `history`, `structure_sha256`) as JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) fields: Value,
    pub(crate) sheets: BTreeMap<String, LayoutSheet>,
}

impl Layout {
    /// The sheet of the page `page`.
    pub(crate) fn sheet(&self, page: &str) -> Result<&LayoutSheet> {
        self.sheets
            .get(page)
            .with_context(|| format!("layout page {page} missing"))
    }

    pub(crate) fn sheet_mut(&mut self, page: &str) -> Result<&mut LayoutSheet> {
        self.sheets
            .get_mut(page)
            .with_context(|| format!("layout page {page} missing"))
    }

    pub(crate) fn from_value(mut value: Value) -> Result<Self> {
        let fields = value.as_object_mut().context("layout must be an object")?;
        let Some(Value::Object(sheets)) = fields.remove("sheets") else {
            bail!("layout sheets missing");
        };
        Ok(Self {
            sheets: sheets
                .into_iter()
                .map(|(page, sheet)| Ok((page, LayoutSheet::from_value(sheet)?)))
                .collect::<Result<_>>()?,
            fields: value,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut layout = self.fields.clone();
        layout["sheets"] = Value::Object(
            self.sheets
                .iter()
                .map(|(page, sheet)| (page.clone(), sheet.to_value()))
                .collect(),
        );
        layout
    }
}

impl Index<&str> for Layout {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "sheets", "layout sheets are typed");
        &self.fields[key]
    }
}

impl IndexMut<&str> for Layout {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "sheets", "layout sheets are typed");
        &mut self.fields[key]
    }
}

/// `fields` with the member `key` holding `items`, serialized with its
/// members in order, as JSON objects are.
fn serialize_with<S: Serializer, T: Serialize>(
    serializer: S,
    fields: &Map<String, Value>,
    key: &str,
    items: &[T],
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(fields.len() + 1))?;
    let mut placed = false;
    for (name, value) in fields {
        if !placed && name.as_str() > key {
            map.serialize_entry(key, items)?;
            placed = true;
        }
        map.serialize_entry(name, value)?;
    }
    if !placed {
        map.serialize_entry(key, items)?;
    }
    map.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_layout_reads_and_writes_as_its_json() {
        let value = json!({"schema_version":"14","history":[],"sheets":{"sheet-1":{
            "name":"S","rows":{"r1":{"key":"r1","position":1}},"fonts":[{"latin":"A"}],
            "element_metadata":[
                {"id":"cell_r1_A","type":"text","font":0},
                {"id":"table_cell%5Fr2%5FA","type":"table","headings":1,"rows":[
                    {"id":"record_r2","cells":[
                        {"id":"cell_r2_A","column":"column_A","font":0},
                        {"id":"cell_r2_B","column":"column_B","label":"cell_r1_B","value":null}]},
                    {"cells":[{"id":"cell_r3_A","formula":null,"column":"column_A","mark":true}],
                     "parent":"record_r2"}]},
                {"type":"blank","id":"gap_x"}]}}});
        let layout = Layout::from_value(value.clone()).unwrap();
        assert_eq!(layout.to_value(), value);
        let sheet = &layout.sheets["sheet-1"];
        let rows = sheet.metadata[1].rows().unwrap();
        assert_eq!(rows[0].cells[1]["label"], "cell_r1_B");
        assert_eq!(rows[0].cells[1].get("value"), Some(&Value::Null));
        assert_eq!(rows[0].cells[0].get("value"), None);
        assert_eq!(rows[1]["parent"], "record_r2");
        assert_eq!(
            serde_json::to_value(&sheet.metadata).unwrap(),
            value["sheets"]["sheet-1"]["element_metadata"]
        );
        let mut cell = rows[0].cells[0].clone();
        cell["category"] = json!("k");
        cell["font"] = json!(1);
        assert_eq!(cell.remove("column"), Some(json!("column_A")));
        assert_eq!(
            cell.to_value(),
            json!({"id":"cell_r2_A","font":1,"category":"k"})
        );
    }
}
