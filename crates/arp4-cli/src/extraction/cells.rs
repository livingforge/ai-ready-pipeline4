//! The cells of an extraction sheet, held compactly, as a document has far
//! more cells than any other fact: each cell is a record of fixed size, whose
//! texts (its ID, address, formula and string value) lie in one buffer of its
//! sheet and whose number format is an index into a table of its sheet. A
//! string value is kept once however many cells of the sheet show it, and a
//! cell's address lies at the end of its ID or right after it. The formulas,
//! which few cells have, are listed apart. A [`Cell`] reads a cell from them.
use super::CellKind;
use anyhow::{Context, Result, bail};
use serde::{Serialize, Serializer};
use serde_json::{Number, Value};
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

/// A text in the buffer of a sheet's cells.
#[derive(Clone, Copy, Debug)]
struct Span {
    start: u32,
    len: u32,
}

/// The span standing for no text, such as the formula of a cell without one.
const NONE: Span = Span {
    start: u32::MAX,
    len: 0,
};

/// The index standing for no entry of a font or style table.
const NO_INDEX: u32 = u32::MAX;

/// A value as a cell record holds it: a number as JSON reads it, so that it is
/// written back as it was read, and a text in the buffer.
#[derive(Clone, Copy, Debug)]
enum Scalar {
    Null,
    Bool(bool),
    PosInt(u64),
    NegInt(i64),
    Float(f64),
    Text(Span),
}

impl Scalar {
    /// The scalar as a record holds it: what it is, and its bits.
    fn pack(self) -> (u32, u64) {
        match self {
            Self::Null => (0, 0),
            Self::Bool(value) => (1, u64::from(value)),
            Self::PosInt(value) => (2, value),
            Self::NegInt(value) => (3, value as u64),
            Self::Float(value) => (4, value.to_bits()),
            Self::Text(span) => (5, u64::from(span.start) | (u64::from(span.len) << 32)),
        }
    }

    fn unpack(what: u32, bits: u64) -> Self {
        match what {
            0 => Self::Null,
            1 => Self::Bool(bits != 0),
            2 => Self::PosInt(bits),
            3 => Self::NegInt(bits as i64),
            4 => Self::Float(f64::from_bits(bits)),
            _ => Self::Text(Span {
                start: bits as u32,
                len: (bits >> 32) as u32,
            }),
        }
    }
}

/// A cell as its sheet holds it in 32 bytes: its value's bits and its ID as
/// a span of the buffer; its address, kind and what its value is packed in
/// one word; its number format and its font and style as indexes into tables.
#[derive(Clone, Copy, Debug)]
struct Record {
    value: u64,
    id: Span,
    /// The length of the address (the low 24 bits), whether it follows the
    /// ID rather than ending it, the kind and what the value is.
    packed: u32,
    number_format: u32,
    font: u32,
    style: u32,
}

/// The longest address a record holds.
const ADDRESS_LEN: u32 = (1 << 24) - 1;
/// The bit of an address that follows its ID.
const FOLLOWS: u32 = 1 << 24;

/// The kinds of cell by their numbers in a record.
const KINDS: [CellKind; 6] = [
    CellKind::String,
    CellKind::Number,
    CellKind::Boolean,
    CellKind::Null,
    CellKind::Formula,
    CellKind::Error,
];

impl Record {
    fn address(&self) -> Span {
        let len = self.packed & ADDRESS_LEN;
        let end = self.id.start + self.id.len;
        Span {
            start: if self.packed & FOLLOWS != 0 {
                end
            } else {
                end - len
            },
            len,
        }
    }

    fn kind(&self) -> CellKind {
        KINDS[((self.packed >> 25) & 7) as usize]
    }

    fn value(&self) -> Scalar {
        Scalar::unpack((self.packed >> 28) & 7, self.value)
    }
}

/// The cells of a sheet, in order.
#[derive(Clone, Default)]
pub struct Cells {
    records: Vec<Record>,
    text: String,
    number_formats: Vec<Box<str>>,
    /// The formulas of the cells that have one, by cell index, in order.
    formulas: Vec<(u32, Span)>,
    /// The places in a text original, by cell index, of the cells that have one.
    positions: Vec<(u32, Value)>,
}

impl Cells {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<Cell<'_>> {
        (index < self.records.len()).then_some(Cell {
            cells: self,
            index: index as u32,
        })
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            cells: self,
            range: 0..self.records.len() as u32,
        }
    }

    /// The cells from `start` up to `end`.
    pub fn range(&self, start: usize, end: usize) -> Iter<'_> {
        let end = end.min(self.records.len());
        Iter {
            cells: self,
            range: start.min(end) as u32..end as u32,
        }
    }

    fn text(&self, span: Span) -> &str {
        let start = span.start as usize;
        &self.text[start..start + span.len as usize]
    }
}

impl std::fmt::Debug for Cells {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Cells are alike when each holds what the other's does, however their
/// texts lie in their buffers.
impl PartialEq for Cells {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<'a> IntoIterator for &'a Cells {
    type Item = Cell<'a>;
    type IntoIter = Iter<'a>;
    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

#[derive(Clone)]
pub struct Iter<'a> {
    cells: &'a Cells,
    range: std::ops::Range<u32>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Cell<'a>;
    fn next(&mut self) -> Option<Cell<'a>> {
        let index = self.range.next()?;
        Some(Cell {
            cells: self.cells,
            index,
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let index = self.range.next_back()?;
        Some(Cell {
            cells: self.cells,
            index,
        })
    }
}

impl ExactSizeIterator for Iter<'_> {}

/// A cell of an extraction sheet: its value, for a formula its cached
/// result, and its font and style as indexes into the tables of its sheet.
#[derive(Clone, Copy)]
pub struct Cell<'a> {
    cells: &'a Cells,
    index: u32,
}

impl<'a> Cell<'a> {
    fn record(&self) -> &'a Record {
        &self.cells.records[self.index as usize]
    }

    /// Its index among the cells of its sheet.
    pub fn index(&self) -> usize {
        self.index as usize
    }

    pub fn id(&self) -> &'a str {
        self.cells.text(self.record().id)
    }

    pub fn address(&self) -> &'a str {
        self.cells.text(self.record().address())
    }

    pub fn kind(&self) -> CellKind {
        self.record().kind()
    }

    pub fn value(&self) -> CellValue<'a> {
        match self.record().value() {
            Scalar::Null => CellValue::Null,
            Scalar::Bool(value) => CellValue::Bool(value),
            Scalar::PosInt(value) => CellValue::Number(CellNumber::PosInt(value)),
            Scalar::NegInt(value) => CellValue::Number(CellNumber::NegInt(value)),
            Scalar::Float(value) => CellValue::Number(CellNumber::Float(value)),
            Scalar::Text(span) => CellValue::String(self.cells.text(span)),
        }
    }

    pub fn formula(&self) -> Option<&'a str> {
        let formulas = &self.cells.formulas;
        let at = formulas
            .binary_search_by_key(&self.index, |&(index, _)| index)
            .ok()?;
        Some(self.cells.text(formulas[at].1))
    }

    /// Shared by the cells of a sheet with the same format.
    pub fn number_format(&self) -> &'a str {
        &self.cells.number_formats[self.record().number_format as usize]
    }

    pub fn font(&self) -> Option<usize> {
        index(self.record().font)
    }

    pub fn style(&self) -> Option<usize> {
        index(self.record().style)
    }

    /// Where the text of a text original lies.
    pub fn position(&self) -> Option<&'a Value> {
        let positions = &self.cells.positions;
        positions
            .binary_search_by_key(&self.index, |(index, _)| *index)
            .ok()
            .map(|at| &positions[at].1)
    }

    /// The cached result a cell carries in its expanded form: its value for a
    /// formula, null otherwise.
    pub fn cached(&self) -> CellValue<'a> {
        if self.kind() == CellKind::Formula {
            self.value()
        } else {
            CellValue::Null
        }
    }
}

fn index(index: u32) -> Option<usize> {
    (index != NO_INDEX).then_some(index as usize)
}

impl std::fmt::Debug for Cell<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cell")
            .field("id", &self.id())
            .field("address", &self.address())
            .field("kind", &self.kind())
            .field("value", &self.value())
            .field("formula", &self.formula())
            .field("number_format", &self.number_format())
            .field("font", &self.font())
            .field("style", &self.style())
            .field("position", &self.position())
            .finish()
    }
}

impl PartialEq for Cell<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
            && self.address() == other.address()
            && self.kind() == other.kind()
            && self.value() == other.value()
            && self.formula() == other.formula()
            && self.number_format() == other.number_format()
            && self.font() == other.font()
            && self.style() == other.style()
            && self.position() == other.position()
    }
}

/// A number as JSON reads it: an integer stays an integer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellNumber {
    PosInt(u64),
    NegInt(i64),
    Float(f64),
}

impl CellNumber {
    pub fn as_f64(self) -> f64 {
        match self {
            Self::PosInt(value) => value as f64,
            Self::NegInt(value) => value as f64,
            Self::Float(value) => value,
        }
    }

    pub fn to_number(self) -> Number {
        match self {
            Self::PosInt(value) => value.into(),
            Self::NegInt(value) => value.into(),
            Self::Float(value) => Number::from_f64(value).expect("a cell number is finite"),
        }
    }

    fn of(number: &Number) -> Self {
        if let Some(value) = number.as_u64() {
            Self::PosInt(value)
        } else if let Some(value) = number.as_i64() {
            Self::NegInt(value)
        } else {
            Self::Float(number.as_f64().expect("a JSON number is an f64"))
        }
    }
}

/// The value of a cell, a scalar of JSON, read from its sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellValue<'a> {
    Null,
    Bool(bool),
    Number(CellNumber),
    String(&'a str),
}

impl<'a> CellValue<'a> {
    pub fn is_null(self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn is_string(self) -> bool {
        matches!(self, Self::String(_))
    }

    pub fn is_number(self) -> bool {
        matches!(self, Self::Number(_))
    }

    pub fn is_boolean(self) -> bool {
        matches!(self, Self::Bool(_))
    }

    /// Its type, as [`crate::data::kind`] names that of a value.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
        }
    }

    pub fn as_str(self) -> Option<&'a str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(value),
            _ => None,
        }
    }

    /// As [`Value::as_f64`] reads it.
    pub fn as_f64(self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(number.as_f64()),
            _ => None,
        }
    }

    /// As [`Value::as_u64`] reads it.
    pub fn as_u64(self) -> Option<u64> {
        match self {
            Self::Number(CellNumber::PosInt(value)) => Some(value),
            _ => None,
        }
    }

    /// As [`Value::as_i64`] reads it.
    pub fn as_i64(self) -> Option<i64> {
        match self {
            Self::Number(CellNumber::PosInt(value)) => i64::try_from(value).ok(),
            Self::Number(CellNumber::NegInt(value)) => Some(value),
            _ => None,
        }
    }

    pub fn to_json(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(number) => Value::Number(number.to_number()),
            Self::String(text) => Value::String(text.to_owned()),
        }
    }
}

impl<'a> CellValue<'a> {
    /// The value a cell holds that `value` gives as JSON, none for a list or
    /// an object.
    pub fn of(value: &'a Value) -> Option<Self> {
        Some(match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Bool(*value),
            Value::Number(number) => Self::Number(CellNumber::of(number)),
            Value::String(text) => Self::String(text),
            _ => return None,
        })
    }
}

impl From<CellValue<'_>> for Value {
    fn from(value: CellValue<'_>) -> Self {
        value.to_json()
    }
}

impl Serialize for CellValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(value) => serializer.serialize_bool(value),
            Self::Number(number) => number.to_number().serialize(serializer),
            Self::String(text) => serializer.serialize_str(text),
        }
    }
}

/// Compared as [`CellValue::to_json`] would be, without making it.
impl PartialEq<Value> for CellValue<'_> {
    fn eq(&self, other: &Value) -> bool {
        match (*self, other) {
            (Self::Null, Value::Null) => true,
            (Self::Bool(a), Value::Bool(b)) => a == *b,
            (Self::Number(a), Value::Number(b)) => a == CellNumber::of(b),
            (Self::String(a), Value::String(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq<CellValue<'_>> for Value {
    fn eq(&self, other: &CellValue<'_>) -> bool {
        other == self
    }
}

impl PartialEq<str> for CellValue<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for CellValue<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl std::fmt::Display for CellValue<'_> {
    /// As JSON, as [`Value`] displays.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.to_json(), f)
    }
}

/// A cell to add to the cells of a sheet, with its font and style as indexes
/// into the tables of its sheet.
pub struct NewCell<'a> {
    pub id: &'a str,
    pub address: &'a str,
    pub kind: CellKind,
    pub value: &'a Value,
    pub formula: Option<&'a str>,
    pub number_format: &'a str,
    pub font: Option<usize>,
    pub style: Option<usize>,
    pub position: Option<Value>,
}

/// The cells of a sheet being added one at a time.
#[derive(Default)]
pub struct CellsBuilder {
    cells: Cells,
    /// The string values added so far, by the hash of their text.
    strings: HashMap<u64, Span>,
    hasher: RandomState,
    number_formats: HashMap<Box<str>, u32>,
}

impl CellsBuilder {
    /// The number of cells added so far.
    pub fn count(&self) -> usize {
        self.cells.records.len()
    }

    pub fn push(&mut self, cell: NewCell<'_>) -> Result<()> {
        let index = u32::try_from(self.cells.records.len()).context("too many cells in a sheet")?;
        let id = self.add(cell.id)?;
        // An Excel cell's ID ends in its address: the address is the end of
        // the ID's text, or else the text right after it.
        let address_len = u32::try_from(cell.address.len())
            .ok()
            .filter(|&len| len <= ADDRESS_LEN)
            .context("cell address too long")?;
        let follows = if cell.id.ends_with(cell.address) {
            0
        } else {
            self.add(cell.address)?;
            FOLLOWS
        };
        let value = match cell.value {
            Value::Null => Scalar::Null,
            Value::Bool(value) => Scalar::Bool(*value),
            Value::Number(number) => match CellNumber::of(number) {
                CellNumber::PosInt(value) => Scalar::PosInt(value),
                CellNumber::NegInt(value) => Scalar::NegInt(value),
                CellNumber::Float(value) => Scalar::Float(value),
            },
            Value::String(text) => Scalar::Text(self.shared(text)?),
            _ => bail!("value of cell {} must be a scalar", cell.address),
        };
        if let Some(formula) = cell.formula {
            let formula = self.add(formula)?;
            self.cells.formulas.push((index, formula));
        }
        let number_format = match self.number_formats.get(cell.number_format) {
            Some(&at) => at,
            None => {
                let at = self.cells.number_formats.len() as u32;
                self.cells.number_formats.push(cell.number_format.into());
                self.number_formats.insert(cell.number_format.into(), at);
                at
            }
        };
        let table = |index: Option<usize>| match index {
            None => Ok(NO_INDEX),
            Some(index) => u32::try_from(index)
                .ok()
                .filter(|&index| index != NO_INDEX)
                .context("font or style index too large"),
        };
        let kind = KINDS
            .iter()
            .position(|&kind| kind == cell.kind)
            .expect("every kind is listed") as u32;
        let (what, value) = value.pack();
        self.cells.records.push(Record {
            value,
            id,
            packed: address_len | follows | (kind << 25) | (what << 28),
            number_format,
            font: table(cell.font)?,
            style: table(cell.style)?,
        });
        if let Some(position) = cell.position {
            self.cells.positions.push((index, position));
        }
        Ok(())
    }

    /// `text` added to the buffer.
    fn add(&mut self, text: &str) -> Result<Span> {
        // The end stays below the start of NONE.
        let span = u32::try_from(self.cells.text.len())
            .ok()
            .zip(u32::try_from(text.len()).ok())
            .filter(|&(start, len)| start.checked_add(len).is_some_and(|end| end < NONE.start))
            .map(|(start, len)| Span { start, len })
            .context("the texts of a sheet's cells exceed 4 GiB")?;
        self.cells.text.push_str(text);
        Ok(span)
    }

    /// `text` as a string value, kept once for the cells showing it.
    fn shared(&mut self, text: &str) -> Result<Span> {
        let key = self.hasher.hash_one(text);
        if let Some(&span) = self.strings.get(&key)
            && self.cells.text(span) == text
        {
            return Ok(span);
        }
        let span = self.add(text)?;
        self.strings.entry(key).or_insert(span);
        Ok(span)
    }

    pub fn finish(self) -> Cells {
        let cells = self.cells;
        Cells {
            records: crate::data::exact(cells.records),
            text: crate::data::exact_text(cells.text),
            formulas: crate::data::exact(cells.formulas),
            positions: crate::data::exact(cells.positions),
            ..cells
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn push(builder: &mut CellsBuilder, id: &str, address: &str, value: Value) {
        builder
            .push(NewCell {
                id,
                address,
                kind: CellKind::String,
                value: &value,
                formula: None,
                number_format: "General",
                font: None,
                style: Some(0),
                position: None,
            })
            .unwrap();
    }

    /// A record holds no text of its own, and the texts of a sheet are kept
    /// once: the address within the ID, a string value however many cells show it.
    #[test]
    fn cells_keep_each_text_once() {
        assert!(std::mem::size_of::<Record>() <= 32);
        let mut builder = CellsBuilder::default();
        push(&mut builder, "c-1-A1", "A1", json!("same"));
        push(&mut builder, "c-1-B1", "B1", json!("same"));
        push(&mut builder, "text-1", "A2", json!(3));
        let cells = builder.finish();
        assert_eq!(cells.text, "c-1-A1samec-1-B1text-1A2");
        let [a, b, c] = [0, 1, 2].map(|i| cells.get(i).unwrap());
        assert_eq!(
            (a.id(), a.address(), a.value()),
            ("c-1-A1", "A1", CellValue::String("same"))
        );
        assert_eq!((b.address(), b.value().as_str()), ("B1", Some("same")));
        assert_eq!((c.id(), c.address(), c.formula()), ("text-1", "A2", None));
        assert_eq!(
            (a.font(), a.style(), a.number_format()),
            (None, Some(0), "General")
        );
    }

    /// A number is given back as JSON read it, an integer as an integer.
    #[test]
    fn numbers_keep_their_form() {
        for value in [
            json!(3),
            json!(-3),
            json!(3.0),
            json!(4.5),
            json!(u64::MAX),
            json!(i64::MIN),
            json!(f64::MIN_POSITIVE),
            json!(true),
            json!(false),
            Value::Null,
        ] {
            let mut builder = CellsBuilder::default();
            push(&mut builder, "c-1-A1", "A1", value.clone());
            let cells = builder.finish();
            let read = cells.get(0).unwrap().value();
            assert_eq!(read.to_json(), value);
            assert_eq!(serde_json::to_string(&read).unwrap(), value.to_string());
            assert!(read == value);
        }
    }

    /// Each cell keeps its kind and formula, a formula listed apart for the
    /// few cells that have one.
    #[test]
    fn cells_keep_their_kinds_and_formulas() {
        let mut builder = CellsBuilder::default();
        let kinds = [
            CellKind::String,
            CellKind::Number,
            CellKind::Boolean,
            CellKind::Null,
            CellKind::Formula,
            CellKind::Error,
        ];
        for (at, kind) in kinds.into_iter().enumerate() {
            let address = format!("A{}", at + 1);
            let formula = (kind == CellKind::Formula).then_some("SUM(B1:B2)");
            builder
                .push(NewCell {
                    id: &format!("c-1-{address}"),
                    address: &address,
                    kind,
                    value: &json!("x"),
                    formula,
                    number_format: "General",
                    font: Some(at),
                    style: None,
                    position: None,
                })
                .unwrap();
        }
        let cells = builder.finish();
        for (at, kind) in kinds.into_iter().enumerate() {
            let cell = cells.get(at).unwrap();
            assert_eq!(cell.kind(), kind);
            assert_eq!(cell.address(), format!("A{}", at + 1));
            assert_eq!(cell.font(), Some(at));
            assert_eq!(
                cell.formula(),
                (kind == CellKind::Formula).then_some("SUM(B1:B2)")
            );
        }
    }
}
