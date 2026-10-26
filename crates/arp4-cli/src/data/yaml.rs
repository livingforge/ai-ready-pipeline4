//! Reads YAML in one pass over the parser's events, building the JSON value directly.
//!
//! Scalars resolve as serde-saphyr resolves typeless values with `strict_booleans` and
//! decimal `010`, so everything `serde_saphyr::to_string` writes reads back
//! unchanged. Hand-typed YAML whose value would silently differ from its text is refused:
//! a comment after a value (`Issue #12` reads as `Issue`), a plain value folded across
//! lines, and null spelled other than `null`. Anchors, aliases, tags and merge keys are
//! refused too; ARP writes none of them.
use anyhow::{Result, bail, ensure};
use serde_json::{Map, Number, Value};
use serde_saphyr::granit_parser::{Event, Parser, Placement, ScalarStyle, Span, StrInput};
use std::borrow::Cow;

/// Nested collections allowed below the document, as serde-saphyr's default budget.
const MAX_DEPTH: usize = 64;

pub(super) fn parse(text: &str) -> Result<Value> {
    let mut events = Events(Parser::new_from_str(text));
    ensure!(
        matches!(events.next()?.0, Event::StreamStart),
        "YAML stream must start"
    );
    let value = match events.next()? {
        (Event::StreamEnd, _) => return Ok(Value::Null),
        (Event::DocumentStart(..), _) => match events.next()? {
            // An empty document is null, as for a typeless serde target.
            (Event::DocumentEnd, _) => Value::Null,
            (event, span) => {
                let value = events.node(event, span, 0)?;
                ensure!(
                    matches!(events.next()?.0, Event::DocumentEnd),
                    "YAML document must end after its value"
                );
                value
            }
        },
        (_, span) => bail!("YAML line {}: expected a document", span.start.line()),
    };
    match events.next()? {
        (Event::StreamEnd, _) => Ok(value),
        (_, span) => bail!(
            "YAML line {}: only one document is allowed",
            span.start.line()
        ),
    }
}

/// The YAML document `text`, a mapping whose member `list` is a list of
/// mappings of one member each, as its other members and the key and value of
/// each item of `list`: read as [`parse`] reads it, without an object for
/// each item, as a page of a Word document has one for each paragraph. None
/// for a document of any other shape or that [`parse`] refuses, which it then
/// reads whole to say why.
pub(super) fn parse_listing(text: &str, list: &str) -> Option<(Map<String, Value>, ListItems)> {
    let mut events = Events(Parser::new_from_str(text));
    let mut next = || events.next().ok();
    matches!(next()?.0, Event::StreamStart).then_some(())?;
    matches!(next()?.0, Event::DocumentStart(..)).then_some(())?;
    let (Event::MappingStart(_, 0, None), _) = next()? else {
        return None;
    };
    let mut fields = Map::new();
    let mut items = None;
    loop {
        let key = match events.next().ok()? {
            (Event::MappingEnd, _) => break,
            (Event::Scalar(key, style, 0, None), span)
                if !(style == ScalarStyle::Plain && key == "<<") =>
            {
                match scalar(key, style, span).ok()? {
                    Value::String(key) => key,
                    _ => return None,
                }
            }
            _ => return None,
        };
        if key == list {
            items.is_none().then_some(())?;
            items = Some(events.items()?);
            continue;
        }
        let (event, span) = events.next().ok()?;
        let value = events.node(event, span, 1).ok()?;
        fields.insert(key, value).is_none().then_some(())?;
    }
    matches!(events.next().ok()?.0, Event::DocumentEnd).then_some(())?;
    matches!(events.next().ok()?.0, Event::StreamEnd).then_some(())?;
    Some((fields, items?))
}

/// The key and value of each item of a list [`parse_listing`] reads.
pub(super) type ListItems = Vec<(String, Value)>;

struct Events<'a>(Parser<'a, StrInput<'a>>);

impl Events<'_> {
    /// A list of mappings of one member each, as [`parse_listing`] reads it.
    fn items(&mut self) -> Option<ListItems> {
        let (Event::SequenceStart(_, 0, None), _) = self.next().ok()? else {
            return None;
        };
        let mut items = vec![];
        loop {
            match self.next().ok()? {
                (Event::SequenceEnd, _) => return Some(items),
                (Event::MappingStart(_, 0, None), _) => {}
                _ => return None,
            }
            let key = match self.next().ok()? {
                (Event::Scalar(key, style, 0, None), span)
                    if !(style == ScalarStyle::Plain && key == "<<") =>
                {
                    match scalar(key, style, span).ok()? {
                        Value::String(key) => key,
                        _ => return None,
                    }
                }
                _ => return None,
            };
            let (event, span) = self.next().ok()?;
            let value = self.node(event, span, 3).ok()?;
            let (Event::MappingEnd, _) = self.next().ok()? else {
                return None;
            };
            items.push((key, value));
        }
    }
}

impl<'a> Events<'a> {
    /// The next data event. Comments are skipped, except one after a value on its line.
    fn next(&mut self) -> Result<(Event<'a>, Span)> {
        loop {
            match self.0.next() {
                None => bail!("YAML ended unexpectedly"),
                Some(Err(error)) => bail!("YAML: {error}"),
                Some(Ok((Event::Comment(_, Placement::Right), span))) => bail!(
                    "YAML line {}: a comment after a value is not allowed; quote text containing ' #'",
                    span.start.line()
                ),
                Some(Ok((Event::Comment(..), _))) => {}
                Some(Ok(next)) => return Ok(next),
            }
        }
    }

    fn node(&mut self, event: Event<'a>, span: Span, depth: usize) -> Result<Value> {
        let line = span.start.line();
        match event {
            Event::Scalar(value, style, anchor, tag) => {
                plain_node(anchor, tag.is_none(), line)?;
                scalar(value, style, span)
            }
            Event::SequenceStart(_, anchor, tag) => {
                plain_node(anchor, tag.is_none(), line)?;
                ensure!(depth < MAX_DEPTH, "YAML line {line}: nesting is too deep");
                let mut items = vec![];
                loop {
                    match self.next()? {
                        (Event::SequenceEnd, _) => return Ok(Value::Array(items)),
                        (event, span) => items.push(self.node(event, span, depth + 1)?),
                    }
                }
            }
            Event::MappingStart(_, anchor, tag) => {
                plain_node(anchor, tag.is_none(), line)?;
                ensure!(depth < MAX_DEPTH, "YAML line {line}: nesting is too deep");
                let mut map = Map::new();
                loop {
                    let key = match self.next()? {
                        (Event::MappingEnd, _) => return Ok(Value::Object(map)),
                        (Event::Scalar(key, style, anchor, tag), span) => {
                            let line = span.start.line();
                            plain_node(anchor, tag.is_none(), line)?;
                            ensure!(
                                !(style == ScalarStyle::Plain && key == "<<"),
                                "YAML line {line}: merge keys are not allowed"
                            );
                            match scalar(key, style, span)? {
                                Value::String(key) => (key, line),
                                _ => bail!("YAML line {line}: keys must be strings"),
                            }
                        }
                        (_, span) => {
                            bail!("YAML line {}: keys must be strings", span.start.line())
                        }
                    };
                    let (event, span) = self.next()?;
                    let value = self.node(event, span, depth + 1)?;
                    let (key, line) = key;
                    ensure!(
                        map.insert(key, value).is_none(),
                        "YAML line {line}: duplicate key"
                    );
                }
            }
            Event::Alias(_) => bail!("YAML line {line}: aliases are not allowed"),
            _ => bail!("YAML line {line}: unexpected YAML event"),
        }
    }
}

/// The start of a node [`Reader`] reads: a scalar's value, or the start of a
/// mapping or a sequence.
pub(crate) enum Node {
    Scalar(Value),
    Mapping,
    Sequence,
}

/// A YAML document read a node at a time, as [`parse`] reads it: for a reader
/// that takes a large node apart as it is read rather than have its value
/// made whole, as a table of a large sheet is several times its values.
pub(crate) struct Reader<'a>(Events<'a>);

impl<'a> Reader<'a> {
    /// The reader of `text`, at the start of the node of its document.
    pub(crate) fn new(text: &'a str) -> Result<Self> {
        let mut events = Events(Parser::new_from_str(text));
        ensure!(
            matches!(events.next()?.0, Event::StreamStart),
            "YAML stream must start"
        );
        ensure!(
            matches!(events.next()?.0, Event::DocumentStart(..)),
            "expected a YAML document"
        );
        Ok(Self(events))
    }

    /// The start of the next node, one at `depth`.
    pub(crate) fn node(&mut self, depth: usize) -> Result<Node> {
        let (event, span) = self.0.next()?;
        Self::start(event, span, depth)
    }

    fn start(event: Event<'a>, span: Span, depth: usize) -> Result<Node> {
        let line = span.start.line();
        let (node, anchor, tag) = match event {
            Event::Scalar(value, style, anchor, tag) => {
                plain_node(anchor, tag.is_none(), line)?;
                return Ok(Node::Scalar(scalar(value, style, span)?));
            }
            Event::SequenceStart(_, anchor, tag) => (Node::Sequence, anchor, tag),
            Event::MappingStart(_, anchor, tag) => (Node::Mapping, anchor, tag),
            Event::Alias(_) => bail!("YAML line {line}: aliases are not allowed"),
            _ => bail!("YAML line {line}: unexpected YAML event"),
        };
        plain_node(anchor, tag.is_none(), line)?;
        ensure!(depth < MAX_DEPTH, "YAML line {line}: nesting is too deep");
        Ok(node)
    }

    /// The next key of the mapping being read, none at its end.
    pub(crate) fn key(&mut self) -> Result<Option<Cow<'a, str>>> {
        match self.0.next()? {
            (Event::MappingEnd, _) => Ok(None),
            (Event::Scalar(key, style, anchor, tag), span) => {
                let line = span.start.line();
                plain_node(anchor, tag.is_none(), line)?;
                ensure!(
                    !(style == ScalarStyle::Plain && key == "<<"),
                    "YAML line {line}: merge keys are not allowed"
                );
                ensure!(
                    style != ScalarStyle::Plain || plain(&key, span)?.is_none(),
                    "YAML line {line}: keys must be strings"
                );
                Ok(Some(key))
            }
            (_, span) => bail!("YAML line {}: keys must be strings", span.start.line()),
        }
    }

    /// The start of the next item of the sequence being read, one at
    /// `depth`, none at its end.
    pub(crate) fn item(&mut self, depth: usize) -> Result<Option<Node>> {
        match self.0.next()? {
            (Event::SequenceEnd, _) => Ok(None),
            (event, span) => Self::start(event, span, depth).map(Some),
        }
    }

    /// The value of the node at `depth` that `node` starts.
    pub(crate) fn value(&mut self, node: Node, depth: usize) -> Result<Value> {
        Ok(match node {
            Node::Scalar(value) => value,
            Node::Sequence => {
                let mut items = vec![];
                while let Some(item) = self.item(depth + 1)? {
                    items.push(self.value(item, depth + 1)?);
                }
                Value::Array(items)
            }
            Node::Mapping => {
                let mut map = Map::new();
                while let Some(key) = self.key()? {
                    let node = self.node(depth + 1)?;
                    let value = self.value(node, depth + 1)?;
                    ensure!(
                        map.insert(key.into_owned(), value).is_none(),
                        "YAML: duplicate key"
                    );
                }
                Value::Object(map)
            }
        })
    }

    /// Reads the end of the document, which ends the stream.
    pub(crate) fn end(mut self) -> Result<()> {
        ensure!(
            matches!(self.0.next()?.0, Event::DocumentEnd),
            "YAML document must end after its value"
        );
        ensure!(
            matches!(self.0.next()?.0, Event::StreamEnd),
            "only one YAML document is allowed"
        );
        Ok(())
    }
}

fn plain_node(anchor: usize, untagged: bool, line: usize) -> Result<()> {
    ensure!(anchor == 0, "YAML line {line}: anchors are not allowed");
    ensure!(
        untagged,
        "YAML line {line}: tags are not allowed; quote text instead"
    );
    Ok(())
}

fn scalar(value: Cow<'_, str>, style: ScalarStyle, span: Span) -> Result<Value> {
    if style != ScalarStyle::Plain {
        return Ok(Value::String(value.into_owned()));
    }
    Ok(match plain(&value, span)? {
        Some(value) => value,
        None => Value::String(value.into_owned()),
    })
}

/// The value of the plain scalar `text` when it is no string, none for text.
fn plain(text: &str, span: Span) -> Result<Option<Value>> {
    let line = span.start.line();
    ensure!(
        span.end.line() == line,
        "YAML line {line}: an unquoted value continues on the next line; quote it or use a block scalar"
    );
    ensure!(
        !matches!(text, "" | "~" | "Null" | "NULL"),
        "YAML line {line}: write null to clear a value, or quote the text"
    );
    // The scanner already strips YAML's own blanks (space, tab). Other Unicode
    // spaces belong to the text: `　1` is the string serde-saphyr wrote, not 1.
    if text.eq_ignore_ascii_case("null") {
        return Ok(Some(Value::Null));
    }
    if text.eq_ignore_ascii_case("true") {
        return Ok(Some(Value::Bool(true)));
    }
    if text.eq_ignore_ascii_case("false") {
        return Ok(Some(Value::Bool(false)));
    }
    if let Some(number) = integer(text) {
        return Ok(Some(Value::Number(number)));
    }
    if let Some(float) = float(text) {
        return match Number::from_f64(float) {
            Some(number) => Ok(Some(Value::Number(number))),
            None => bail!("YAML line {line}: non-finite number"),
        };
    }
    Ok(None)
}

/// A YAML 1.2 integer that fits `i64` (negative) or `u64`: decimal without a redundant
/// leading zero, or `0x`/`0o`/`0b`, with single underscores between decimal digits.
fn integer(text: &str) -> Option<Number> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let magnitude = if let Some(rest) = strip_radix(rest, 'x') {
        digits(rest, 16)?
    } else if let Some(rest) = strip_radix(rest, 'o') {
        digits(rest, 8)?
    } else if let Some(rest) = strip_radix(rest, 'b') {
        digits(rest, 2)?
    } else {
        if rest.starts_with('0') && rest != "0" {
            return None;
        }
        digits(rest, 10)?
    };
    if negative {
        let value = 0i128.checked_sub(i128::try_from(magnitude).ok()?)?;
        Some(Number::from(i64::try_from(value).ok()?))
    } else {
        Some(Number::from(u64::try_from(magnitude).ok()?))
    }
}

fn strip_radix(text: &str, radix: char) -> Option<&str> {
    text.strip_prefix('0')?
        .strip_prefix([radix, radix.to_ascii_uppercase()])
}

/// Digits of `radix`; an underscore may only stand between two digits.
fn digits(text: &str, radix: u32) -> Option<u128> {
    let bytes = text.as_bytes();
    let mut value: u128 = 0;
    let mut seen = false;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            if i == 0 || bytes[i - 1] == b'_' || bytes.get(i + 1).is_none_or(|&n| n == b'_') {
                return None;
            }
            continue;
        }
        let digit = char::from(b).to_digit(radix)?;
        value = value
            .checked_mul(u128::from(radix))?
            .checked_add(u128::from(digit))?;
        seen = true;
    }
    seen.then_some(value)
}

/// A float as serde-saphyr reads one: `.inf`/`.nan` spellings, then Rust's parser for
/// finite values, and numerals that overflow to infinity. Non-finite results are refused
/// by the caller; alphabetic `inf`/`nan` stay text.
fn float(text: &str) -> Option<f64> {
    // Compared in place: a long text is not copied to lower case.
    let spelled = |spellings: &[&str]| spellings.iter().any(|s| text.eq_ignore_ascii_case(s));
    if spelled(&[".nan", "+.nan", "-.nan"]) {
        return Some(f64::NAN);
    }
    if spelled(&[".inf", "+.inf"]) {
        return Some(f64::INFINITY);
    }
    if spelled(&["-.inf"]) {
        return Some(f64::NEG_INFINITY);
    }
    let value = text.parse::<f64>().ok()?;
    let numeral = text
        .strip_prefix(['+', '-'])
        .unwrap_or(text)
        .starts_with(|c: char| c.is_ascii_digit());
    (value.is_finite() || numeral).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`parse_listing`] of `text`, given back as the value [`parse`] reads.
    fn listed(text: &str) -> Option<Value> {
        let (mut fields, items) = parse_listing(text, "elements")?;
        let items = items
            .into_iter()
            .map(|(key, value)| Value::Object([(key, value)].into_iter().collect()))
            .collect();
        fields.insert("elements".into(), Value::Array(items));
        Some(Value::Object(fields))
    }

    /// A document read a node at a time reads as [`parse`] reads it, and is
    /// refused where it is.
    #[test]
    fn a_reader_reads_as_the_document_does() {
        let read = |text: &str| -> Result<Value> {
            let mut reader = Reader::new(text)?;
            let node = reader.node(0)?;
            let value = reader.value(node, 0)?;
            reader.end()?;
            Ok(value)
        };
        let text = "elements:\n- text: 段落\n- table:\n    columns:\n    - 項目\n    rows:\n    - 項目: 1\n      children:\n      - 項目: '010'\n- blank: null\npage_id: sheet-1\nschema_version: '6'\n";
        assert_eq!(read(text).unwrap(), parse(text).unwrap());
        for text in [
            "a: 1 # note\n",
            "a: 1\na: 2\n",
            "1: a\n",
            "a: &x 1\n",
            "a: !tag 1\n",
            "a: 1\n---\nb: 2\n",
        ] {
            assert!(parse(text).is_err(), "{text}");
            assert!(read(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_listing_reads_as_the_document_does() {
        let text = "elements:\n- text: 段落\n- table:\n    columns:\n    - 項目\n    rows:\n    - 項目: 1\n- blank: null\npage_id: sheet-1\nschema_version: '6'\ntitle: S\n";
        assert_eq!(listed(text).unwrap(), parse(text).unwrap());
        assert_eq!(
            listed("page_id: p\nelements: []\n").unwrap(),
            parse("page_id: p\nelements: []\n").unwrap()
        );
        for text in [
            // What parse refuses, and documents of another shape.
            "elements:\n- text: a # note\n",
            "elements:\n- text: a\npage_id: p\npage_id: q\n",
            "elements:\n- text: a\n  formula: b\n",
            "elements:\n- a\n",
            "page_id: p\n",
            "elements: {}\n",
            "- elements: []\n",
            "elements:\n- &a text: a\n",
        ] {
            assert!(listed(text).is_none(), "{text}");
        }
    }
}
