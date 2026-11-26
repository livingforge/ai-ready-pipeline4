//! Repository-local configuration shared by document and specification commands.
use crate::agent_format::AgentFormat;
use crate::data::{read, under, write};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn discover(start: &Path) -> Result<PathBuf> {
    let start = crate::data::canonicalize(start)?;
    for root in start.ancestors() {
        if under(root, ".arp/config.yml")?.is_file() {
            return Ok(root.to_owned());
        }
        if root.join(".git").exists() {
            break;
        }
    }
    anyhow::bail!("no .arp/config.yml found in this repository; run documents init")
}

pub fn open(root: &Path) -> Result<PathBuf> {
    let root = crate::data::canonicalize(root)?;
    let config = read(&under(&root, ".arp/config.yml")?, Some("project-config"))?;
    under(
        &root,
        config["sources"].as_str().context("sources required")?,
    )?;
    project_picture_folders(&root, &config)?;
    Exclusion::of(&config)?;
    AssetLimits::of(&config)?;
    Ok(root)
}

/// The originals below the sources folder that are not documents, by the
/// regular expressions of `exclude` in the project configuration. A pattern
/// is searched for anywhere in a path relative to the sources folder, with
/// `/` between names; a folder's path ends with `/`, and a folder that
/// matches excludes everything below it.
pub struct Exclusion(Vec<regex::Regex>);

impl Exclusion {
    /// The patterns of the project configuration `config`.
    pub fn of(config: &Value) -> Result<Self> {
        let patterns = config["exclude"].as_array().map_or(&[][..], Vec::as_slice);
        patterns
            .iter()
            .map(|pattern| {
                let pattern = pattern
                    .as_str()
                    .context("exclude pattern must be a string")?;
                regex::Regex::new(pattern).with_context(|| {
                    format!(
                        "exclude pattern {pattern} of .arp/config.yml is not a regular expression"
                    )
                })
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    /// The pattern excluding `path`, relative to the sources folder: the
    /// first that matches it or one of the folders holding it. A folder's
    /// path ends with `/`.
    pub fn pattern(&self, path: &str) -> Option<&str> {
        if self.0.is_empty() {
            return None;
        }
        let folders = path
            .match_indices('/')
            .map(|(end, _)| &path[..=end])
            .filter(|folder| folder.len() < path.len());
        folders.chain([path]).find_map(|path| {
            self.0
                .iter()
                .find(|pattern| pattern.is_match(path))
                .map(regex::Regex::as_str)
        })
    }
}

/// What import does with an original that opens only with its protection
/// removed (a password, IRM or a sensitivity label), by `protected_originals`
/// of the project configuration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedOriginals {
    /// Leaves it as it is and does not import it; `documents unprotect`
    /// removes the protection once the user approves.
    #[default]
    Skip,
    /// Removes the protection of an Office original, saves it over
    /// unprotected and imports it.
    Unprotect,
}

impl ProtectedOriginals {
    /// The setting of the project configuration `config`.
    pub fn of(config: &Value) -> Result<Self> {
        config
            .get("protected_originals")
            .map_or(Ok(Self::default()), |value| {
                serde_json::from_value(value.clone())
                    .context("protected_originals of .arp/config.yml must be skip or unprotect")
            })
    }
}

/// Which of the pictures an original shows import leaves out of its assets,
/// by `assets` of the project configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssetLimits {
    /// The most times longer than it is short a picture may be; none when
    /// no picture is left out for its shape.
    pub max_aspect_ratio: Option<f64>,
}

impl Default for AssetLimits {
    /// Pictures more than 20 times longer than they are short are rules,
    /// bands and other decoration rather than figures.
    fn default() -> Self {
        Self {
            max_aspect_ratio: Some(20.0),
        }
    }
}

impl AssetLimits {
    /// The limits of the project configuration `config`.
    pub fn of(config: &Value) -> Result<Self> {
        let mut limits = Self::default();
        if let Some(ratio) = config.get("assets").and_then(|a| a.get("max_aspect_ratio")) {
            limits.max_aspect_ratio = match ratio {
                Value::Null => None,
                ratio => Some(ratio.as_f64().filter(|r| *r >= 1.0).context(
                    "assets.max_aspect_ratio of .arp/config.yml must be a number of 1 or more, or null",
                )?),
            };
        }
        Ok(limits)
    }

    /// Whether a picture `width` by `height` is left out for its shape.
    pub fn slender(&self, width: f64, height: f64) -> bool {
        let (long, short) = (width.max(height), width.min(height));
        self.max_aspect_ratio
            .is_some_and(|most| long > most * short)
    }
}

/// The `picture_folders` of the project configuration `config`, which stay
/// inside the project at `root`.
fn project_picture_folders(root: &Path, config: &Value) -> Result<Vec<PathBuf>> {
    let folders = config["picture_folders"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    folders
        .iter()
        .map(|folder| {
            let folder = folder.as_str().context("picture folder must be a string")?;
            ensure!(
                !folder.starts_with(".arp") && !folder.starts_with(".git"),
                "picture_folders must be outside ARP/Git metadata: {folder}"
            );
            under(root, folder).with_context(|| {
                format!("picture folder {folder} of .arp/config.yml must lie inside the project; give a folder outside it in .arp/local.yml")
            })
        })
        .collect()
}

/// The folders whose picture files the HTML page at `page` may read as
/// assets: the sources folder of the project holding it, the
/// `picture_folders` of its configuration and of this machine's
/// `.arp/local.yml`, else the page's own folder. Folders that do not exist
/// are left out.
pub fn picture_folders(page: &Path) -> Result<Vec<PathBuf>> {
    let page = crate::data::canonicalize(page)?;
    let folder = page.parent().context("page has no folder")?;
    for root in folder.ancestors() {
        let path = under(root, ".arp/config.yml")?;
        if path.is_file() {
            let config = read(&path, Some("project-config"))?;
            let mut folders = vec![under(
                root,
                config["sources"].as_str().context("sources required")?,
            )?];
            folders.extend(project_picture_folders(root, &config)?);
            let local = under(root, ".arp/local.yml")?;
            if local.is_file() {
                // A project made before this file is kept from committing it.
                ensure_lines(&under(root, ".arp/.gitignore")?, &["/local.yml"])?;
                let local = read(&local, Some("project-local-config"))?;
                for folder in local["picture_folders"].as_array().into_iter().flatten() {
                    folders.push(
                        root.join(folder.as_str().context("picture folder must be a string")?),
                    );
                }
            }
            return Ok(folders
                .into_iter()
                .filter_map(|folder| crate::data::canonicalize(folder).ok())
                .collect());
        }
        if root.join(".git").exists() {
            break;
        }
    }
    Ok(vec![folder.to_owned()])
}

/// Reading views may also be produced outside a configured project. Stop at the
/// repository boundary, and never hide an invalid existing configuration.
pub fn agent_read_format(start: &Path) -> Result<AgentFormat> {
    let start = crate::data::canonicalize(start)?;
    for root in start.ancestors() {
        let path = under(root, ".arp/config.yml")?;
        if path.is_file() {
            let config = read(&path, Some("project-config"))?;
            return config
                .get("agent_read_format")
                .map(|value| serde_json::from_value(value.clone()).map_err(Into::into))
                .unwrap_or(Ok(AgentFormat::default()));
        }
        if root.join(".git").exists() {
            break;
        }
    }
    Ok(AgentFormat::default())
}

pub fn init(root: &Path, sources: &str) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let root = crate::data::canonicalize(root)?;
    let folder = under(&root, sources)?;
    ensure!(
        !sources.starts_with(".arp/") && sources != ".arp" && !sources.starts_with(".git"),
        "sources must be outside ARP/Git metadata"
    );
    // Document IDs are paths below this folder, so it exists before the first import.
    fs::create_dir_all(&folder)?;
    let config = under(&root, ".arp/config.yml")?;
    if config.exists() {
        open(&root)?;
        ensure!(
            read(&config, None)?["sources"] == sources,
            "sources already configured"
        );
    } else {
        write(&config, &initial_config(sources)?)?;
    }
    ensure_lines(
        &under(&root, ".arp/.gitignore")?,
        &["/work/", "/cache/", "/view/", "/local.yml"],
    )?;
    // Originals and records are verified byte for byte, so Git must not convert
    // their line endings. These deeper files override the repository's own settings.
    for folder in [".arp", sources] {
        ensure_lines(
            &under(&root, &format!("{folder}/.gitattributes"))?,
            &["* -text"],
        )?;
    }
    Ok(root)
}

/// The configuration `documents init` writes for the sources folder
/// `sources`: every setting of the `project-config` contract with the value
/// the contract gives it, so that the file shows what can be changed. A
/// setting added to the contract appears here without a change to this code.
pub fn initial_config(sources: &str) -> Result<Value> {
    let schema = crate::schema("project-config").context("project-config contract required")?;
    let mut config = defaults(&schema).context("project-config has no settings")?;
    config["sources"] = json!(sources);
    crate::data::validate("project-config", &config)?;
    Ok(config)
}

/// The value `schema` gives a setting the user leaves out: its `const` or
/// `default`, else for an object the defaults of its properties. None for a
/// setting that has neither.
fn defaults(schema: &Value) -> Option<Value> {
    if let Some(value) = schema.get("const").or_else(|| schema.get("default")) {
        return Some(value.clone());
    }
    let properties = schema.get("properties")?.as_object()?;
    Some(Value::Object(
        properties
            .iter()
            .filter_map(|(name, schema)| Some((name.clone(), defaults(schema)?)))
            .collect(),
    ))
}

/// Appends the lines a file lacks, keeping what the user already wrote.
pub(crate) fn ensure_lines(path: &Path, lines: &[&str]) -> Result<()> {
    let mut contents = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    for line in lines {
        if !contents.lines().any(|existing| existing == *line) {
            if !contents.is_empty() && !contents.ends_with('\n') {
                contents.push('\n');
            }
            contents.push_str(line);
            contents.push('\n');
        }
    }
    crate::data::replace(path, contents.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The settings of `schema` that [`defaults`] would leave out, by path.
    fn without_default(schema: &Value, path: &str, missing: &mut Vec<String>) {
        if schema
            .get("const")
            .or_else(|| schema.get("default"))
            .is_some()
        {
            return;
        }
        match schema.get("properties").and_then(Value::as_object) {
            Some(properties) => {
                for (name, schema) in properties {
                    without_default(schema, &format!("{path}/{name}"), missing);
                }
            }
            None => missing.push(path.to_owned()),
        }
    }

    #[test]
    fn initial_configuration_holds_every_setting_of_the_contract_with_its_default() {
        // A setting added to the contract without a default would be left
        // out of the file documents init writes.
        let schema = crate::schema("project-config").unwrap();
        let mut missing = Vec::new();
        without_default(&schema, "", &mut missing);
        assert_eq!(missing, ["/sources"], "settings need a default");
        let config = initial_config("docs").unwrap();
        assert_eq!(
            config,
            json!({
                "schema_version": "1",
                "sources": "docs",
                "agent_read_format": "toon",
                "picture_folders": [],
                "exclude": [],
                "protected_originals": "skip",
                "assets": {"max_aspect_ratio": 20},
            })
        );
        // The defaults of the contract are those the code takes for a
        // setting left out.
        let none = json!({});
        assert_eq!(
            AssetLimits::of(&config).unwrap(),
            AssetLimits::of(&none).unwrap()
        );
        assert_eq!(
            ProtectedOriginals::of(&config).unwrap(),
            ProtectedOriginals::of(&none).unwrap()
        );
        assert_eq!(config["agent_read_format"], json!(AgentFormat::default()));
        assert!(Exclusion::of(&config).unwrap().0.is_empty());
    }

    #[test]
    fn asset_limits_are_those_of_the_configuration() {
        let limits = |config: Value| AssetLimits::of(&config).map(|l| l.max_aspect_ratio);
        assert_eq!(limits(json!({})).unwrap(), Some(20.0));
        assert_eq!(limits(json!({"assets":{}})).unwrap(), Some(20.0));
        assert_eq!(
            limits(json!({"assets":{"max_aspect_ratio":40}})).unwrap(),
            Some(40.0)
        );
        assert_eq!(
            limits(json!({"assets":{"max_aspect_ratio":null}})).unwrap(),
            None
        );
        assert!(limits(json!({"assets":{"max_aspect_ratio":0.5}})).is_err());
        assert!(limits(json!({"assets":{"max_aspect_ratio":"wide"}})).is_err());
        // Whichever side is the longer, and no longer than the limit is kept.
        let default = AssetLimits::default();
        assert!(default.slender(401.0, 20.0) && default.slender(20.0, 401.0));
        assert!(!default.slender(400.0, 20.0) && !default.slender(0.0, 0.0));
        assert!(default.slender(5.0, 0.0));
        let none = AssetLimits {
            max_aspect_ratio: None,
        };
        assert!(!none.slender(1000.0, 1.0));
    }
}
