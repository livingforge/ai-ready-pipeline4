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

// Match Python contracts.encoded, including exponent signs and float representation.
pub fn encoded(value: &Value) -> Vec<u8> {
    fn render(v: &Value, depth: usize) -> String {
        let indent = "  ".repeat(depth + 1);
        match v {
            Value::Object(map) if !map.is_empty() => format!(
                "{{\n{}\n{}}}",
                map.iter()
                    .map(|(k, v)| format!(
                        "{indent}{}: {}",
                        serde_json::to_string(k).unwrap(),
                        render(v, depth + 1)
                    ))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                "  ".repeat(depth)
            ),
            Value::Array(items) if !items.is_empty() => format!(
                "[\n{}\n{}]",
                items
                    .iter()
                    .map(|v| format!("{indent}{}", render(v, depth + 1)))
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
    format!("{}\n", render(value, 0)).into_bytes()
}

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
                Ok(Strict(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                if !v.is_finite() {
                    return Err(E::custom("non-finite number"));
                }
                Ok(Strict(json!(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut list = vec![];
                while let Some(Strict(v)) = seq.next_element()? {
                    list.push(v)
                }
                Ok(Strict(json!(list)))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(Strict(key)) = map.next_key()? {
                    let key = key
                        .as_str()
                        .ok_or_else(|| de::Error::custom("keys must be strings"))?
                        .to_owned();
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    let Strict(v) = map.next_value()?;
                    values.insert(key, v);
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
        let options = serde_saphyr::options! { budget: serde_saphyr::budget! {max_anchors: 0}, alias_limits: serde_saphyr::alias_limits! {max_alias_expansions_per_anchor: 0}, no_schema: true, legacy_octal_numbers: true };
        Ok(serde_saphyr::from_str_with_options::<Strict>(text, options)?.0)
    }
}
pub fn validate(name: &str, value: &Value) -> Result<()> {
    let schemas = crate::schemas();
    let validator = jsonschema::validator_for(&schemas[name])?;
    if let Err(error) = validator.validate(value) {
        bail!("{name}: {error}")
    };
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
    ensure!(
        regex::Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9_-]*$")?.is_match(id),
        "invalid ID: {id}"
    );
    ensure!(
        !regex::Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$")?.is_match(id),
        "reserved ID: {id}"
    );
    Ok(())
}
