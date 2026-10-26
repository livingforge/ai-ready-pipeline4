use super::*;
use roxmltree::NodeId;
use std::cell::OnceCell;

/// The shared string table of a package, parsed when a cell first refers to it.
pub(super) struct SharedStrings<'a> {
    parts: &'a BTreeMap<String, Vec<u8>>,
    table: OnceCell<StringTable<'a>>,
}

struct StringTable<'a> {
    source: &'a str,
    document: Document<'a>,
    items: Vec<NodeId>,
}

impl<'a> SharedStrings<'a> {
    pub(super) fn new(parts: &'a BTreeMap<String, Vec<u8>>) -> Self {
        Self {
            parts,
            table: OnceCell::new(),
        }
    }

    /// The `si` at `index`, if any, and the text it was parsed from.
    fn item(&self, index: usize) -> Result<(Option<Node<'_, 'a>>, &'a str)> {
        let table = match self.table.get() {
            Some(table) => table,
            None => {
                let source = std::str::from_utf8(
                    self.parts
                        .get("xl/sharedStrings.xml")
                        .context("missing shared strings")?,
                )?;
                let document = xml(source.as_bytes())?;
                let items = document
                    .root_element()
                    .children()
                    .filter(|node| node.has_tag_name((NS, "si")))
                    .map(|node| node.id())
                    .collect();
                self.table.get_or_init(|| StringTable {
                    source,
                    document,
                    items,
                })
            }
        };
        Ok((
            table
                .items
                .get(index)
                .and_then(|id| table.document.get_node(*id)),
            table.source,
        ))
    }
}

/// Run properties remain invariant when shared strings become cell-local text.
pub(super) fn run_properties(
    cell: Node<'_, '_>,
    original: &str,
    shared: &SharedStrings<'_>,
) -> Result<Vec<String>> {
    let (container, source) = if cell.attribute("t") == Some("s") {
        let index: usize = child(cell, "v")
            .and_then(|node| node.text())
            .context("missing shared string index")?
            .parse()?;
        shared.item(index)?
    } else {
        (child(cell, "is"), original)
    };
    Ok(container
        .into_iter()
        .flat_map(|container| {
            container
                .children()
                .filter(|node| node.has_tag_name((NS, "r")))
        })
        .map(|run| child(run, "rPr").map_or(String::new(), |node| source[node.range()].to_owned()))
        .collect())
}

/// Edit only one unambiguously identified rich-text run. Shared strings are
/// copied into this cell, never changed in the shared string table.
pub(super) fn edited_body(
    cell: Node<'_, '_>,
    original: &str,
    shared: &SharedStrings<'_>,
    value: &Value,
    prefix: &str,
) -> Result<String> {
    let (container, source) = if cell.attribute("t") == Some("s") {
        let index: usize = child(cell, "v")
            .and_then(|node| node.text())
            .context("missing shared string index")?
            .parse()?;
        let (container, source) = shared.item(index)?;
        (
            Some(container.context("invalid shared string index")?),
            source,
        )
    } else {
        (child(cell, "is"), original)
    };
    let Some(container) = container else {
        return scalar_body(prefix, value);
    };
    if value
        .as_str()
        .is_some_and(|after| after == texts(container))
    {
        return preserved_container(container, source);
    }
    ensure!(
        child(container, "rPh").is_none() && child(container, "phoneticPr").is_none(),
        "cell {} has phonetic text; edit it in Excel to preserve its readings",
        cell.attribute("r").unwrap_or("")
    );
    let runs: Vec<_> = container
        .children()
        .filter(|node| node.has_tag_name((NS, "r")))
        .collect();
    if runs.is_empty() {
        return scalar_body(prefix, value);
    }
    let after = value
        .as_str()
        .context("rich text must be edited as a string")?;
    // Validate Excel's text limit and XML characters before preserving run markup.
    scalar_body(prefix, value)?;
    ensure!(
        container
            .children()
            .filter(Node::is_element)
            .all(|node| node.has_tag_name((NS, "r"))),
        "unsupported rich text content; edit it in Excel"
    );
    let mut before = String::new();
    let mut ranges = vec![];
    for run in &runs {
        let text = child(*run, "t").context("rich text run has no text")?;
        ensure!(
            run.children()
                .filter(|node| node.has_tag_name((NS, "t")))
                .count()
                == 1,
            "unsupported rich text run"
        );
        let start = before.len();
        before.push_str(&decode_xstring(text.text().unwrap_or("")));
        ranges.push((start..before.len(), text));
    }
    let locate = |prefix_first: bool| {
        let common = |a: &str, b: &str| {
            a.chars()
                .zip(b.chars())
                .take_while(|(x, y)| x == y)
                .map(|(c, _)| c.len_utf8())
                .sum::<usize>()
        };
        let suffix = |a: &str, b: &str| {
            a.chars()
                .rev()
                .zip(b.chars().rev())
                .take_while(|(x, y)| x == y)
                .map(|(c, _)| c.len_utf8())
                .sum::<usize>()
        };
        let (start, tail) = if prefix_first {
            let start = common(&before, after);
            (start, suffix(&before[start..], &after[start..]))
        } else {
            let tail = suffix(&before, after);
            (
                common(&before[..before.len() - tail], &after[..after.len() - tail]),
                tail,
            )
        };
        (start, before.len() - tail, after.len() - tail)
    };
    let (start, end, new_end) = locate(true);
    ensure!(
        locate(false) == (start, end, new_end),
        "ambiguous repeated rich text; edit it in Excel"
    );
    let touched: Vec<_> = ranges
        .iter()
        .filter(|(range, _)| {
            if start == end {
                range.start <= start && start <= range.end
            } else {
                range.start < end && start < range.end
            }
        })
        .collect();
    ensure!(
        touched.len() == 1,
        "rich text edit crosses a run boundary; edit one run at a time or edit it in Excel"
    );
    let (range, text) = touched[0];
    let replacement = format!(
        "{}{}{}",
        &before[range.start..start],
        &after[start..new_end],
        &before[end..range.end]
    );
    // Keep the original run and property XML; change only its t element.
    let raw = &source[container.range()];
    let opening_end = raw.find('>').context("invalid rich text container")? + 1;
    let closing_start = raw.rfind("</").context("invalid rich text container")?;
    let mut content = raw[opening_end..closing_start].to_owned();
    let encoded = encode_xstring(&replacement)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;");
    let tag = element_tag(&source[text.range()])?;
    let (opening, _) = xml_opening(&source[text.range()])?;
    let opening = set_xml_attribute(opening, "xml:space", "preserve")?;
    let replacement = format!(
        "{}>{encoded}</{tag}>",
        opening.trim_end_matches('>').trim_end_matches('/')
    );
    content.replace_range(
        text.range().start - container.range().start - opening_end
            ..text.range().end - container.range().start - opening_end,
        &replacement,
    );
    wrap_container(container, &content)
}

fn preserved_container(container: Node<'_, '_>, source: &str) -> Result<String> {
    let raw = &source[container.range()];
    let start = raw.find('>').context("invalid rich text container")? + 1;
    let end = raw.rfind("</").unwrap_or(start);
    wrap_container(container, &raw[start..end])
}

fn wrap_container(container: Node<'_, '_>, content: &str) -> Result<String> {
    let namespaces: String = container
        .namespaces()
        .filter(|ns| ns.name() != Some("xml"))
        .map(|ns| match ns.name() {
            Some(name) => format!(" xmlns:{name}=\"{}\"", xml_attr(ns.uri())),
            None => String::new(),
        })
        .collect();
    Ok(format!(
        " t=\"inlineStr\"><is xmlns=\"{NS}\"{namespaces}>{content}</is>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_strings_are_the_si_elements_by_position() {
        let parts = BTreeMap::from([(
            "xl/sharedStrings.xml".to_owned(),
            format!(
                r#"<sst xmlns="{NS}"><si><t>a</t></si><extLst/><si><r><rPr><b/></rPr><t>b</t></r></si></sst>"#
            )
            .into_bytes(),
        )]);
        let shared = SharedStrings::new(&parts);
        for (index, text) in [(1, Some("b")), (0, Some("a")), (2, None), (1, Some("b"))] {
            let (item, _) = shared.item(index).unwrap();
            assert_eq!(item.map(texts).as_deref(), text);
        }
        let sheet = format!(r#"<c xmlns="{NS}" r="A1" t="s"><v>1</v></c>"#);
        let doc = Document::parse(&sheet).unwrap();
        assert_eq!(
            run_properties(doc.root_element(), &sheet, &shared).unwrap(),
            ["<rPr><b/></rPr>"]
        );
        let none = BTreeMap::new();
        let error =
            run_properties(doc.root_element(), &sheet, &SharedStrings::new(&none)).unwrap_err();
        assert_eq!(error.to_string(), "missing shared strings");
    }
}
