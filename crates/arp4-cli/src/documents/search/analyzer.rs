use crate::data::{array, read, string};
use anyhow::{Result, ensure};
use std::{collections::BTreeSet, path::Path};
use unicode_normalization::UnicodeNormalization;

pub(super) fn normalize(text: &str) -> String {
    let folded: String = text.nfkc().flat_map(char::to_lowercase).collect();
    let mut out = String::with_capacity(folded.len());
    for word in folded.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn encoded(prefix: char, text: &str) -> String {
    let mut out = String::with_capacity(1 + 4 * text.len());
    push_encoded(&mut out, prefix, text.chars());
    out
}

/// Writes the token of `chars` after `prefix`: each character as its code
/// point in lowercase hex and a `z`. Written by hand: a table row has
/// hundreds of characters, each written several times, and formatting each
/// took a third of indexing a sheet.
fn push_encoded(out: &mut String, prefix: char, chars: impl IntoIterator<Item = char>) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push(prefix);
    for c in chars {
        let mut code = c as u32;
        let mut digits = [0u8; 8];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = DIGITS[(code & 0xf) as usize];
            code >>= 4;
            if code == 0 {
                break;
            }
        }
        for &digit in &digits[start..] {
            out.push(char::from(digit));
        }
        out.push('z');
    }
}

/// Adds the token of `chars` after `prefix` to the tokens of `out`, which
/// spaces separate.
fn push_token(out: &mut String, prefix: char, chars: impl IntoIterator<Item = char>) {
    if !out.is_empty() {
        out.push(' ');
    }
    push_encoded(out, prefix, chars);
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
    let mut out = String::new();
    for (ascii, run) in runs(&normalize(text)) {
        let segments = if ascii {
            vec![run]
        } else {
            tiniestsegmenter::tokenize(run)
        };
        for segment in segments {
            for word in segment
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
            {
                push_token(&mut out, 'w', word.chars());
            }
        }
    }
    out
}

pub(super) fn grams(text: &str) -> String {
    // Query terms hold no whitespace, so a pair across whitespace is never
    // searched; in a table row half the pairs are. Whitespace becomes one
    // break token instead, so pairs on either side never read as adjacent.
    let mut out = String::with_capacity(12 * text.len());
    for chunk in text.split_whitespace() {
        let mut chars = chunk.chars();
        let Some(mut before) = chars.next() else {
            continue;
        };
        let mut pairs = chars.peekable();
        if pairs.peek().is_none() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(" b");
        }
        for after in pairs {
            push_token(&mut out, 'g', [before, after]);
            before = after;
        }
    }
    // Unigrams support two-kanji and single-character queries without a scan,
    // in the order of their tokens.
    let singles: BTreeSet<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut singles: Vec<String> = singles
        .into_iter()
        .map(|c| encoded('u', &c.to_string()))
        .collect();
    singles.sort_unstable();
    for single in singles {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&single);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens are each character's code point in hex and a z, after their
    /// kind, separated by spaces: pairs within each run of non-space text, a
    /// break between runs, then each character once in their order.
    #[test]
    fn tokens_spell_each_character_in_hex() {
        assert_eq!(encoded('g', "aあ"), "g61z3042z");
        assert_eq!(encoded('u', "😀"), "u1f600z");
        assert_eq!(encoded('u', " "), "u0z");
        assert_eq!(
            grams("ab cd e"),
            "g61z62z b g63z64z u61z u62z u63z u64z u65z"
        );
        assert_eq!(grams("😀"), "u1f600z");
        assert_eq!(grams(" "), "");
        assert_eq!(
            words("TBL-00001  Ab"),
            "w74z62z6cz w30z30z30z30z31z w61z62z"
        );
        assert_eq!(normalize("  Ａ	 b  "), "a b");
    }
}
