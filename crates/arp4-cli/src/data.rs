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

// Canonical JSON encoding preserves exponent signs and float representation for stable hashes.
/// Written into one buffer: rendering each nested value to its own string would
/// copy every byte once per enclosing level of large inputs and extractions.
pub fn encoded(value: &Value) -> Vec<u8> {
    fn render(v: &Value, depth: usize, out: &mut String) {
        let indent = |out: &mut String, depth: usize| {
            for _ in 0..depth {
                out.push_str("  ");
            }
        };
        match v {
            Value::Object(map) if !map.is_empty() => {
                out.push_str("{\n");
                for (index, (k, v)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    indent(out, depth + 1);
                    out.push_str(&serde_json::to_string(k).unwrap());
                    out.push_str(": ");
                    render(v, depth + 1, out);
                }
                out.push('\n');
                indent(out, depth);
                out.push('}');
            }
            Value::Array(items) if !items.is_empty() => {
                out.push_str("[\n");
                for (index, v) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(",\n");
                    }
                    indent(out, depth + 1);
                    render(v, depth + 1, out);
                }
                out.push('\n');
                indent(out, depth);
                out.push(']');
            }
            Value::Number(n) if n.is_f64() => {
                let f = n.as_f64().unwrap();
                if f != 0.0 && (f.abs() >= 1e16 || f.abs() < 1e-4) {
                    let repr = format!("{f:e}");
                    let (mantissa, exponent) = repr.split_once('e').unwrap();
                    let exp: i32 = exponent.parse().unwrap();
                    out.push_str(&format!("{mantissa}e{exp:+03}"));
                } else {
                    let s = f.to_string();
                    out.push_str(&s);
                    if !s.contains('.') {
                        out.push_str(".0");
                    }
                }
            }
            _ => out.push_str(&serde_json::to_string(v).unwrap()),
        }
    }
    let mut out = String::new();
    render(value, 0, &mut out);
    out.push('\n');
    out.into_bytes()
}

/// JSON with unique keys and finite numbers. Each node is moved into its parent:
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
    let validator = std::sync::Arc::new(jsonschema::validator_for(&crate::schemas()[name])?);
    validators.insert(name.to_owned(), validator.clone());
    Ok(validator)
}
pub fn validate(name: &str, value: &Value) -> Result<()> {
    if let Err(error) = validator(name)?.validate(value) {
        bail!("{name}: {error}")
    };
    if name == "mappings" {
        crate::document_structure::validate_corrections(&value["interpretation"])?;
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
pub fn write(path: &Path, value: &Value) -> Result<()> {
    let bytes = if path.extension().is_some_and(|s| s == "json") {
        encoded(value)
    } else {
        serde_saphyr::to_string(value)?.into_bytes()
    };
    replace(path, &bytes)
}
pub fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("missing parent")?)?;
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
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
            let p = e?.path();
            let meta = fs::symlink_metadata(&p)?;
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

/// Group `"<document>: {finding JSON}"` warnings so a finding repeated across
/// documents is shown once with its document list. Unparseable entries keep their text.
pub fn grouped_warnings<'a>(warnings: impl IntoIterator<Item = &'a str>) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "lines":"a\nb ","long":"x ".repeat(80),"octal":"010","bool":"yes","N":"N","n":1.5});
        write(&path, &value).unwrap();
        assert_eq!(read(&path, None).unwrap(), value);
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
}
