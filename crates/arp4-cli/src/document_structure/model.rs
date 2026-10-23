//! The structure interpretation in memory. A large sheet gives a table cell
//! per extraction cell, and a cell held as JSON took several times its text;
//! its cells are typed and the rest of each element and of the interpretation
//! stays JSON. It reads and writes as the JSON the contract gives it.
use crate::data::Lazy;
use anyhow::{Context, Result, bail, ensure};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Map, Value};
use std::fmt;
use std::ops::{Index, IndexMut};

/// The role of a cell in its element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Unassigned,
    Text,
    Data,
    RowHeader,
    ColumnHeader,
    Note,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::Text => "text",
            Self::Data => "data",
            Self::RowHeader => "row_header",
            Self::ColumnHeader => "column_header",
            Self::Note => "note",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "unassigned" => Self::Unassigned,
            "text" => Self::Text,
            "data" => Self::Data,
            "row_header" => Self::RowHeader,
            "column_header" => Self::ColumnHeader,
            "note" => Self::Note,
            _ => bail!("unknown cell role {name}"),
        })
    }

    /// Whether the cell heads others: a row or column header.
    pub fn heads(self) -> bool {
        matches!(self, Self::RowHeader | Self::ColumnHeader)
    }
}

/// Whether a cell's text was read, and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextState {
    Read,
    Empty,
    Unreadable,
    NotExamined,
}

impl TextState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Empty => "empty",
            Self::Unreadable => "unreadable",
            Self::NotExamined => "not_examined",
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "read" => Self::Read,
            "empty" => Self::Empty,
            "unreadable" => Self::Unreadable,
            "not_examined" => Self::NotExamined,
            _ => bail!("unknown text state {name}"),
        })
    }
}

/// A cell of an element: its ID, address, role, the IDs of its headers and
/// whether its text was read. Its members are those of the contract's `cell`,
/// in the order the canonical encoding writes them.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementCell {
    pub address: Box<str>,
    pub headers: Vec<Box<str>>,
    pub id: Box<str>,
    pub role: Role,
    pub text_state: TextState,
}

impl Serialize for ElementCell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("address", &*self.address)?;
        map.serialize_entry("headers", &self.headers)?;
        map.serialize_entry("id", &*self.id)?;
        map.serialize_entry("role", self.role.as_str())?;
        map.serialize_entry("text_state", self.text_state.as_str())?;
        map.end()
    }
}

impl ElementCell {
    pub fn from_value(cell: Value) -> Result<Self> {
        serde_json::from_value(cell).context("invalid element cell")
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("an element cell is JSON")
    }

    /// The cell as the contract checks it beyond its members' types: its ID,
    /// address and headers are texts, its headers once each.
    fn check(&self) -> Result<()> {
        let text = |name: &str, value: &str| {
            ensure!(
                !value.is_empty()
                    && !value.starts_with(char::is_whitespace)
                    && !value.ends_with(char::is_whitespace),
                "invalid document structure: cell {name} {value:?} is not a text"
            );
            Ok(())
        };
        text("id", &self.id)?;
        text("address", &self.address)?;
        for (index, header) in self.headers.iter().enumerate() {
            text("header", header)?;
            ensure!(
                !self.headers[..index].contains(header),
                "invalid document structure: cell {} names header {header} twice",
                self.id
            );
        }
        Ok(())
    }
}

/// An element of the interpretation: its cells, and its other members
/// (`fields`), such as its ID, kind, sheet, reading and parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub fields: Value,
    pub cells: Vec<ElementCell>,
}

impl Index<&str> for Element {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "cells", "element cells are typed");
        &self.fields[key]
    }
}

impl IndexMut<&str> for Element {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "cells", "element cells are typed");
        &mut self.fields[key]
    }
}

impl Element {
    /// The element of `fields`, an object without cells, and `cells`.
    pub fn new(fields: Value, cells: Vec<ElementCell>) -> Self {
        debug_assert!(fields.get("cells").is_none(), "element cells are typed");
        Self { fields, cells }
    }

    pub fn from_value(mut element: Value) -> Result<Self> {
        let fields = element
            .as_object_mut()
            .context("element must be an object")?;
        let Some(Value::Array(cells)) = fields.remove("cells") else {
            bail!("element cells must be an array");
        };
        Ok(Self {
            cells: cells
                .into_iter()
                .map(ElementCell::from_value)
                .collect::<Result<_>>()?,
            fields: element,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut element = self.fields.clone();
        element["cells"] = Value::Array(self.cells.iter().map(ElementCell::to_value).collect());
        element
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        debug_assert!(key != "cells", "element cells are typed");
        self.fields.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        debug_assert!(key != "cells", "element cells are typed");
        self.fields.get_mut(key)
    }

    /// The ID of the element.
    pub fn id(&self) -> Result<&str> {
        crate::data::string(&self.fields["id"])
    }

    /// The name of the sheet of the element.
    pub fn sheet(&self) -> Result<&str> {
        crate::data::string(&self.fields["sheet"])
    }

    /// The element as [`crate::data::hash_encoded_lazy`] encodes it, a cell at
    /// a time.
    pub fn lazy(&self) -> Lazy<'_> {
        let mut members = Lazy::members(&self.fields, &[]);
        members.push((
            "cells",
            Lazy::Items(Box::new(
                self.cells
                    .iter()
                    .map(|cell| Ok(Lazy::Value(cell.to_value()))),
            )),
        ));
        Lazy::Object(members)
    }
}

impl Serialize for Element {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_with(serializer, &self.fields, "cells", &self.cells)
    }
}

impl<'de> Deserialize<'de> for Element {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (fields, cells) = deserializer.deserialize_map(Members::<ElementCell>::new("cells"))?;
        Ok(Self { fields, cells })
    }
}

/// A structure interpretation: its elements, and its other members
/// (`fields`), such as its document, original, regions, visuals and review.
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    pub fields: Value,
    pub elements: Vec<Element>,
}

impl Index<&str> for Structure {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        debug_assert!(key != "elements", "structure elements are typed");
        &self.fields[key]
    }
}

impl IndexMut<&str> for Structure {
    fn index_mut(&mut self, key: &str) -> &mut Value {
        debug_assert!(key != "elements", "structure elements are typed");
        &mut self.fields[key]
    }
}

impl Structure {
    /// The structure of `fields`, an object without elements, and `elements`.
    pub fn new(fields: Value, elements: Vec<Element>) -> Self {
        debug_assert!(
            fields.get("elements").is_none(),
            "structure elements are typed"
        );
        Self { fields, elements }
    }

    /// The structure `value` gives, which the caller has checked against the
    /// contract.
    pub fn from_value(mut value: Value) -> Result<Self> {
        let fields = value
            .as_object_mut()
            .context("structure must be an object")?;
        let Some(Value::Array(elements)) = fields.remove("elements") else {
            bail!("structure elements must be an array");
        };
        Ok(Self {
            elements: elements
                .into_iter()
                .map(Element::from_value)
                .collect::<Result<_>>()?,
            fields: value,
        })
    }

    pub fn to_value(&self) -> Value {
        let mut value = self.fields.clone();
        value["elements"] = Value::Array(self.elements.iter().map(Element::to_value).collect());
        value
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        debug_assert!(key != "elements", "structure elements are typed");
        self.fields.get(key)
    }

    /// The structure as [`crate::data::hash_encoded_lazy`] encodes it, without
    /// its member `skip`.
    pub fn lazy(&self, skip: Option<&str>) -> Lazy<'_> {
        let mut members = Lazy::members(&self.fields, skip.as_slice());
        members.push((
            "elements",
            Lazy::Items(Box::new(self.elements.iter().map(|e| Ok(e.lazy())))),
        ));
        Lazy::Object(members)
    }

    /// The contract checks of `self` that typed cells do not already hold:
    /// the structure with the first cell of each element against the schema,
    /// and the texts of every cell.
    pub(super) fn check(&self, schema: impl Fn(&Value) -> Result<()>) -> Result<()> {
        let mut skeleton = self.fields.clone();
        skeleton["elements"] = Value::Array(
            self.elements
                .iter()
                .map(|element| {
                    let mut element_value = element.fields.clone();
                    element_value["cells"] = Value::Array(
                        element
                            .cells
                            .first()
                            .map(ElementCell::to_value)
                            .into_iter()
                            .collect(),
                    );
                    element_value
                })
                .collect(),
        );
        schema(&skeleton)?;
        for element in &self.elements {
            for cell in &element.cells {
                cell.check()?;
            }
        }
        Ok(())
    }
}

impl Serialize for Structure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serialize_with(serializer, &self.fields, "elements", &self.elements)
    }
}

impl<'de> Deserialize<'de> for Structure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (fields, elements) =
            deserializer.deserialize_map(Members::<Element>::new("elements"))?;
        Ok(Self { fields, elements })
    }
}

/// `fields` with the member `key` holding `items`, serialized with its
/// members in order, as JSON objects are.
fn serialize_with<S: Serializer, T: Serialize>(
    serializer: S,
    fields: &Value,
    key: &str,
    items: &[T],
) -> Result<S::Ok, S::Error> {
    let empty = Map::new();
    let fields = fields.as_object().unwrap_or(&empty);
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

/// Reads an object whose member `key` is a list of `T`, and the rest as JSON.
struct Members<T> {
    key: &'static str,
    marker: std::marker::PhantomData<T>,
}

impl<T> Members<T> {
    fn new(key: &'static str) -> Self {
        Self {
            key,
            marker: std::marker::PhantomData,
        }
    }
}

impl<'de, T: Deserialize<'de>> de::Visitor<'de> for Members<T> {
    type Value = (Value, Vec<T>);

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "an object with {}", self.key)
    }

    fn visit_map<A: de::MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
        let mut fields = Map::new();
        let mut items = None;
        while let Some(name) = access.next_key::<String>()? {
            if name == self.key {
                if items.is_some() {
                    return Err(de::Error::duplicate_field(self.key));
                }
                items = Some(access.next_value::<Vec<T>>()?);
            } else if fields.insert(name.clone(), access.next_value()?).is_some() {
                return Err(de::Error::custom(format!("duplicate member {name}")));
            }
        }
        let items = items.ok_or_else(|| de::Error::missing_field(self.key))?;
        Ok((Value::Object(fields), items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{encoded, hash, hash_encoded_lazy, hash_encoded_without};
    use serde_json::json;

    fn sample() -> Value {
        json!({"schema_version":1,"document":"d.xlsx","source":{"path":"d.xlsx","sha256":"a"},
            "extraction_hash":"b","regions":[],"visuals":[{"id":"v"}],"review":{"status":"pending"},
            "elements":[
                {"id":"t","kind":"table","sheet":"S","evidence":[],"reading":{"method":"parser"},
                 "parent":{"element":"p","context":["s1-A1"]},
                 "cells":[
                    {"id":"s1-A1","address":"A1","role":"column_header","headers":[],"text_state":"read"},
                    {"id":"s1-A2","address":"A2","role":"data","headers":["s1-A1"],"text_state":"empty"}]},
                {"id":"x","kind":"text","sheet":"S","evidence":[],"reading":{},
                 "cells":[{"id":"s1-C9","address":"C9","role":"unassigned","headers":[],"text_state":"not_examined"}]}]})
    }

    #[test]
    fn a_structure_reads_and_writes_as_its_json() {
        let value = sample();
        let structure = Structure::from_value(value.clone()).unwrap();
        assert_eq!(structure.to_value(), value);
        assert_eq!(
            hash_encoded_lazy(structure.lazy(None)).unwrap(),
            hash(&encoded(&value))
        );
        assert_eq!(
            hash_encoded_lazy(structure.lazy(Some("review"))).unwrap(),
            hash_encoded_without(&value, "review")
        );
        let json = serde_json::to_vec(&structure).unwrap();
        assert_eq!(json, serde_json::to_vec(&value).unwrap());
        assert_eq!(
            serde_json::from_slice::<Structure>(&json).unwrap(),
            structure
        );
        assert_eq!(
            serde_saphyr::to_string(&structure).unwrap(),
            serde_saphyr::to_string(&value).unwrap()
        );
        let dir = tempfile::tempdir().unwrap();
        for name in ["s.json", "s.yml"] {
            let path = dir.path().join(name);
            crate::data::write_lazy(&path, structure.lazy(None)).unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                crate::data::serialized(&path, &value).unwrap()
            );
        }
    }

    #[test]
    fn cells_outside_the_contract_do_not_read() {
        for cell in [
            json!({"id":"a","address":"A1","role":"boss","headers":[],"text_state":"read"}),
            json!({"id":"a","address":"A1","role":"data","headers":[],"text_state":"read","x":1}),
            json!({"id":"a","address":"A1","role":"data","headers":[]}),
        ] {
            assert!(ElementCell::from_value(cell).is_err());
        }
        let mut value = sample();
        value["elements"][0]["cells"][1]["headers"] = json!(["s1-A1", "s1-A1"]);
        let structure = Structure::from_value(value).unwrap();
        assert!(structure.check(|_| Ok(())).is_err());
    }
}
