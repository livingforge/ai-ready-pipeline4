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

struct Events<'a>(Parser<'a, StrInput<'a>>);

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
    let line = span.start.line();
    ensure!(
        span.end.line() == line,
        "YAML line {line}: an unquoted value continues on the next line; quote it or use a block scalar"
    );
    ensure!(
        !matches!(value.as_ref(), "" | "~" | "Null" | "NULL"),
        "YAML line {line}: write null to clear a value, or quote the text"
    );
    let text = value.trim();
    if text.eq_ignore_ascii_case("null") {
        return Ok(Value::Null);
    }
    if text.eq_ignore_ascii_case("true") {
        return Ok(Value::Bool(true));
    }
    if text.eq_ignore_ascii_case("false") {
        return Ok(Value::Bool(false));
    }
    if let Some(number) = integer(text) {
        return Ok(Value::Number(number));
    }
    if let Some(float) = float(text) {
        return match Number::from_f64(float) {
            Some(number) => Ok(Value::Number(number)),
            None => bail!("YAML line {line}: non-finite number"),
        };
    }
    Ok(Value::String(value.into_owned()))
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
    match text.to_ascii_lowercase().as_str() {
        ".nan" | "+.nan" | "-.nan" => return Some(f64::NAN),
        ".inf" | "+.inf" => return Some(f64::INFINITY),
        "-.inf" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let value = text.parse::<f64>().ok()?;
    let numeral = text
        .strip_prefix(['+', '-'])
        .unwrap_or(text)
        .starts_with(|c: char| c.is_ascii_digit());
    (value.is_finite() || numeral).then_some(value)
}
