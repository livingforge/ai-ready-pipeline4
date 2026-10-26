use anyhow::{Context, Result, bail, ensure};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

mod yaml;
pub(crate) use yaml::{Node as YamlNode, Reader as YamlReader};

/// Replaces non-overlapping `edits` of `original` in one pass. Replacing each
/// range in place would move the rest of a large part for every edit.
pub fn splice(
    original: &str,
    mut edits: Vec<(std::ops::Range<usize>, String)>,
    what: &str,
) -> Result<String> {
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = String::with_capacity(original.len());
    let mut end = 0;
    for (range, replacement) in edits {
        ensure!(end <= range.start, "overlapping {what} edits");
        result.push_str(&original[end..range.start]);
        result.push_str(&replacement);
        end = range.end;
    }
    result.push_str(&original[end..]);
    Ok(result)
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn string(value: &Value) -> Result<&str> {
    value.as_str().context("expected string")
}
pub fn array(value: &Value) -> Result<&Vec<Value>> {
    value.as_array().context("expected array")
}
pub fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        _ => "object",
    }
}

/// Where the canonical encoding is written: a buffer, or a hash fed as it goes.
trait Sink {
    fn put(&mut self, text: &str);
}

impl Sink for String {
    fn put(&mut self, text: &str) {
        self.push_str(text);
    }
}

/// A SHA-256 fed in blocks, so that hashing an encoding does not hold it.
struct HashSink {
    hasher: Sha256,
    buffer: Vec<u8>,
}

impl HashSink {
    const BLOCK: usize = 1 << 16;

    fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            buffer: Vec::with_capacity(Self::BLOCK),
        }
    }

    fn finish(mut self) -> String {
        self.hasher.update(&self.buffer);
        format!("{:x}", self.hasher.finalize())
    }
}

impl Sink for HashSink {
    fn put(&mut self, text: &str) {
        self.buffer.extend_from_slice(text.as_bytes());
        if self.buffer.len() >= Self::BLOCK {
            self.hasher.update(&self.buffer);
            self.buffer.clear();
        }
    }
}

/// A writer the encoding goes to as it is made, keeping the first error.
struct WriteSink<W: Write> {
    out: W,
    error: Option<std::io::Error>,
}

impl<W: Write> Sink for WriteSink<W> {
    fn put(&mut self, text: &str) {
        if self.error.is_none()
            && let Err(error) = self.out.write_all(text.as_bytes())
        {
            self.error = Some(error);
        }
    }
}

fn indent(out: &mut impl Sink, depth: usize) {
    for _ in 0..depth {
        out.put("  ");
    }
}

/// The members of an object, which an empty one has none of. The items of an
/// array held by a member named in `inline` are written one per line.
fn render_fields<'a>(
    fields: impl Iterator<Item = (&'a String, &'a Value)>,
    depth: usize,
    inline: &[&str],
    out: &mut impl Sink,
) {
    let mut fields = fields.peekable();
    if fields.peek().is_none() {
        out.put("{}");
        return;
    }
    out.put(
        "{
",
    );
    for (index, (k, v)) in fields.enumerate() {
        if index > 0 {
            out.put(
                ",
",
            );
        }
        indent(out, depth + 1);
        out.put(&serde_json::to_string(k).unwrap());
        out.put(": ");
        match v {
            Value::Array(items) if !items.is_empty() && inline.contains(&k.as_str()) => {
                out.put(
                    "[
",
                );
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.put(
                            ",
",
                        );
                    }
                    indent(out, depth + 2);
                    render_line(item, out);
                }
                out.put(
                    "
",
                );
                indent(out, depth + 1);
                out.put("]");
            }
            _ => render(v, depth + 1, inline, out),
        }
    }
    out.put(
        "
",
    );
    indent(out, depth);
    out.put("}");
}

// Canonical JSON encoding preserves exponent signs and float representation for stable hashes.
fn render(v: &Value, depth: usize, inline: &[&str], out: &mut impl Sink) {
    match v {
        Value::Object(map) => render_fields(map.iter(), depth, inline, out),
        Value::Array(items) if !items.is_empty() => {
            out.put(
                "[
",
            );
            for (index, v) in items.iter().enumerate() {
                if index > 0 {
                    out.put(
                        ",
",
                    );
                }
                indent(out, depth + 1);
                render(v, depth + 1, inline, out);
            }
            out.put(
                "
",
            );
            indent(out, depth);
            out.put("]");
        }
        _ => render_scalar(v, out),
    }
}

/// `v` on one line, its members and items separated by `, `.
fn render_line(v: &Value, out: &mut impl Sink) {
    match v {
        Value::Object(map) => {
            out.put("{");
            for (index, (k, v)) in map.iter().enumerate() {
                if index > 0 {
                    out.put(", ");
                }
                out.put(&serde_json::to_string(k).unwrap());
                out.put(": ");
                render_line(v, out);
            }
            out.put("}");
        }
        Value::Array(items) => {
            out.put("[");
            for (index, v) in items.iter().enumerate() {
                if index > 0 {
                    out.put(", ");
                }
                render_line(v, out);
            }
            out.put("]");
        }
        _ => render_scalar(v, out),
    }
}

fn render_scalar(v: &Value, out: &mut impl Sink) {
    match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap();
            if f != 0.0 && (f.abs() >= 1e16 || f.abs() < 1e-4) {
                let repr = format!("{f:e}");
                let (mantissa, exponent) = repr.split_once('e').unwrap();
                let exp: i32 = exponent.parse().unwrap();
                out.put(&format!("{mantissa}e{exp:+03}"));
            } else {
                let s = f.to_string();
                out.put(&s);
                if !s.contains('.') {
                    out.put(".0");
                }
            }
        }
        _ => out.put(&serde_json::to_string(v).unwrap()),
    }
}

/// Written into one buffer: rendering each nested value to its own string would
/// copy every byte once per enclosing level of large inputs and extractions.
pub fn encoded(value: &Value) -> Vec<u8> {
    encoded_lines(value, &[])
}

/// [`encoded`], with each item of an array held by a member named in `inline` on
/// one line of its own, as an extraction keeps its cells: a changed item is
/// one changed line of a diff.
pub fn encoded_lines(value: &Value, inline: &[&str]) -> Vec<u8> {
    let mut out = String::new();
    render(value, 0, inline, &mut out);
    out.push('\n');
    out.into_bytes()
}

/// `hash(&encoded(value))`, without holding the encoding.
pub fn hash_encoded(value: &Value) -> String {
    hash_encoded_lines(value, &[])
}

/// `hash(&encoded_lines(value, inline))`, without holding the encoding.
pub fn hash_encoded_lines(value: &Value, inline: &[&str]) -> String {
    let mut digest = HashSink::new();
    render(value, 0, inline, &mut digest);
    digest.put(
        "
",
    );
    digest.finish()
}

/// [`hash_encoded`] of an object without its member `skip`, without copying it.
pub fn hash_encoded_without(value: &Value, skip: &str) -> String {
    let mut digest = HashSink::new();
    match value.as_object() {
        Some(map) => render_fields(map.iter().filter(|(k, _)| *k != skip), 0, &[], &mut digest),
        None => render(value, 0, &[], &mut digest),
    }
    digest.put(
        "
",
    );
    digest.finish()
}

/// A value [`hash_encoded_lazy`] hashes as [`hash_encoded`] hashes it, with
/// the items of its arrays made only as they are hashed: a large array is not
/// held whole.
pub enum Lazy<'a> {
    Value(Value),
    Borrowed(&'a Value),
    /// An object, its members in any order.
    Object(Vec<(&'a str, Lazy<'a>)>),
    Items(Box<dyn Iterator<Item = Result<Lazy<'a>>> + 'a>),
    /// A value the line encodings write whole on one line, as a list short
    /// enough to read at a glance.
    Line(Value),
}

impl<'a> Lazy<'a> {
    /// The members of the object `fields` without those in `skip`, borrowed.
    pub fn members(fields: &'a Value, skip: &[&str]) -> Vec<(&'a str, Lazy<'a>)> {
        fields
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| !skip.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), Lazy::Borrowed(value)))
            .collect()
    }

    /// The value `self` stands for, made whole.
    pub fn into_value(self) -> Result<Value> {
        Ok(match self {
            Lazy::Value(value) | Lazy::Line(value) => value,
            Lazy::Borrowed(value) => value.clone(),
            Lazy::Object(members) => Value::Object(
                members
                    .into_iter()
                    .map(|(key, member)| Ok((key.to_owned(), member.into_value()?)))
                    .collect::<Result<_>>()?,
            ),
            Lazy::Items(items) => Value::Array(
                items
                    .map(|item| item?.into_value())
                    .collect::<Result<_>>()?,
            ),
        })
    }
}

/// A [`Lazy`] serialized once, as the value it stands for is: an object's
/// members in order and an array's items as they are made.
struct Once<'a>(std::cell::RefCell<Option<Lazy<'a>>>);

impl serde::Serialize for Once<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeMap, SerializeSeq};
        let lazy = self
            .0
            .borrow_mut()
            .take()
            .ok_or_else(|| S::Error::custom("a lazy value is written once"))?;
        match lazy {
            Lazy::Value(value) | Lazy::Line(value) => value.serialize(serializer),
            Lazy::Borrowed(value) => value.serialize(serializer),
            Lazy::Object(mut members) => {
                members.sort_by(|a, b| a.0.cmp(b.0));
                let mut map = serializer.serialize_map(Some(members.len()))?;
                for (key, member) in members {
                    map.serialize_entry(key, &Once(std::cell::RefCell::new(Some(member))))?;
                }
                map.end()
            }
            Lazy::Items(items) => {
                let mut seq = serializer.serialize_seq(None)?;
                for item in items {
                    let item = item.map_err(|error| S::Error::custom(format!("{error:#}")))?;
                    seq.serialize_element(&Once(std::cell::RefCell::new(Some(item))))?;
                }
                seq.end()
            }
        }
    }
}

/// [`render`] of `value`, the items of an array held by a member named in
/// `inline` on one line of their own, as [`encoded_lines`] writes them, and a
/// [`Lazy::Line`] whole on one line.
fn render_lazy(value: Lazy<'_>, depth: usize, inline: &[&str], out: &mut impl Sink) -> Result<()> {
    match value {
        Lazy::Value(value) => render(&value, depth, inline, out),
        Lazy::Borrowed(value) => render(value, depth, inline, out),
        Lazy::Line(value) => render_line(&value, out),
        Lazy::Object(mut members) => {
            if members.is_empty() {
                out.put("{}");
                return Ok(());
            }
            members.sort_by(|a, b| a.0.cmp(b.0));
            out.put("{\n");
            for (index, (key, member)) in members.into_iter().enumerate() {
                if index > 0 {
                    out.put(",\n");
                }
                indent(out, depth + 1);
                out.put(&serde_json::to_string(key).unwrap());
                out.put(": ");
                if inline.contains(&key) {
                    render_lines(member, depth + 1, inline, out)?;
                } else {
                    render_lazy(member, depth + 1, inline, out)?;
                }
            }
            out.put("\n");
            indent(out, depth);
            out.put("}");
        }
        Lazy::Items(items) => {
            let mut empty = true;
            for item in items {
                let item = item?;
                out.put(if empty { "[\n" } else { ",\n" });
                empty = false;
                indent(out, depth + 1);
                render_lazy(item, depth + 1, inline, out)?;
            }
            if empty {
                out.put("[]");
            } else {
                out.put("\n");
                indent(out, depth);
                out.put("]");
            }
        }
    }
    Ok(())
}

/// The member `value` of a member named in the `inline` of [`render_lazy`]:
/// a non-empty array with each item on one line, anything else as
/// [`render`] writes it.
fn render_lines(value: Lazy<'_>, depth: usize, inline: &[&str], out: &mut impl Sink) -> Result<()> {
    let items: Box<dyn Iterator<Item = Result<Value>> + '_> = match value {
        Lazy::Items(items) => Box::new(items.map(|item| item?.into_value())),
        Lazy::Borrowed(Value::Array(items)) => Box::new(items.iter().cloned().map(Ok)),
        Lazy::Value(Value::Array(items)) => Box::new(items.into_iter().map(Ok)),
        other => return render_lazy(other, depth, inline, out),
    };
    let mut empty = true;
    for item in items {
        let item = item?;
        out.put(if empty { "[\n" } else { ",\n" });
        empty = false;
        indent(out, depth + 1);
        render_line(&item, out);
    }
    if empty {
        out.put("[]");
    } else {
        out.put("\n");
        indent(out, depth);
        out.put("]");
    }
    Ok(())
}

/// `hash(&encoded(value))` of the value `value` stands for, made only as it is
/// hashed.
pub fn hash_encoded_lazy(value: Lazy<'_>) -> Result<String> {
    hash_encoded_lazy_lines(value, &[])
}

/// `hash(&encoded_lines(value, inline))` of the value `value` stands for,
/// made only as it is hashed.
pub fn hash_encoded_lazy_lines(value: Lazy<'_>, inline: &[&str]) -> Result<String> {
    let mut digest = HashSink::new();
    render_lazy(value, 0, inline, &mut digest)?;
    digest.put("\n");
    Ok(digest.finish())
}

/// Whether `expected`, each `\r\n` read as `\n`, is `encoded_lines(value,
/// inline)` of the value `value` stands for: compared as it is made rather
/// than held, as the encoding of a large extraction is.
pub fn is_encoded_lazy_lines(value: Lazy<'_>, inline: &[&str], expected: &[u8]) -> Result<bool> {
    let mut compared = CompareSink {
        rest: expected,
        crlf: expected.contains(&b'\r'),
        same: true,
    };
    render_lazy(value, 0, inline, &mut compared)?;
    compared.put("\n");
    Ok(compared.same && compared.rest.is_empty())
}

/// What an encoding is compared with as it is made: the bytes not yet
/// matched, and whether they all matched so far.
struct CompareSink<'a> {
    rest: &'a [u8],
    /// Whether `rest` holds a `\r`, each before `\n` read as nothing.
    crlf: bool,
    same: bool,
}

impl Sink for CompareSink<'_> {
    fn put(&mut self, text: &str) {
        if !self.same {
            return;
        }
        if !self.crlf {
            match self.rest.strip_prefix(text.as_bytes()) {
                Some(rest) => self.rest = rest,
                None => self.same = false,
            }
            return;
        }
        for &byte in text.as_bytes() {
            if self.rest.starts_with(b"\r\n") {
                self.rest = &self.rest[1..];
            }
            match self.rest.split_first() {
                Some((&next, rest)) if next == byte => self.rest = rest,
                _ => {
                    self.same = false;
                    return;
                }
            }
        }
    }
}

/// `encoded_lines(value, inline)` of the value `value` stands for, made as it
/// is written into the buffer.
pub fn encoded_lazy_lines(value: Lazy<'_>, inline: &[&str]) -> Result<Vec<u8>> {
    let mut out = String::new();
    render_lazy(value, 0, inline, &mut out)?;
    out.push('\n');
    Ok(out.into_bytes())
}

// JSON with unique keys and finite numbers. Each node is moved into its parent:
/// `json!` on a built array would copy it again at every enclosing level.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("finite JSON-compatible value with unique string keys")
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut list = vec![];
                while let Some(Strict(v)) = seq.next_element()? {
                    list.push(v)
                }
                Ok(Strict(Value::Array(list)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(Strict(key)) = map.next_key()? {
                    let Value::String(key) = key else {
                        return Err(de::Error::custom("keys must be strings"));
                    };
                    let Strict(v) = map.next_value()?;
                    if values.insert(key, v).is_some() {
                        return Err(de::Error::custom("duplicate key"));
                    }
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// The members of a mapping besides a list, and the key and value of each
/// item of the list ([`parse_listing`]).
pub type Listing = (serde_json::Map<String, Value>, Vec<(String, Value)>);

/// The YAML document `text`, a mapping whose member `list` is a list of
/// mappings of one member each, as its other members and the key and value of
/// each item of `list`; none for any other document or one [`parse`] refuses.
pub fn parse_listing(text: &str, list: &str) -> Option<Listing> {
    yaml::parse_listing(text, list)
}
pub fn parse(text: &str, is_json: bool) -> Result<Value> {
    if is_json {
        Ok(serde_json::from_str::<Strict>(text)?.0)
    } else {
        yaml::parse(text)
    }
}
/// The compiled validator of document schema `name`. Inspection validates every
/// content page, so each schema is parsed and compiled once.
fn validator(name: &str) -> Result<std::sync::Arc<jsonschema::Validator>> {
    static VALIDATORS: std::sync::LazyLock<
        std::sync::Mutex<BTreeMap<String, std::sync::Arc<jsonschema::Validator>>>,
    > = std::sync::LazyLock::new(Default::default);
    let mut validators = VALIDATORS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(validator) = validators.get(name) {
        return Ok(validator.clone());
    }
    let schema = crate::schema(name).unwrap_or(Value::Null);
    let validator = std::sync::Arc::new(jsonschema::validator_for(&schema)?);
    validators.insert(name.to_owned(), validator.clone());
    Ok(validator)
}
pub fn validate(name: &str, value: &Value) -> Result<()> {
    if let Err(error) = validator(name)?.validate(value) {
        bail!("{name}: {error} at {}", error.instance_path())
    };
    // A journal recording nothing is left out.
    if name == "mappings"
        && let Some(journal) = value.get("interpretation")
    {
        crate::document_structure::validate_corrections(journal)?;
    }
    Ok(())
}
pub fn read(path: &Path, schema: Option<&str>) -> Result<Value> {
    let value = parse(
        &fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
        path.extension().is_some_and(|s| s == "json"),
    )?;
    if let Some(name) = schema {
        validate(name, &value)?
    }
    Ok(value)
}
/// Stores [`serialized`] of `value` at `path` as [`replace`] does, encoding
/// it straight into the file: a large output encoded whole first was held
/// twice, as the value and as its bytes.
pub fn write(path: &Path, value: &Value) -> Result<()> {
    write_lazy(path, Lazy::Borrowed(value))
}
/// [`write`] of the value `lazy` stands for, made only as it is written: a
/// large output is not held whole.
pub fn write_lazy(path: &Path, lazy: Lazy<'_>) -> Result<()> {
    write_lazy_lines(path, lazy, &[])
}

/// [`write_lazy`], a JSON file with each item of an array held by a member
/// named in `inline` on one line of its own, as [`encoded_lazy_lines`] writes
/// it.
pub fn write_lazy_lines(path: &Path, lazy: Lazy<'_>, inline: &[&str]) -> Result<()> {
    replace_streamed(path, |out| {
        if path.extension().is_some_and(|s| s == "json") {
            let mut sink = WriteSink { out, error: None };
            render_lazy(lazy, 0, inline, &mut sink)?;
            sink.put("\n");
            if let Some(error) = sink.error {
                return Err(error.into());
            }
        } else {
            serde_saphyr::to_io_writer(out, &Once(std::cell::RefCell::new(Some(lazy))))?;
        }
        Ok(())
    })
}
/// `items` in as much room as they take, for a list held long after it is
/// made. `Vec::shrink_to_fit` leaves a list in its room when the list takes
/// half of it or more, as the allocator keeps a block shrunk by no more than
/// half where it is: the items are moved to room of their own instead.
pub fn exact<T>(mut items: Vec<T>) -> Vec<T> {
    if items.capacity() == items.len() {
        return items;
    }
    let mut exact = Vec::with_capacity(items.len());
    exact.append(&mut items);
    exact
}
/// [`exact`] of a text.
pub fn exact_text(text: String) -> String {
    if text.capacity() == text.len() {
        return text;
    }
    let mut exact = String::with_capacity(text.len());
    exact.push_str(&text);
    exact
}
/// [`replace`] with the bytes `fill` writes, buffered, rather than bytes
/// held whole first.
pub fn replace_streamed(
    path: &Path,
    fill: impl FnOnce(&mut std::io::BufWriter<&mut fs::File>) -> Result<()>,
) -> Result<()> {
    replace_buffered(path, true, fill)
}
/// [`replace_cache`] with the bytes `fill` writes, buffered, as
/// [`replace_streamed`] writes them.
pub fn replace_cache_streamed(
    path: &Path,
    fill: impl FnOnce(&mut std::io::BufWriter<&mut fs::File>) -> Result<()>,
) -> Result<()> {
    replace_buffered(path, false, fill)
}
fn replace_buffered(
    path: &Path,
    durable: bool,
    fill: impl FnOnce(&mut std::io::BufWriter<&mut fs::File>) -> Result<()>,
) -> Result<()> {
    replace_with(path, durable, |file| {
        let mut out = std::io::BufWriter::with_capacity(1 << 16, file);
        fill(&mut out)?;
        out.flush()?;
        Ok(())
    })
}
/// The bytes [`write`] stores for `value` at `path`: canonical JSON or YAML.
pub fn serialized(path: &Path, value: &Value) -> Result<Vec<u8>> {
    Ok(if path.extension().is_some_and(|s| s == "json") {
        encoded(value)
    } else {
        serde_saphyr::to_string(value)?.into_bytes()
    })
}
pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    replace_with(path, true, |file| Ok(file.write_all(bytes)?))
}
/// [`replace`] for a file of `.arp/cache/` or of a workflow's status cache,
/// which is made again when it is lost or does not read back. It still takes
/// the place of the old file whole, but is not flushed to the disk first,
/// which took longer than writing it.
pub fn replace_cache(path: &Path, bytes: &[u8]) -> Result<()> {
    replace_with(path, false, |file| Ok(file.write_all(bytes)?))
}
/// A new file that `fill` writes takes the place of `path` whole, flushed to
/// the disk first when `durable`.
fn replace_with(
    path: &Path,
    durable: bool,
    fill: impl FnOnce(&mut fs::File) -> Result<()>,
) -> Result<()> {
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    fill(tmp.as_file_mut())?;
    if durable {
        tmp.as_file().sync_all()?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn immutable(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        ensure!(
            fs::read(path)? == bytes,
            "immutable evidence changed: {}",
            path.display()
        );
        return Ok(());
    }
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}
/// Remove `directory`, then each parent, while it is empty, stopping below `boundary`.
/// Best effort: the caller's operation is already complete, so a directory that is
/// not empty or is held open elsewhere ends the walk instead of failing it.
pub fn remove_empty_directories(directory: &Path, boundary: &Path) {
    let mut current = Some(directory);
    while let Some(directory) = current.filter(|d| d.starts_with(boundary) && *d != boundary) {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => break,
        }
        current = directory.parent();
    }
}
/// A temporary directory inside ARP-managed storage. Dropping it also removes the
/// parents it leaves empty, up to `boundary`.
pub struct Stage {
    directory: Option<tempfile::TempDir>,
    boundary: PathBuf,
}
impl Stage {
    pub fn new_in(parent: &Path, boundary: &Path) -> Result<Self> {
        fs::create_dir_all(parent)?;
        Ok(Self {
            directory: Some(tempfile::tempdir_in(parent)?),
            boundary: boundary.to_owned(),
        })
    }
    pub fn path(&self) -> &Path {
        self.directory.as_ref().unwrap().path()
    }
    /// Keep the directory, for example as recovery data after a failed rollback.
    pub fn keep(mut self) -> PathBuf {
        self.directory.take().unwrap().keep()
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if let Some(directory) = self.directory.take() {
            let parent = directory.path().parent().map(Path::to_owned);
            drop(directory);
            if let Some(parent) = parent {
                remove_empty_directories(&parent, &self.boundary);
            }
        }
    }
}
pub fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}
pub fn under(root: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !relative.is_empty() && !relative.contains('\\') && !relative.contains(':'),
        "invalid relative path: {relative}"
    );
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            bail!("path must stay inside project: {relative}")
        };
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) => ensure!(!is_link(&meta), "links are not allowed: {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}
pub fn files(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for e in fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            // Does not follow links. On Windows directory enumeration already
            // carries this metadata, avoiding an extra filesystem call per entry.
            let meta = e.metadata()?;
            ensure!(!is_link(&meta), "links are not allowed: {}", p.display());
            if meta.is_dir() {
                walk(base, &p, out)?
            } else if meta.is_file() {
                out.insert(
                    p.strip_prefix(base)?.to_string_lossy().replace('\\', "/"),
                    p,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out)?;
    Ok(out)
}
pub fn identifier(id: &str) -> Result<()> {
    static VALID: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$").unwrap());
    static RESERVED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$").unwrap()
    });
    ensure!(VALID.is_match(id), "invalid ID: {id}");
    ensure!(!RESERVED.is_match(id), "reserved ID: {id}");
    Ok(())
}

/// A document ID is the original's path below the sources directory, so that the
/// adopted records mirror the folders of the originals. Each segment must also be a
/// file name that Windows and Git can check out.
pub fn document_id(id: &str) -> Result<()> {
    static RESERVED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\..*)?$").unwrap()
    });
    ensure!(!id.is_empty(), "empty document ID");
    for segment in id.split('/') {
        ensure!(
            !segment.is_empty() && segment != "." && segment != "..",
            "invalid document ID: {id}"
        );
        ensure!(
            !segment
                .chars()
                .any(|c| c.is_control() || "\\:*?\"<>|".contains(c)),
            "document ID contains a character Windows does not allow in file names: {id}"
        );
        // Windows drops a trailing dot or space, so the checkout would name another file.
        ensure!(
            !segment.ends_with('.') && !segment.ends_with(' '),
            "document ID segment ends with a dot or space: {id}"
        );
        ensure!(
            !RESERVED.is_match(segment),
            "document ID uses a reserved Windows device name: {id}"
        );
    }
    Ok(())
}

/// A failure whose diagnostics reach the agent as JSON, never as an escaped string.
/// The payload carries `code` and `diagnostics`; Display keeps the persisted evidence
/// format (compact JSON) so history and failure classification are unchanged.
#[derive(Debug)]
pub struct Rejection(pub Value);
impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0.to_string())
    }
}
impl std::error::Error for Rejection {}
pub fn rejection(error: &anyhow::Error) -> Option<&Value> {
    error
        .chain()
        .find_map(|e| e.downcast_ref::<Rejection>())
        .map(|r| &r.0)
}

/// A text that JSON values equal by `==` share and no unequal value has, so
/// that a set or map can look values up instead of comparing with each one.
/// Object keys are written sorted, and both zeros of a float as one.
pub fn value_key(value: &Value) -> String {
    fn write(value: &Value, key: &mut String) {
        match value {
            Value::Null => key.push('n'),
            Value::Bool(true) => key.push('t'),
            Value::Bool(false) => key.push('f'),
            Value::Number(number) => {
                if let Some(n) = number.as_u64() {
                    key.push_str(&format!("u{n};"));
                } else if let Some(n) = number.as_i64() {
                    key.push_str(&format!("i{n};"));
                } else {
                    // `-0.0 == 0.0`; any other floats are equal only when the same.
                    let n = number.as_f64().unwrap_or(f64::NAN);
                    let n = if n == 0.0 { 0.0 } else { n };
                    key.push_str(&format!("d{n:?};"));
                }
            }
            Value::String(text) => {
                key.push('s');
                key.push_str(&Value::String(text.clone()).to_string());
            }
            Value::Array(values) => {
                key.push('[');
                for value in values {
                    write(value, key);
                }
                key.push(']');
            }
            Value::Object(fields) => {
                let mut names: Vec<&String> = fields.keys().collect();
                names.sort_unstable();
                key.push('{');
                for name in names {
                    key.push_str(&Value::String(name.clone()).to_string());
                    write(&fields[name], key);
                }
                key.push('}');
            }
        }
    }
    let mut key = String::new();
    write(value, &mut key);
    key
}

/// Group `"<document>: {finding JSON}"` warnings so a finding repeated across
/// documents is shown once with its document list. Unparseable entries keep their text.
pub fn grouped_warnings<'a>(warnings: impl IntoIterator<Item = &'a str>) -> Value {
    let mut groups: Vec<Value> = Vec::new();
    // The group of each finding by its fields other than `documents`, and the
    // documents each group lists, looked up rather than compared with every group.
    let mut by_finding: std::collections::HashMap<String, usize> = Default::default();
    let mut listed: Vec<std::collections::HashSet<&str>> = Vec::new();
    for warning in warnings {
        let parsed = warning.split_once(": ").and_then(|(document, rest)| {
            serde_json::from_str::<Value>(rest)
                .ok()
                .filter(Value::is_object)
                .map(|finding| (document, finding))
        });
        let Some((document, finding)) = parsed else {
            groups.push(json!({"message":warning}));
            listed.push(Default::default());
            continue;
        };
        // A group matches a finding with the same fields besides `documents`.
        let mut fields = finding.clone();
        fields.as_object_mut().unwrap().remove("documents");
        match by_finding.entry(value_key(&fields)) {
            std::collections::hash_map::Entry::Occupied(group) => {
                let index = *group.get();
                if listed[index].insert(document) {
                    groups[index]["documents"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(document));
                }
            }
            std::collections::hash_map::Entry::Vacant(group) => {
                group.insert(groups.len());
                let mut group = finding;
                group["documents"] = json!([document]);
                groups.push(group);
                listed.push(std::collections::HashSet::from([document]));
            }
        }
    }
    json!(groups)
}

/// The program as built: its version, path, size and modification time, so
/// that what a build of other code saved in a cache is not reused.
pub fn program() -> Result<String> {
    let path = std::env::current_exe()?;
    let metadata = fs::metadata(&path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    Ok(format!(
        "{}\n{}\n{}\n{modified}",
        env!("CARGO_PKG_VERSION"),
        path.display(),
        metadata.len()
    ))
}

/// The results of `work` on each of `items`, in their order, done on up to
/// four threads. A panic in `work` panics here.
pub fn in_order<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4)
        .min(items.len());
    if workers < 2 {
        return items.iter().map(work).collect();
    }
    let work = &work;
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(items.len().div_ceil(workers))
            .map(|chunk| scope.spawn(move || chunk.iter().map(work).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// Removes the files in `directory` beyond the `kept` most recently written,
/// as a cache keeps its newest entries. Best effort: a file that cannot be
/// read or removed stays.
pub fn keep_newest(directory: &Path, kept: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut saved: Vec<_> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    if saved.len() > kept {
        saved.sort();
        for (_, path) in &saved[..saved.len() - kept] {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_encoding_is_compared_as_it_is_made() {
        let value = json!({"a":[{"b":1,"c":"x"},{"b":2,"c":"y\r\nz"}],"d":{"e":[]}});
        let lazy = || Lazy::Borrowed(&value);
        let encoded = encoded_lazy_lines(lazy(), &["a"]).unwrap();
        assert!(is_encoded_lazy_lines(lazy(), &["a"], &encoded).unwrap());
        // As text.replace("\r\n", "\n") compared it: line breaks written as
        // CRLF read as LF, and a CR the encoding escapes stays apart.
        let crlf = String::from_utf8(encoded.clone())
            .unwrap()
            .replace('\n', "\r\n");
        assert!(is_encoded_lazy_lines(lazy(), &["a"], crlf.as_bytes()).unwrap());
        for other in [
            &encoded[..encoded.len() - 1],
            &[encoded.as_slice(), b" "].concat(),
        ] {
            assert!(!is_encoded_lazy_lines(lazy(), &["a"], other).unwrap());
        }
        let mut changed = encoded.clone();
        changed[3] = b'#';
        assert!(!is_encoded_lazy_lines(lazy(), &["a"], &changed).unwrap());
        assert!(!is_encoded_lazy_lines(lazy(), &[], &encoded).unwrap());
    }

    #[test]
    fn written_files_hold_the_serialized_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let big: Vec<_> = (0..20_000)
            .map(|i| json!({"id":format!("c-{i}"),"value":i as f64 / 3.0,"text":"在庫 引当"}))
            .collect();
        for value in [
            json!(null),
            json!({}),
            json!({"a":[1, 2.5, 1e20, -0.00001],"b":{},"c":[],"d":"x: #y","e":"010"}),
            json!({"elements":big}),
        ] {
            for name in ["out.json", "out.yml"] {
                let path = dir.path().join(name);
                write(&path, &value).unwrap();
                assert_eq!(fs::read(&path).unwrap(), serialized(&path, &value).unwrap());
            }
        }
    }

    #[test]
    fn lazy_values_write_as_the_values_they_stand_for() {
        let dir = tempfile::tempdir().unwrap();
        let fields = json!({"z":{"a":[1, 2.5, 1e20]},"b":{},"c":[],"d":"x: #y","e":"010"});
        let items = |values: Vec<Value>| {
            Lazy::Items(Box::new(values.into_iter().map(|v| Ok(Lazy::Value(v)))))
        };
        let lazy = || {
            let mut members = Lazy::members(&fields, &["e"]);
            members.push((
                "rows",
                items(vec![json!({"cells":[]}), json!([1, {"k":null}])]),
            ));
            members.push(("empty", items(vec![])));
            members.push((
                "nested",
                Lazy::Object(vec![("inner", items(vec![json!("v")]))]),
            ));
            members.push(("none", Lazy::Object(vec![])));
            Lazy::Object(members)
        };
        let value = lazy().into_value().unwrap();
        assert_eq!(value["rows"][1][1]["k"], Value::Null);
        assert!(value.get("e").is_none());
        for name in ["out.json", "out.yml"] {
            let path = dir.path().join(name);
            write_lazy(&path, lazy()).unwrap();
            assert_eq!(fs::read(&path).unwrap(), serialized(&path, &value).unwrap());
        }
    }

    #[test]
    fn streamed_hashes_equal_hashes_of_the_encoding() {
        let big: Vec<_> = (0..20_000)
            .map(|i| {
                json!({"id":format!("c-{i}"),"value":i as f64 / 3.0,"text":"在庫
引当"})
            })
            .collect();
        for value in [
            json!(null),
            json!({}),
            json!([]),
            json!({"review":{"status":"pending"}}),
            json!({"a":[1, 2.5, 1e20, -0.00001],"b":{},"c":[],"review":1,"d":"x"}),
            json!({"elements":big,"review":{"status":"accepted"}}),
        ] {
            assert_eq!(hash_encoded(&value), hash(&encoded(&value)));
            let mut without = value.clone();
            if let Some(map) = without.as_object_mut() {
                map.remove("review");
            }
            assert_eq!(
                hash_encoded_without(&value, "review"),
                hash(&encoded(&without))
            );
        }
        for items in [
            vec![],
            vec![json!({})],
            vec![json!({"a":[1]}), json!("x"), json!([])],
        ] {
            let lazy = |items: Vec<Value>| {
                Lazy::Items(Box::new(
                    items.into_iter().map(|item| Ok(Lazy::Value(item))),
                ))
            };
            assert_eq!(
                hash_encoded_lazy(lazy(items.clone())).unwrap(),
                hash(&encoded(&Value::Array(items.clone())))
            );
            // Members in any order, an empty object and a nested lazy array.
            let object = Lazy::Object(vec![
                ("z", lazy(items.clone())),
                ("a", Lazy::Value(json!(1))),
                ("m", Lazy::Object(vec![])),
            ]);
            assert_eq!(
                hash_encoded_lazy(object).unwrap(),
                hash(&encoded(&json!({"z":items,"a":1,"m":{}})))
            );
        }
    }

    #[test]
    fn rejection_payload_is_recoverable_through_context_and_displays_compact_json() {
        let payload = json!({"code":"reply_validation_failed","diagnostics":[{"path":"/items/0"}]});
        let error = anyhow::Error::new(Rejection(payload.clone())).context("submit failed");
        assert_eq!(rejection(&error), Some(&payload));
        assert_eq!(
            format!("{:#}", error),
            format!("submit failed: {}", payload)
        );
        assert!(rejection(&anyhow::anyhow!("plain")).is_none());
    }

    #[test]
    fn warnings_group_identical_findings_and_keep_unparseable_text() {
        let r001 = r#"{"code":"R001","level":"warning","message":"未抽出"}"#;
        let grouped = grouped_warnings([
            &format!("a: {r001}"),
            &format!("b: {r001}"),
            &format!("a: {r001}"),
            "a: {\"code\":\"R002\",\"level\":\"warning\",\"message\":\"別\"}",
            "free text warning",
            "c: not json",
        ]);
        assert_eq!(
            grouped,
            json!([
                {"code":"R001","level":"warning","message":"未抽出","documents":["a","b"]},
                {"code":"R002","level":"warning","message":"別","documents":["a"]},
                {"message":"free text warning"},
                {"message":"c: not json"}
            ])
        );
        assert_eq!(grouped_warnings([]), json!([]));
    }

    #[test]
    fn value_keys_are_shared_exactly_by_equal_values() {
        let values = [
            json!(null),
            json!(true),
            json!(false),
            json!(0),
            json!(0.0),
            json!(-0.0),
            json!(-1),
            json!(1),
            json!(1.0),
            json!(u64::MAX),
            json!(""),
            json!("0"),
            json!("u0;"),
            json!([]),
            json!([[]]),
            json!(["a", "b"]),
            json!(["ab"]),
            json!({}),
            json!({"a":1,"b":[2]}),
            json!({"b":[2],"a":1}),
            json!({"a":"1"}),
            json!({"ab":null}),
            json!({"a":{"b":null}}),
        ];
        for a in &values {
            for b in &values {
                assert_eq!(a == b, value_key(a) == value_key(b), "{a} {b}");
            }
        }
    }

    #[test]
    fn grouped_warnings_match_comparing_each_group() {
        // The grouping as it was made before findings were looked up by their
        // fields: each finding compared with every group. Kept as the reference.
        fn scanned(warnings: &[&str]) -> Value {
            let mut groups: Vec<Value> = Vec::new();
            for warning in warnings {
                let parsed = warning.split_once(": ").and_then(|(document, rest)| {
                    serde_json::from_str::<Value>(rest)
                        .ok()
                        .filter(Value::is_object)
                        .map(|finding| (document, finding))
                });
                let Some((document, finding)) = parsed else {
                    groups.push(json!({"message":warning}));
                    continue;
                };
                let existing = groups.iter_mut().find(|g| {
                    g["documents"].is_array()
                        && g.as_object().unwrap().iter().all(|(k, v)| {
                            k == "documents" || finding.get(k).is_some_and(|other| other == v)
                        })
                        && finding
                            .as_object()
                            .unwrap()
                            .keys()
                            .all(|k| g.get(k).is_some())
                });
                match existing {
                    Some(group) => {
                        let documents = group["documents"].as_array_mut().unwrap();
                        if !documents.iter().any(|d| d == document) {
                            documents.push(json!(document));
                        }
                    }
                    None => {
                        let mut group = finding;
                        group["documents"] = json!([document]);
                        groups.push(group);
                    }
                }
            }
            json!(groups)
        }
        let findings = [
            r#"{"code":"R1","level":"warning"}"#,
            r#"{"level":"warning","code":"R1"}"#,
            r#"{"code":"R1"}"#,
            r#"{"code":"R1","level":"warning","documents":["x"]}"#,
            r#"{"code":"R1","level":"warning","documents":"y"}"#,
            r#"{"code":"R2","value":0.0}"#,
            r#"{"code":"R2","value":-0.0}"#,
            r#"{"code":"R2","value":0}"#,
            r#"{"message":"m"}"#,
            r#"["not","an","object"]"#,
            "not json",
        ];
        let mut warnings = Vec::new();
        for (i, finding) in findings.iter().enumerate() {
            for document in ["a", "b", "a: c"] {
                warnings.push(format!("{document}: {finding}"));
                if i % 3 == 0 {
                    warnings.push(format!(
                        "{document}: {}",
                        findings[(i * 7 + 3) % findings.len()]
                    ));
                }
            }
        }
        warnings.push("plain".into());
        let warnings: Vec<&str> = warnings.iter().map(String::as_str).collect();
        for start in 0..warnings.len() {
            let rotated: Vec<&str> = warnings[start..]
                .iter()
                .chain(&warnings[..start])
                .copied()
                .collect();
            assert_eq!(grouped_warnings(rotated.iter().copied()), scanned(&rotated));
        }
    }

    #[test]
    fn empty_directories_are_removed_up_to_the_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let boundary = temp.path().join(".arp");
        let kept = boundary.join("changes/kept");
        let emptied = boundary.join("changes/doc/proposal");
        fs::create_dir_all(&kept).unwrap();
        fs::write(kept.join("proposal.json"), "{}").unwrap();
        fs::create_dir_all(&emptied).unwrap();

        remove_empty_directories(&emptied, &boundary);
        assert!(!boundary.join("changes/doc").exists());
        assert!(kept.exists());

        fs::remove_dir_all(&kept).unwrap();
        remove_empty_directories(&kept, &boundary);
        assert!(!boundary.join("changes").exists());
        assert!(boundary.exists());

        let outside = temp.path().join("sources");
        fs::create_dir(&outside).unwrap();
        remove_empty_directories(&outside, &boundary);
        assert!(outside.exists());
    }

    #[test]
    fn stage_removes_the_parents_it_leaves_empty_unless_kept() {
        let temp = tempfile::tempdir().unwrap();
        let boundary = temp.path().join(".arp");
        let parent = boundary.join("cache/export");
        let stage = Stage::new_in(&parent, &boundary).unwrap();
        fs::write(stage.path().join("result.xlsx"), "x").unwrap();
        drop(stage);
        assert!(!boundary.join("cache").exists());
        assert!(boundary.exists());

        let kept = Stage::new_in(&parent, &boundary).unwrap().keep();
        assert!(kept.exists());
        fs::remove_dir(&kept).unwrap();
    }

    #[test]
    fn yaml_written_by_arp_reads_back_beyond_parser_default_budgets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.yml");
        let value = json!({"cells":(0..=1_000_000).collect::<Vec<_>>(),"text":"x".repeat(65 * 1024 * 1024)});
        write(&path, &value).unwrap();
        assert!(read(&path, None).unwrap() == value);
    }

    #[test]
    fn yaml_rejects_anchors_and_aliases() {
        assert!(parse("a: &x 1\nb: *x\n", false).is_err());
    }

    #[test]
    fn yaml_rejects_tags_merge_keys_and_deep_nesting() {
        for text in [
            "a: !!binary aGk=\n",
            "a: !!str 1\n",
            "<<: {a: 1}\n",
            "a: 1\n---\nb: 2\n",
        ] {
            assert!(parse(text, false).is_err(), "{text:?}");
        }
        assert_eq!(parse("'<<': 1\n", false).unwrap(), json!({"<<":1}));
        let nested = |depth: usize| format!("{}1{}\n", "[".repeat(depth), "]".repeat(depth));
        assert!(parse(&nested(64), false).is_ok());
        assert!(parse(&nested(65), false).is_err());
    }

    #[test]
    fn hand_typed_yaml_numbers_follow_yaml_1_2() {
        assert_eq!(
            parse(
                "a: 0x1F\nb: -1_000\nc: 1e3\nd: '1e3'\ne: 1.2.3\nf: inf\ng: TRUE\nh: 18446744073709551616\n",
                false
            )
            .unwrap(),
            json!({"a":31,"b":-1000,"c":1000.0,"d":"1e3","e":"1.2.3","f":"inf","g":true,"h":18446744073709551616.0})
        );
        assert!(parse("a: .inf\n", false).is_err());
    }

    #[test]
    fn hand_typed_yaml_keeps_decimal_numbers_and_yaml_1_2_booleans() {
        assert_eq!(
            parse("a: 010\nb: yes\nN: x\nc: true\nd: 12\n", false).unwrap(),
            json!({"a":10.0,"b":"yes","N":"x","c":true,"d":12})
        );
    }

    #[test]
    fn hand_typed_yaml_refuses_values_that_differ_from_their_text() {
        for text in [
            "A: Issue #12 対応\n",
            "A: 1 # note\n",
            "A: 行1\n  行2\n",
            "A:\n",
            "A: ~\n",
            "A: NULL\n",
            "A: Null\n",
        ] {
            assert!(parse(text, false).is_err(), "{text:?}");
        }
        assert_eq!(
            parse(
                "# header\nA: null\nB: \"Issue #12\"\nC: 'NULL'\nD: |-\n  行1\n  行2\n",
                false
            )
            .unwrap(),
            json!({"A":null,"B":"Issue #12","C":"NULL","D":"行1\n行2"})
        );
    }

    #[test]
    fn yaml_written_by_arp_passes_the_hand_edit_checks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("values.yml");
        let value = json!({"null":null,"empty":"","tilde":"~","upper":"NULL","hash":"a #b",
            "lines":"a\nb ","long":"x ".repeat(80),"octal":"010","bool":"yes","N":"N","n":1.5,
            "ideographic":"\u{3000}1","trailing":"1\u{3000}","nbsp":"\u{a0}1","em":"1\u{2003}",
            "spaced_bool":"\u{3000}true","spaced_null":"null\u{3000}"});
        write(&path, &value).unwrap();
        assert_eq!(read(&path, None).unwrap(), value);
    }

    #[test]
    fn hand_typed_yaml_keeps_unicode_spaces_around_numbers_as_text() {
        assert_eq!(
            parse("a: \u{3000}1\nb: 2\u{a0}\nc: true\u{3000}\n", false).unwrap(),
            json!({"a":"\u{3000}1","b":"2\u{a0}","c":"true\u{3000}"})
        );
    }

    #[test]
    fn splice_applies_edits_in_order_and_rejects_overlaps() {
        let edits = vec![
            (6..11, "there".to_owned()),
            (0..0, "[".to_owned()),
            (5..5, ",".to_owned()),
            (11..11, "]".to_owned()),
            (0..0, "<".to_owned()),
        ];
        assert_eq!(
            splice("hello world", edits, "test").unwrap(),
            "[<hello, there]"
        );
        let overlapping = vec![(0..3, String::new()), (2..4, String::new())];
        assert!(splice("abcd", overlapping, "test").is_err());
    }

    /// Renders each nested value to its own string, as the canonical form was defined.
    fn nested_render(v: &Value, depth: usize) -> String {
        let indent = "  ".repeat(depth + 1);
        match v {
            Value::Object(map) if !map.is_empty() => format!(
                "{{\n{}\n{}}}",
                map.iter()
                    .map(|(k, v)| format!(
                        "{indent}{}: {}",
                        serde_json::to_string(k).unwrap(),
                        nested_render(v, depth + 1)
                    ))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                "  ".repeat(depth)
            ),
            Value::Array(items) if !items.is_empty() => format!(
                "[\n{}\n{}]",
                items
                    .iter()
                    .map(|v| format!("{indent}{}", nested_render(v, depth + 1)))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                "  ".repeat(depth)
            ),
            Value::Number(n) if n.is_f64() => {
                let f = n.as_f64().unwrap();
                if f != 0.0 && (f.abs() >= 1e16 || f.abs() < 1e-4) {
                    let repr = format!("{f:e}");
                    let (mantissa, exponent) = repr.split_once('e').unwrap();
                    let exp: i32 = exponent.parse().unwrap();
                    format!("{mantissa}e{exp:+03}")
                } else {
                    let s = f.to_string();
                    if s.contains('.') { s } else { format!("{s}.0") }
                }
            }
            _ => serde_json::to_string(v).unwrap(),
        }
    }

    #[test]
    fn encoded_matches_the_nested_canonical_form() {
        let value = json!({
            "empty": {"object": {}, "array": []},
            "numbers": [0, -3, 1.5, 2.0, 1e-7, 3.2e20, -0.0001, 12345678901234567890u64],
            "text": ["", "語\n\"quoted\"", "\u{1f600}", null, true],
            "nested": [[{"a": [1, {"b": []}]}], {"c": {"d": {"e": [null]}}}],
        });
        for sample in [
            value.clone(),
            json!([]),
            json!({}),
            json!(1.0),
            json!("x"),
            json!([value]),
        ] {
            assert_eq!(
                String::from_utf8(encoded(&sample)).unwrap(),
                format!("{}\n", nested_render(&sample, 0))
            );
        }
    }

    #[test]
    fn encoded_lines_keeps_each_item_of_the_named_member_on_one_line() {
        let value = json!({
            "sheets": [{
                "cells": [
                    {"address": "A1", "value": 1.0, "runs": [{"text": "a"}, {"text": "b"}]},
                    {"address": "B1", "value": "語\n\"x\"", "style": {}, "tags": []},
                ],
                "merges": ["A1:B1"],
            }],
            "cells": [],
        });
        let text = String::from_utf8(encoded_lines(&value, &["cells"])).unwrap();
        let expected = r#"{
  "cells": [],
  "sheets": [
    {
      "cells": [
        {"address": "A1", "runs": [{"text": "a"}, {"text": "b"}], "value": 1.0},
        {"address": "B1", "style": {}, "tags": [], "value": "語\n\"x\""}
      ],
      "merges": [
        "A1:B1"
      ]
    }
  ]
}
"#;
        assert_eq!(text, expected);
        assert_eq!(parse(&text, true).unwrap(), value);
        assert_eq!(
            hash_encoded_lines(&value, &["cells"]),
            hash(text.as_bytes())
        );
        assert_eq!(encoded_lines(&value, &[]), encoded(&value));
    }
}
