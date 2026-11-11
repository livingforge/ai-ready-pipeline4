//! The links between adopted HTML documents: where each `<a href>` points
//! among the imported pages and their anchors, and those that point nowhere.
use super::*;

/// What a link points to.
enum Target {
    /// Another site, mail and the like, which are not checked.
    External,
    /// A path from the site root, whose root ARP does not know.
    RootRelative,
    /// A file of the project, by its path from the project root, and the anchor.
    File {
        path: String,
        fragment: Option<String>,
    },
}

use crate::document_source::percent_decode_lenient as percent_decoded;

/// Where `url`, a link of the page at `page` (a path from the project root),
/// points.
fn target(page: &str, url: &str) -> Target {
    let url = url.trim();
    let scheme = url.find(':').is_some_and(|colon| {
        let name = &url[..colon];
        name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if scheme || url.starts_with("//") {
        return Target::External;
    }
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(percent_decoded(fragment))),
        None => (url, None),
    };
    let path = rest.split('?').next().unwrap_or("");
    if path.starts_with('/') {
        return Target::RootRelative;
    }
    let mut parts: Vec<String> = page.split('/').map(str::to_owned).collect();
    parts.pop();
    if path.is_empty() {
        return Target::File {
            path: page.to_owned(),
            fragment,
        };
    }
    for segment in percent_decoded(path).split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment => parts.push(segment.to_owned()),
        }
    }
    Target::File {
        path: parts.join("/"),
        fragment,
    }
}

/// An adopted HTML document: its ID and the cells its anchors name.
struct Page {
    id: String,
    anchors: BTreeMap<String, String>,
}

impl Store {
    /// The links of the adopted HTML documents at or below `scope`, each with
    /// the document and cell it points to and its state: `ok`, `external`,
    /// `root_relative` (not checked), `not_imported` (a file of the project
    /// that is no adopted document), `missing_page` or `missing_anchor`. The
    /// items are those not `ok` or `external`, or with `all` every link.
    pub fn link_check(&self, scope: Option<&str>, all: bool) -> Result<(Value, Vec<Value>)> {
        // Every adopted page, by its path in lower case, which links may
        // spell otherwise on Windows.
        let mut pages: BTreeMap<String, Page> = BTreeMap::new();
        let mut links = vec![];
        let scoped: BTreeSet<String> = self.ids("documents", scope)?.into_iter().collect();
        for id in self.ids("documents", None)? {
            let dir = self.document(&id)?;
            let meta = read(&dir.join("document.yml"), Some("document"))?;
            let path = string(&meta["source"]["path"])?.to_owned();
            let extension = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !crate::document_source::HTML_FORMATS.contains(&extension.as_str()) {
                pages.insert(
                    path.to_lowercase(),
                    Page {
                        id,
                        anchors: BTreeMap::new(),
                    },
                );
                continue;
            }
            let extraction = crate::extraction::read(&dir.join("extraction.json"))?;
            let sheet = extraction
                .sheets
                .first()
                .context("HTML extraction has no page")?;
            let anchors = sheet["anchors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    Some((
                        a["name"].as_str()?.to_owned(),
                        a["range"].as_str()?.to_owned(),
                    ))
                })
                .collect();
            if scoped.contains(&id) {
                for link in sheet["links"].as_array().into_iter().flatten() {
                    links.push((id.clone(), path.clone(), link.clone()));
                }
            }
            pages.insert(path.to_lowercase(), Page { id, anchors });
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut items = vec![];
        for (id, path, link) in &links {
            let url = link["url"].as_str().unwrap_or("");
            let mut item = json!({"document":id,"cell":link["range"],"url":url});
            if let Some(text) = link.get("text") {
                item["text"] = text.clone();
            }
            let state = match target(path, url) {
                Target::External => "external",
                Target::RootRelative => "root_relative",
                Target::File { mut path, fragment } => {
                    // A folder is served as its index page.
                    if !pages.contains_key(&path.to_lowercase())
                        && under(&self.root, &path).is_ok_and(|p| p.is_dir())
                        && let Some(index) = ["index.html", "index.htm"]
                            .iter()
                            .map(|name| format!("{}/{name}", path.trim_end_matches('/')))
                            .find(|index| pages.contains_key(&index.to_lowercase()))
                    {
                        path = index;
                    }
                    let page = pages.get(&path.to_lowercase());
                    item["target"] = json!({"path":path,"document":page.map(|p| &p.id)});
                    match (page, fragment.as_deref().filter(|f| !f.is_empty())) {
                        (None, _) if under(&self.root, &path).is_ok_and(|p| p.is_file()) => {
                            "not_imported"
                        }
                        (None, _) => "missing_page",
                        (Some(_), None) => "ok",
                        (Some(page), Some(fragment)) => match page.anchors.get(fragment) {
                            Some(cell) => {
                                item["target"]["cell"] = json!(cell);
                                item["target"]["anchor"] = json!(fragment);
                                "ok"
                            }
                            // Other formats have no anchors to check.
                            None if html_page(&path) => {
                                item["target"]["anchor"] = json!(fragment);
                                "missing_anchor"
                            }
                            None => "ok",
                        },
                    }
                }
            };
            *counts.entry(state).or_default() += 1;
            item["state"] = json!(state);
            if all || !matches!(state, "ok" | "external") {
                items.push(item);
            }
        }
        let broken = counts.get("missing_page").copied().unwrap_or(0)
            + counts.get("missing_anchor").copied().unwrap_or(0);
        Ok((
            json!({"state":"checked","links":links.len(),"broken":broken,"counts":counts}),
            items,
        ))
    }
}

/// Whether `path` names an HTML page, whose anchors are all recorded.
fn html_page(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            crate::document_source::HTML_FORMATS.contains(&e.to_ascii_lowercase().as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(target: Target) -> (String, Option<String>) {
        match target {
            Target::File { path, fragment } => (path, fragment),
            _ => panic!("not a file"),
        }
    }

    #[test]
    fn links_resolve_against_the_page() {
        let page = "docs/guide/install.html";
        assert_eq!(
            file(target(page, "linux.html")),
            ("docs/guide/linux.html".into(), None)
        );
        assert_eq!(
            file(target(page, "../api/%E8%A8%AD%E5%AE%9A.html?x=1#opt-a")),
            ("docs/api/設定.html".into(), Some("opt-a".into()))
        );
        assert_eq!(
            file(target(page, "#top")),
            (page.into(), Some("top".into()))
        );
        assert_eq!(file(target(page, "./")), ("docs/guide".into(), None));
        for url in [
            "https://example.com/",
            "mailto:a@b",
            "//cdn/x.js",
            "javascript:void(0)",
        ] {
            assert!(matches!(target(page, url), Target::External), "{url}");
        }
        assert!(matches!(target(page, "/docs/x.html"), Target::RootRelative));
    }
}
