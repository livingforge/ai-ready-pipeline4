use crate::data::{array, read, string};
use anyhow::{Result, ensure};
use std::{collections::BTreeSet, path::Path};
use unicode_normalization::UnicodeNormalization;

pub(super) fn normalize(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn encoded(prefix: char, text: &str) -> String {
    use std::fmt::Write;
    let mut out = String::from(prefix);
    for c in text.chars() {
        write!(out, "{:x}z", c as u32).unwrap();
    }
    out
}

/// Split text where it changes between ASCII letters and digits and the rest.
fn runs(text: &str) -> Vec<(bool, &str)> {
    let mut runs = vec![];
    let mut start = 0;
    let mut current = None;
    for (i, c) in text.char_indices() {
        let ascii = c.is_ascii_alphanumeric();
        if let Some(kind) = current.filter(|kind| *kind != ascii) {
            runs.push((kind, &text[start..i]));
            start = i;
        }
        current = Some(ascii);
    }
    if let Some(kind) = current {
        runs.push((kind, &text[start..]));
    }
    runs
}

pub(super) fn words(text: &str) -> String {
    // The segmenter cuts digits apart one by one, so an ID such as E01234
    // would match any text holding an e and those digits anywhere. A run of
    // ASCII letters and digits stays one word; only the rest is segmented.
    runs(&normalize(text))
        .into_iter()
        .flat_map(|(ascii, run)| {
            if ascii {
                vec![run]
            } else {
                tiniestsegmenter::tokenize(run)
            }
        })
        .flat_map(|s| {
            s.split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .map(|s| encoded('w', s))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn grams(text: &str) -> String {
    // Query terms hold no whitespace, so a pair across whitespace is never
    // searched; in a table row half the pairs are. Whitespace becomes one
    // break token instead, so pairs on either side never read as adjacent.
    let mut pairs = vec![];
    for chunk in text.split_whitespace() {
        let chars: Vec<_> = chunk.chars().collect();
        if chars.len() < 2 {
            continue;
        }
        if !pairs.is_empty() {
            pairs.push("b".to_owned());
        }
        pairs.extend(
            chars
                .windows(2)
                .map(|p| encoded('g', &p.iter().collect::<String>())),
        );
    }
    // Unigrams support two-kanji and single-character queries without a scan.
    let singles: BTreeSet<_> = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| encoded('u', &c.to_string()))
        .collect();
    pairs.extend(singles);
    pairs.join(" ")
}

pub(super) fn identifiers(text: &str) -> String {
    let ids: BTreeSet<_> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || "_./:-".contains(c)))
        .filter(|s| s.chars().any(|c| c.is_ascii_alphanumeric()))
        .collect();
    format!("\n{}\n", ids.into_iter().collect::<Vec<_>>().join("\n"))
}

pub(super) struct Query {
    pub expression: String,
    pub normalized: String,
    pub identifier: String,
    pub groups: Vec<Vec<String>>,
}

pub(super) fn dictionary(path: Option<&Path>) -> Result<Vec<Vec<String>>> {
    let dictionary = if let Some(path) = path {
        read(path, Some("search-synonyms"))?
    } else {
        serde_json::json!({"groups":[]})
    };
    array(&dictionary["groups"])?
        .iter()
        .map(|group| {
            array(group)?
                .iter()
                .map(|v| Ok(normalize(string(v)?)))
                .collect()
        })
        .collect()
}

impl Query {
    pub fn new(text: &str, dictionary: &[Vec<String>]) -> Result<Self> {
        let normalized = normalize(text);
        ensure!(
            !normalized.is_empty() && normalized.chars().count() <= 256,
            "search query must contain 1..256 characters"
        );
        ensure!(
            normalized.chars().any(char::is_alphanumeric),
            "search query must contain letters or numbers"
        );
        let mut groups = vec![];
        for term in normalized.split_whitespace() {
            let mut group = BTreeSet::from([term.to_owned()]);
            for synonyms in dictionary {
                if synonyms.iter().any(|s| s == term) {
                    group.extend(synonyms.iter().cloned());
                }
            }
            ensure!(
                group.len() <= 32,
                "too many synonyms for query term: {term}"
            );
            groups.push(group.into_iter().collect::<Vec<_>>());
        }
        ensure!(groups.len() <= 16, "search query exceeds 16 terms");
        let expression = groups
            .iter()
            .map(|group| {
                let alternatives = group
                    .iter()
                    .map(|term| {
                        let chars: Vec<_> = term.chars().collect();
                        let partial = if chars.len() == 1 {
                            encoded('u', term)
                        } else {
                            chars
                                .windows(2)
                                .map(|p| encoded('g', &p.iter().collect::<String>()))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        let segmented = words(term)
                            .split_whitespace()
                            .map(|w| format!("{{title body context}} : \"{w}\""))
                            .collect::<Vec<_>>()
                            .join(" AND ");
                        if segmented.is_empty() {
                            format!("grams : \"{partial}\"")
                        } else {
                            format!("(({segmented}) OR grams : \"{partial}\")")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ");
                format!("({alternatives})")
            })
            .collect::<Vec<_>>()
            .join(" AND ");
        Ok(Self {
            expression,
            normalized,
            identifier: format!("\n{}\n", text.trim()),
            groups,
        })
    }
}
