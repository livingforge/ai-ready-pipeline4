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
    project_picture_folders(&root, &config)?;
    Ok(root)
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
    let page = dunce::canonicalize(page)?;
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
                .filter_map(|folder| dunce::canonicalize(folder).ok())
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
