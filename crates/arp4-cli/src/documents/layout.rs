//! A layout in memory. A large sheet has a table cell of element metadata
//! for each cell, and a cell held as a JSON object took several times its
//! members; the cells are typed, their common members in members of their
//! own, and the rest of each row, table, element, sheet and layout stays JSON.
//! Each reads and writes as the JSON it stands for.
use super::elements::Name;
use anyhow::{Context, Result, bail};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::{Map, Value, json};
use std::borrow::Cow;
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

        // Not every slotted kind reads every accessor.
        #[allow(dead_code)]
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

/// A table cell of element metadata: its ID and logical column as texts, its
/// font by its index, as nearly every cell's is, and, while a page is
/// encoded, its value in members of their own, and its other members
/// (`label`, `category`, `mark`, `formula`, spans, roles, headers and a font
/// given otherwise), which few cells have, as JSON. A large sheet has one for
/// each cell: its ID is held in it, and its column shared one pointer wide.
/// The ID, the column and the font are read by their own methods; every
/// member is written by name, and the others read by name, as the object is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MetaCell {
    id: Option<CellId>,
    /// Shared by the cells of a logical column.
    column: Option<Name>,
    /// The index of its font in its sheet's fonts.
    font: Option<u32>,
    value: Option<Box<Value>>,
    members: Option<Box<Map<String, Value>>>,
}

/// The ID of a [`MetaCell`]: held in the cell when it is as short as nearly
/// every one is, rather than in room of its own.
#[derive(Clone, PartialEq)]
enum CellId {
    Short { len: u8, bytes: [u8; SHORT_ID] },
    Long(Box<str>),
}

/// The longest ID a [`MetaCell`] holds in itself.
const SHORT_ID: usize = 22;

impl CellId {
    fn new(id: &str) -> Self {
        if id.len() <= SHORT_ID {
            let mut bytes = [0; SHORT_ID];
            bytes[..id.len()].copy_from_slice(id.as_bytes());
            Self::Short {
                len: id.len() as u8,
                bytes,
            }
        } else {
            Self::Long(id.into())
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Short { len, bytes } => std::str::from_utf8(&bytes[..usize::from(*len)])
                .expect("a short ID is the text it was made from"),
            Self::Long(id) => id,
        }
    }
}

impl std::fmt::Debug for CellId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

/// The members of a [`MetaCell`] its own methods read.
const CELL_TYPED: [&str; 3] = ["column", "font", "id"];

impl MetaCell {
    /// The cell with the ID `id` and no other member.
    pub(crate) fn with_id(id: impl AsRef<str>) -> Self {
        Self {
            id: Some(CellId::new(id.as_ref())),
            ..Self::default()
        }
    }

    pub(crate) fn id(&self) -> Result<&str> {
        self.id
            .as_ref()
            .map(CellId::as_str)
            .context("table cell ID missing")
    }

    pub(crate) fn set_id(&mut self, id: impl AsRef<str>) {
        self.id = Some(CellId::new(id.as_ref()));
    }

    /// The logical column.
    pub(crate) fn column(&self) -> Result<&str> {
        self.column.as_deref().context("table cell column missing")
    }

    pub(crate) fn set_column(&mut self, column: Name) {
        self.column = Some(column);
    }

    /// The font, as JSON.
    pub(crate) fn font(&self) -> Option<Cow<'_, Value>> {
        match self.font {
            Some(index) => Some(Cow::Owned(json!(index))),
            None => self.members.as_ref()?.get("font").map(Cow::Borrowed),
        }
    }

    /// The member `key`, which is neither the ID, the column nor the font:
    /// those are read by [`MetaCell::id`], [`MetaCell::column`] and
    /// [`MetaCell::font`].
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        debug_assert!(!CELL_TYPED.contains(&key), "table cell {key} is typed");
        match key {
            "value" => self.value.as_deref(),
            _ => self.members.as_ref()?.get(key),
        }
    }

    /// Sets the member `key`; the ID and the column take a text.
    pub(crate) fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        if let Some(text) = self.text_mut(&key) {
            let Value::String(new) = value else {
                debug_assert!(false, "table cell {key} must be a text");
                return None;
            };
            return text.replace(new).map(Value::String);
        }
        match key.as_str() {
            "font" => {
                let old = self.remove("font");
                match value.as_u64().and_then(|index| u32::try_from(index).ok()) {
                    Some(index) => self.font = Some(index),
                    None => {
                        self.members.get_or_insert_default().insert(key, value);
                    }
                }
                old
            }
            "value" => self.value.replace(Box::new(value)).map(|old| *old),
            _ => self.members.get_or_insert_default().insert(key, value),
        }
    }

    pub(crate) fn remove(&mut self, key: &str) -> Option<Value> {
        if let Some(text) = self.text_mut(key) {
            return text.take().map(Value::String);
        }
        match key {
            "font" if self.font.is_some() => self.font.take().map(|index| json!(index)),
            "value" => self.value.take().map(|old| *old),
            _ => {
                let members = self.members.as_mut()?;
                let removed = members.remove(key);
                if members.is_empty() {
                    self.members = None;
                }
                removed
            }
        }
    }

    /// The ID or the column, as an owned text, for [`MetaCell::insert`] and
    /// [`MetaCell::remove`].
    fn text_mut(&mut self, key: &str) -> Option<TextSlot<'_>> {
        match key {
            "id" => Some(TextSlot::Id(&mut self.id)),
            "column" => Some(TextSlot::Column(&mut self.column)),
            _ => None,
        }
    }

    pub(crate) fn contains_key(&self, key: &str) -> bool {
        match key {
            "font" => self.font().is_some(),
            _ => self.get(key).is_some(),
        }
    }

    /// Whether the cell has no member, its ID and column too.
    pub(crate) fn is_empty(&self) -> bool {
        self.id.is_none()
            && self.column.is_none()
            && self.font.is_none()
            && self.value.is_none()
            && self.members.is_none()
    }

    /// The members, in the order of their names.
    fn entries(&self) -> Vec<(&str, Member<'_>)> {
        let texts = [
            ("column", self.column.as_deref()),
            ("id", self.id.as_ref().map(CellId::as_str)),
        ];
        let font = self.font.map(|index| ("font", Member::Index(index)));
        let value = self
            .value
            .as_deref()
            .map(|value| ("value", Member::Json(value)));
        let mut members: Vec<(&str, Member<'_>)> = texts
            .into_iter()
            .filter_map(|(key, text)| Some((key, Member::Text(text?))))
            .chain(font)
            .chain(value)
            .chain(
                self.members
                    .iter()
                    .flat_map(|members| members.iter())
                    .map(|(key, value)| (key.as_str(), Member::Json(value))),
            )
            .collect();
        members.sort_by(|a, b| a.0.cmp(b.0));
        members
    }

    pub(crate) fn from_map(map: Map<String, Value>) -> Result<Self> {
        let mut cell = Self::default();
        for (key, value) in map {
            match key.as_str() {
                "id" => cell.id = Some(CellId::new(&text_of(value, "table cell ID")?)),
                "column" => cell.column = Some(text_of(value, "table cell column")?.into()),
                _ => {
                    cell.insert(key, value);
                }
            }
        }
        Ok(cell)
    }

    pub(crate) fn from_value(value: Value) -> Result<Self> {
        let Value::Object(map) = value else {
            bail!("table cell must be an object");
        };
        Self::from_map(map)
    }

    pub(crate) fn into_map(self) -> Map<String, Value> {
        let mut map = self.members.map(|members| *members).unwrap_or_default();
        if let Some(id) = self.id {
            map.insert("id".into(), json!(id.as_str()));
        }
        if let Some(column) = self.column {
            map.insert("column".into(), json!(&*column));
        }
        if let Some(font) = self.font {
            map.insert("font".into(), json!(font));
        }
        if let Some(value) = self.value {
            map.insert("value".into(), *value);
        }
        map
    }

    pub(crate) fn to_value(&self) -> Value {
        Value::Object(self.clone().into_map())
    }
}

/// The ID or the column of a [`MetaCell`], set and taken as a text.
enum TextSlot<'a> {
    Id(&'a mut Option<CellId>),
    Column(&'a mut Option<Name>),
}

impl TextSlot<'_> {
    fn replace(self, text: String) -> Option<String> {
        match self {
            Self::Id(id) => id
                .replace(CellId::new(&text))
                .map(|old| old.as_str().to_owned()),
            Self::Column(column) => column.replace(text.into()).map(|old| old.to_string()),
        }
    }

    fn take(self) -> Option<String> {
        match self {
            Self::Id(id) => id.take().map(|old| old.as_str().to_owned()),
            Self::Column(column) => column.take().map(|old| old.to_string()),
        }
    }
}

/// A member of a [`MetaCell`] as it is written: a text of its own or JSON.
#[derive(serde::Serialize)]
#[serde(untagged)]
enum Member<'a> {
    Text(&'a str),
    Index(u32),
    Json(&'a Value),
}

/// The text `value` holds, which a `what` must be.
fn text_of(value: Value, what: &str) -> Result<String> {
    match value {
        Value::String(text) => Ok(text),
        _ => bail!("{what} must be a string"),
    }
}

impl Index<&str> for MetaCell {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl IndexMut<&str> for MetaCell {
    /// The member `key`, null when there was none, as a JSON object gives it.
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(!CELL_TYPED.contains(&key), "table cell {key} is typed");
        if key == "value" {
            return self.value.get_or_insert_with(|| Box::new(Value::Null));
        }
        self.members
            .get_or_insert_default()
            .entry(key)
            .or_insert(Value::Null)
    }
}

impl Serialize for MetaCell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let members = self.entries();
        let mut map = serializer.serialize_map(Some(members.len()))?;
        for (key, value) in members {
            map.serialize_entry(key, &value)?;
        }
        map.end()
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

slotted! {
    /// The members of a record of a table besides its cells: its ID and
    /// parent in members of their own, and any other as JSON. A large table
    /// has a record for each row, and a JSON object of them took several
    /// times their size.
    MetaRowFields ("table record") { id: "id", parent: "parent" }
}

/// A record of a table of element metadata: its cells, and its other members
/// (`id`, `parent`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MetaRow {
    pub(crate) fields: MetaRowFields,
    pub(crate) cells: Vec<MetaCell>,
}

impl MetaRow {
    /// The record of `cells`, held in as much room as they take
    /// ([`crate::data::exact`]): a record's cells are added one at a time,
    /// and a list grown by doubling held up to twice their room.
    pub(crate) fn new(cells: Vec<MetaCell>) -> Self {
        Self {
            fields: MetaRowFields::default(),
            cells: crate::data::exact(cells),
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
            fields: MetaRowFields::from_map(fields),
            cells: cells
                .into_iter()
                .map(MetaCell::from_value)
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut row = self.fields.clone().into_map();
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
        &self.fields[key]
    }
}

impl IndexMut<&str> for MetaRow {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "cells", "record cells are typed");
        &mut self.fields[key]
    }
}

impl Serialize for MetaRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_with(serializer, self.fields.iter(), "cells", &self.cells)
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
                fields
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, value)| (key.as_str(), value)),
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

/// A row or column of a layout sheet: the key it has in the extraction, as
/// `r12` or `C`, or the operation key of an insertion not yet applied, and its
/// position on the sheet as the pending operations leave it, once placed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Line {
    pub(crate) key: String,
    pub(crate) position: Option<u64>,
}

/// The rows or the columns of a layout sheet by identity, in the order of
/// their identities. Held typed: a sheet has one for each row, and a JSON
/// object of each took several times its key.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Lines(BTreeMap<String, Line>);

impl Lines {
    pub(crate) fn get(&self, id: &str) -> Option<&Line> {
        self.0.get(id)
    }

    /// The position of the row or column `id`, none for one not placed.
    pub(crate) fn position(&self, id: &str) -> Option<u64> {
        self.0.get(id)?.position
    }

    /// The key of the row or column `id`.
    pub(crate) fn key(&self, id: &str) -> Option<&str> {
        self.0.get(id).map(|entry| entry.key.as_str())
    }

    pub(crate) fn contains_key(&self, id: &str) -> bool {
        self.0.contains_key(id)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &Line)> {
        self.0.iter()
    }

    pub(crate) fn values_mut(&mut self) -> impl Iterator<Item = &mut Line> {
        self.0.values_mut()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }

    pub(crate) fn insert(&mut self, id: String, entry: Line) -> Option<Line> {
        self.0.insert(id, entry)
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<Line> {
        self.0.remove(id)
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&String, &mut Line) -> bool) {
        self.0.retain(keep)
    }

    /// The axis given as JSON, an object of `{key, position}` by identity.
    pub(crate) fn from_value(value: &Value) -> Result<Self> {
        let entries = value.as_object().context("layout axis missing")?;
        let mut axis = Self::default();
        for (id, entry) in entries {
            let key = entry["key"]
                .as_str()
                .with_context(|| format!("layout key of {id} missing"))?;
            let position = match &entry["position"] {
                Value::Null => None,
                position => Some(position.as_u64().context("layout position missing")?),
            };
            axis.insert(
                id.clone(),
                Line {
                    key: key.to_owned(),
                    position,
                },
            );
        }
        Ok(axis)
    }

    pub(crate) fn to_value(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(id, entry)| {
                    let mut value = serde_json::json!({"key": entry.key});
                    if let Some(position) = entry.position {
                        value["position"] = serde_json::json!(position);
                    }
                    (id.clone(), value)
                })
                .collect(),
        )
    }
}

/// A sheet of a layout: its element metadata, its rows and columns, and its
/// other members (`name`, `formulas`, `fonts`, `visuals` and such) as JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutSheet {
    pub(crate) fields: Value,
    pub(crate) metadata: Vec<Meta>,
    pub(crate) rows: Lines,
    pub(crate) columns: Lines,
    /// Not stored: the diagrams the page's drawings make, by the IDs of their
    /// drawings and texts, which the extraction gives as the layout is placed.
    pub(crate) diagrams: std::sync::Arc<Vec<crate::document_body::Diagram<String>>>,
}

impl LayoutSheet {
    /// The sheet of `fields` and no metadata, rows or columns.
    pub(crate) fn new(fields: Value) -> Self {
        Self {
            fields,
            metadata: vec![],
            rows: Lines::default(),
            columns: Lines::default(),
            diagrams: Default::default(),
        }
    }

    /// The rows (`axis` "rows") or the columns.
    pub(crate) fn axis(&self, axis: &str) -> &Lines {
        match axis {
            "rows" => &self.rows,
            _ => &self.columns,
        }
    }

    pub(crate) fn axis_mut(&mut self, axis: &str) -> &mut Lines {
        match axis {
            "rows" => &mut self.rows,
            _ => &mut self.columns,
        }
    }

    pub(crate) fn from_value(mut value: Value) -> Result<Self> {
        let fields = value
            .as_object_mut()
            .context("layout sheet must be an object")?;
        let metadata = metadata_from(
            fields
                .remove("element_metadata")
                .context("element metadata missing")?,
        )?;
        let mut axis = |name: &str| -> Result<Lines> {
            Lines::from_value(&fields.remove(name).context("layout axis missing")?)
        };
        let (rows, columns) = (axis("rows")?, axis("columns")?);
        Ok(Self {
            fields: value,
            metadata,
            rows,
            columns,
            diagrams: Default::default(),
        })
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut sheet = self.fields.clone();
        sheet["element_metadata"] = metadata_value(&self.metadata);
        sheet["rows"] = self.rows.to_value();
        sheet["columns"] = self.columns.to_value();
        sheet
    }
}

impl Index<&str> for LayoutSheet {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(
            !matches!(key, "element_metadata" | "rows" | "columns"),
            "layout sheet {key} are typed"
        );
        &self.fields[key]
    }
}

impl IndexMut<&str> for LayoutSheet {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(
            !matches!(key, "element_metadata" | "rows" | "columns"),
            "layout sheet {key} are typed"
        );
        &mut self.fields[key]
    }
}

/// A layout: its sheets by page ID, and its other members (`schema_version`,
/// `history`, `structure_sha256`) as JSON.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) fields: Value,
    pub(crate) sheets: BTreeMap<String, LayoutSheet>,
    /// Not stored: the pending row and column operations its rows and
    /// columns are placed under, which tell where a cell shifted within a
    /// band is shown.
    pub(crate) operations: std::sync::Arc<Vec<crate::excel::StructuralOperation>>,
    /// Not stored: the pending placements of drawings, which with the
    /// operations tell which drawings a deletion takes.
    pub(crate) placements: std::sync::Arc<Vec<crate::excel::PlaceOperation>>,
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
            operations: Default::default(),
            placements: Default::default(),
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
fn serialize_with<'a, S: Serializer, T: Serialize>(
    serializer: S,
    fields: impl Iterator<Item = (&'a str, &'a Value)>,
    key: &str,
    items: &[T],
) -> Result<S::Ok, S::Error> {
    let fields: Vec<_> = fields.collect();
    let mut map = serializer.serialize_map(Some(fields.len() + 1))?;
    let mut placed = false;
    for (name, value) in fields {
        if !placed && name > key {
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

    /// A large sheet has a table cell for each of its cells: one holds no
    /// more than its ID, column and font index, and room for the rest.
    #[test]
    fn a_table_cell_is_small() {
        assert!(std::mem::size_of::<MetaCell>() <= 56);
    }

    /// A cell holds a short ID in itself and a long one apart, and a font
    /// given as JSON among its other members, and reads them back as given.
    #[test]
    fn a_table_cell_keeps_its_id_and_font_however_held() {
        let long = "cell_".to_owned() + &"r".repeat(40) + "_A";
        for id in ["cell_r2_A", long.as_str(), ""] {
            let mut cell = MetaCell::with_id(id);
            assert_eq!(cell.id().unwrap(), id);
            assert_eq!(cell.insert("id", json!("cell_r3_B")), Some(json!(id)));
            assert_eq!(cell.id().unwrap(), "cell_r3_B");
        }
        let mut cell = MetaCell::with_id("cell_r2_A");
        assert_eq!(cell.insert("font", json!({"latin":"A"})), None);
        assert_eq!(cell.font().as_deref(), Some(&json!({"latin":"A"})));
        assert!(cell.contains_key("font"));
        assert_eq!(cell.insert("font", json!(2)), Some(json!({"latin":"A"})));
        assert_eq!(cell.to_value(), json!({"id":"cell_r2_A","font":2}));
        assert_eq!(cell.remove("font"), Some(json!(2)));
        assert!(!cell.contains_key("font"));
        let given = json!({"id":"cell_r2_A","font":{"latin":"A"},"label":"x"});
        let cell = MetaCell::from_value(given.clone()).unwrap();
        assert_eq!(cell.to_value(), given);
        assert_eq!(serde_json::to_value(&cell).unwrap(), given);
    }

    #[test]
    fn a_layout_reads_and_writes_as_its_json() {
        let value = json!({"schema_version":"14","history":[],"sheets":{"sheet-1":{
            "name":"S","rows":{"r1":{"key":"r1","position":1}},"columns":{},"fonts":[{"latin":"A"}],
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
        cell.insert("font", json!(1));
        assert_eq!(cell.remove("column"), Some(json!("column_A")));
        assert_eq!(
            cell.to_value(),
            json!({"id":"cell_r2_A","font":1,"category":"k"})
        );
    }
}
