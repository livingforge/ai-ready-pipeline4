//! Repository-local configuration shared by document and specification commands.
use crate::agent_format::AgentFormat;
use crate::data::{read, under, write};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn discover(start: &Path) -> Result<PathBuf> {
    let start = dunce::canonicalize(start)?;
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
    let root = dunce::canonicalize(root)?;
    let config = read(&under(&root, ".arp/config.yml")?, Some("project-config"))?;
    under(
        &root,
        config["sources"].as_str().context("sources required")?,
    )?;
    Ok(root)
}

/// Reading views may also be produced outside a configured project. Stop at the
/// repository boundary, and never hide an invalid existing configuration.
pub fn agent_read_format(start: &Path) -> Result<AgentFormat> {
    let start = dunce::canonicalize(start)?;
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
    let root = dunce::canonicalize(root)?;
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
        write(
            &config,
            &json!({"schema_version":"1", "sources":sources, "agent_read_format":AgentFormat::default()}),
        )?;
    }
    ensure_lines(&under(&root, ".arp/.gitignore")?, &["/work/", "/cache/"])?;
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

/// Appends the lines a file lacks, keeping what the user already wrote.
fn ensure_lines(path: &Path, lines: &[&str]) -> Result<()> {
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
