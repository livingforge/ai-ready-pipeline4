use super::*;

pub(super) fn replace_xml_attribute(opening: &str, name: &str, value: &str) -> Result<String> {
    let pattern = format!(r#"\s+{}\s*=\s*(?:"[^"]*"|'[^']*')"#, regex::escape(name));
    let re = regex::Regex::new(&pattern)?;
    ensure!(re.is_match(opening), "missing XML attribute: {name}");
    Ok(re
        .replace(opening, format!(" {name}=\"{value}\""))
        .into_owned())
}

pub(super) fn remove_xml_attribute(opening: &str, name: &str) -> Result<String> {
    let pattern = format!(r#"\s+{}\s*=\s*(?:"[^"]*"|'[^']*')"#, regex::escape(name));
    let re = regex::Regex::new(&pattern)?;
    Ok(re.replace(opening, "").into_owned())
}

pub(super) fn xml_opening(raw: &str) -> Result<(&str, &str)> {
    let end = raw.find('>').context("invalid XML element")?;
    Ok((&raw[..end + 1], &raw[end + 1..]))
}

pub(super) fn xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub(super) fn relationships_part(part: &str) -> Result<String> {
    let (directory, file) = part.rsplit_once('/').context("part has no directory")?;
    Ok(format!("{directory}/_rels/{file}.rels"))
}

pub(super) fn normalized_part(path: &str) -> Result<String> {
    let mut segments = vec![];
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => ensure!(segments.pop().is_some(), "relationship escapes package"),
            value => segments.push(value),
        }
    }
    ensure!(!segments.is_empty(), "empty package part");
    Ok(segments.join("/"))
}

pub(super) fn relationship_target(base_part: &str, target: &str) -> Result<String> {
    if target.starts_with('/') {
        return normalized_part(target.trim_start_matches('/'));
    }
    let directory = base_part
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    normalized_part(&format!("{directory}/{target}"))
}

pub(super) fn relative_target(base_part: &str, target_part: &str) -> Result<String> {
    let base = base_part
        .rsplit_once('/')
        .map_or_else(Vec::new, |(directory, _)| directory.split('/').collect());
    let target: Vec<_> = target_part.split('/').collect();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = vec![];
    parts.extend(std::iter::repeat_n("..", base.len() - common));
    parts.extend(target.iter().skip(common).copied());
    ensure!(!parts.is_empty(), "empty relationship target");
    Ok(parts.join("/"))
}

pub(super) fn relationship(rels: &str, id: &str) -> Result<Option<(String, String)>> {
    let doc = xml(rels.as_bytes())?;
    let Some(node) = doc
        .root_element()
        .children()
        .filter(|node| node.has_tag_name((PKG_REL, "Relationship")))
        .find(|node| node.attribute("Id") == Some(id))
    else {
        return Ok(None);
    };
    Ok(Some((
        node.attribute("Type")
            .context("relationship type missing")?
            .to_owned(),
        node.attribute("Target")
            .context("relationship target missing")?
            .to_owned(),
    )))
}

pub(super) fn relationship_id(rels: Option<&str>) -> Result<String> {
    let Some(rels) = rels else {
        return Ok("rId1".to_owned());
    };
    let doc = xml(rels.as_bytes())?;
    let mut used = BTreeSet::new();
    for node in doc
        .root_element()
        .children()
        .filter(|node| node.has_tag_name((PKG_REL, "Relationship")))
    {
        let id = node.attribute("Id").context("relationship ID missing")?;
        used.insert(id.to_owned());
    }
    for number in 1..u32::MAX {
        let candidate = format!("rId{number}");
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    bail!("relationship ID space exhausted")
}

pub(super) fn append_relationship(
    rels: Option<&str>,
    id: &str,
    kind: &str,
    target: &str,
) -> Result<String> {
    let mut output = rels
        .map(str::to_owned)
        .unwrap_or_else(|| format!(r#"<Relationships xmlns="{PKG_REL}"></Relationships>"#));
    let doc = xml(output.as_bytes())?;
    ensure!(
        relationship(&output, id)?.is_none(),
        "duplicate relationship ID"
    );
    ensure!(
        doc.root_element().has_tag_name((PKG_REL, "Relationships")),
        "invalid relationships part"
    );
    let element = format!(
        r#"<Relationship Id="{}" Type="{}" Target="{}"/>"#,
        xml_attr(id),
        xml_attr(kind),
        xml_attr(target)
    );
    let end = output
        .rfind("</")
        .context("relationships root has no closing tag")?;
    output.insert_str(end, &element);
    Ok(output)
}

pub(super) fn insert_before_root_children(
    original: &str,
    child_xml: &str,
    boundaries: &[(&str, &str)],
) -> Result<String> {
    let doc = xml(original.as_bytes())?;
    let root = doc.root_element();
    let end = root
        .children()
        .filter(Node::is_element)
        .find(|node| {
            boundaries
                .iter()
                .any(|(namespace, name)| node.has_tag_name((*namespace, *name)))
        })
        .map_or_else(
            || original.rfind("</").context("XML root has no closing tag"),
            |node| Ok(node.range().start),
        )?;
    let mut output = original.to_owned();
    output.insert_str(end, child_xml);
    Ok(output)
}

pub(super) fn append_xml_child(original: &str, child_xml: &str) -> Result<String> {
    insert_before_root_children(original, child_xml, &[])
}

pub(crate) fn write_archive(
    raw: &[u8],
    destination: &Path,
    patched: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    write_archive_without(raw, destination, patched, &BTreeSet::new())
}

/// Writes the original bytes to a new file, so an unedited export keeps the
/// source hash.
pub(crate) fn write_unchanged(raw: &[u8], destination: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    file.write_all(raw)?;
    file.sync_all()?;
    Ok(())
}

/// Writes the package with `patched` parts replaced or added and `removed`
/// parts left out; all other entries are copied unchanged.
pub(super) fn write_archive_without(
    raw: &[u8],
    destination: &Path,
    patched: &BTreeMap<String, Vec<u8>>,
    removed: &BTreeSet<String>,
) -> Result<()> {
    // Rebuilding the archive rewrites ZIP headers (the zip crate adds S_IFREG
    // to external attributes) even when every part is copied as-is.
    if patched.is_empty() && removed.is_empty() {
        return write_unchanged(raw, destination);
    }
    let mut source = ZipArchive::new(Cursor::new(raw))?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut output = ZipWriter::new(file);
    output.set_raw_comment(source.comment().to_vec().into())?;
    let mut copied = BTreeSet::new();
    for i in 0..source.len() {
        let entry = source.by_index(i)?;
        copied.insert(entry.name().to_owned());
        if removed.contains(entry.name()) {
            continue;
        }
        if let Some(bytes) = patched.get(entry.name()) {
            let mut options = SimpleFileOptions::default().compression_method(entry.compression());
            if let Some(time) = entry.last_modified() {
                options = options.last_modified_time(time)
            }
            if let Some(mode) = entry.unix_mode() {
                options = options.unix_permissions(mode)
            }
            output.start_file(entry.name(), options)?;
            output.write_all(bytes)?;
        } else {
            output.raw_copy_file(entry)?;
        }
    }
    for (name, bytes) in patched {
        if copied.contains(name) {
            continue;
        }
        output.start_file(name, SimpleFileOptions::default())?;
        output.write_all(bytes)?;
    }
    output.finish()?.sync_all()?;
    Ok(())
}
