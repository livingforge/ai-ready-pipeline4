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

pub(super) fn words(text: &str) -> String {
    tiniestsegmenter::tokenize(&normalize(text))
        .into_iter()
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
    let chars: Vec<_> = text.chars().collect();
    let pairs = chars
        .windows(2)
        .map(|p| encoded('g', &p.iter().collect::<String>()));
    // Unigrams support two-kanji and single-character queries without a scan.
    let singles: BTreeSet<_> = chars
        .iter()
        .filter(|c| !c.is_whitespace())
        .map(|c| encoded('u', &c.to_string()))
        .collect();
    pairs.chain(singles).collect::<Vec<_>>().join(" ")
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

impl Query {
    pub fn new(text: &str, synonyms: Option<&Path>) -> Result<Self> {
        let normalized = normalize(text);
        ensure!(
            !normalized.is_empty() && normalized.chars().count() <= 256,
            "search query must contain 1..256 characters"
        );
        ensure!(
            normalized.chars().any(char::is_alphanumeric),
            "search query must contain letters or numbers"
        );
        let dictionary = if let Some(path) = synonyms {
            read(path, Some("search-synonyms"))?
        } else {
            serde_json::json!({"groups":[]})
        };
        let dictionary: Vec<Vec<String>> = array(&dictionary["groups"])?
            .iter()
            .map(|group| {
                array(group)?
                    .iter()
                    .map(|v| Ok(normalize(string(v)?)))
                    .collect()
            })
            .collect::<Result<_>>()?;
        let mut groups = vec![];
        for term in normalized.split_whitespace() {
            let mut group = BTreeSet::from([term.to_owned()]);
            for synonyms in &dictionary {
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
