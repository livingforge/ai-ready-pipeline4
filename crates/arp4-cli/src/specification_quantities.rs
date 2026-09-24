//! Conservative, source-bound checks for explicitly supported Japanese quantities.
//! This is a lexical consistency check, not an entailment or completeness proof.
use super::{AtomicValue, Comparison, Item, Source};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuantitySemantics {
    #[default]
    Scalar,
    Mean,
    Period,
}
impl QuantitySemantics {
    pub fn is_scalar(&self) -> bool {
        *self == Self::Scalar
    }
}

/// Small, reviewed lexical normalizations for expressions without an Arabic
/// number. This is deliberately an enum, rather than a caller-supplied rule
/// string, so the validator never trusts an unregistered interpretation.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuantityNormalization {
    Once,
    Quarterly,
}

/// How a quantity was interpreted. The interpretation is part of the value,
/// rather than an accidental property of the validator's vocabulary.
#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuantityInterpretation {
    #[default]
    Canonical,
    OpaqueUnit,
    ReviewedLexical {
        rule: QuantityNormalization,
    },
}

static QUANTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?P<device>[A-Za-z][A-Za-z0-9_-]*\s*[x×]\s*)?(?P<prefix>平均|約|上限|最大|最低|最小)?\s*(?P<number>[0-9]+(?:,[0-9]{3})*(?:\.[0-9]+)?)\s*(?P<scale>千|万|億)?\s*(?P<unit>(?:ミリ秒|トークン|チャンク|か所|箇所|段階|次元|ms|時間|世代|明細|部門|ヶ月|か月|秒|分|日|年|月|週|名|人|件|回|台|枚|問|倍|円|文字|桁|%|％|GB|MB|KB)(?:/(?:トークン|チャンク|秒|分|時|日|月|年|件|回|人))?)?\s*(?P<suffix>以上|以下|以内|未満|を超えた|を超える|を上限|超|程度|ごと|毎|（平均）|\(平均\))?|(?P<period>日次|週次|月次|年次)"
).unwrap()
});

static OPAQUE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<prefix>平均|約|上限|最大|最低|最小)?\s*(?P<number>[0-9]+(?:,[0-9]{3})*(?:\.[0-9]+)?)\s*(?P<scale>千|万|億)?$",
    )
    .unwrap()
});

static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+(?:,[0-9]{3})*(?:\.[0-9]+)?").unwrap());

// A number followed by a separate word may carry an unregistered unit ("3 ノード").
static OPAQUE_TEXT_QUANTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]+(?:,[0-9]{3})*(?:\.[0-9]+)?\s+[^\s、。,.;:()（）\[\]{}]+").unwrap()
});

// Calendar labels are text, not durations. Only complete, bounded dates are excluded.
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r"(?:[0-9]{4}[/\-](?:1[0-2]|0?[1-9])(?:[/\-](?:3[01]|[12][0-9]|0?[1-9]))?|[0-9]{4}年度|[0-9]{4}年(?:1[0-2]|0?[1-9])月(?:(?:3[01]|[12][0-9]|0?[1-9])日)?)"
).unwrap()
});

static JAPANESE_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"第\s*[0-9]+(?:\.[0-9]+)*\s*版").unwrap());

// A yearless M/D is also a ratio ("3/4 の賛成"); only a following temporal particle decides.
static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<date>(?:0?[1-9]|1[0-2])/(?:0?[1-9]|[12][0-9]|3[01]))\s*(?:に|まで|から|時点|頃|ごろ|付|以降|以前)",
    )
    .unwrap()
});

/// Why a number inside a text value is not a quantity. Only fixed lexical
/// rules decide this, recomputed on every check, so an extraction cannot
/// label its own quantities as exempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantityExemption {
    CalendarDate,
    Identifier,
    Version,
}
impl QuantityExemption {
    pub fn label(&self) -> &'static str {
        match self {
            Self::CalendarDate => "日付",
            Self::Identifier => "識別子",
            Self::Version => "版番号",
        }
    }
}

fn bounded_date(text: &str, start: usize, end: usize) -> Option<&str> {
    DATE.find_iter(text)
        .find(|m| {
            m.start() <= start
                && end <= m.end()
                && !text[..m.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit())
                && !text[m.end()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        })
        .map(|m| m.as_str())
}

fn month_day(text: &str, start: usize, end: usize) -> Option<&str> {
    MONTH_DAY
        .captures_iter(text)
        .filter_map(|c| c.name("date"))
        .find(|m| {
            m.start() <= start
                && end <= m.end()
                && !text[..m.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit() || c == '/')
        })
        .map(|m| m.as_str())
}

/// "2026 年度" is still a calendar label once the spacing after the year is ignored.
fn spaced_date(text: &str, number: (usize, usize)) -> Option<String> {
    let rest = text[number.1..].trim_start();
    if rest.len() == text.len() - number.1 || rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let joined = format!("{}{rest}", &text[number.0..number.1]);
    DATE.find(&joined)
        .filter(|d| {
            d.start() == 0
                && !joined[d.end()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        })
        .map(|d| d.as_str().to_owned())
}

/// A number glued to an ASCII name ("SHA-256", "voyage-4", "v1.2"). Callers
/// keep it a quantity when a known unit follows ("Top-5件", "Top-5 件").
fn identifier(text: &str, start: usize, end: usize) -> Option<(QuantityExemption, String)> {
    let glued = |c: char| c.is_ascii_alphanumeric() || "-_.".contains(c);
    let head = text[..start]
        .char_indices()
        .rev()
        .take_while(|(_, c)| glued(*c))
        .last()
        .map_or(start, |(i, _)| i);
    let prefix = text[head..start].to_ascii_lowercase();
    // "x4" is a multiplier, not a name.
    if !prefix.chars().any(|c| c.is_ascii_alphabetic()) || prefix == "x" {
        return None;
    }
    let tail = end
        + text[end..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || "-_".contains(*c))
            .map(char::len_utf8)
            .sum::<usize>();
    let kind = if matches!(prefix.as_str(), "v" | "ver" | "ver.") {
        QuantityExemption::Version
    } else {
        QuantityExemption::Identifier
    };
    Some((kind, text[head..tail].to_owned()))
}

/// Byte ranges: `literal` is the matched quantity phrase, `number` its digits.
fn exemption(
    text: &str,
    literal: (usize, usize),
    number: (usize, usize),
    has_unit: bool,
) -> Option<(QuantityExemption, String)> {
    if let Some(date) = bounded_date(text, literal.0, literal.1)
        .or_else(|| month_day(text, number.0, number.1))
        .map(str::to_owned)
        .or_else(|| spaced_date(text, number))
    {
        return Some((QuantityExemption::CalendarDate, date));
    }
    if has_unit {
        return None;
    }
    if let Some(version) = JAPANESE_VERSION
        .find_iter(text)
        .find(|m| m.start() <= number.0 && number.1 <= m.end())
    {
        return Some((QuantityExemption::Version, version.as_str().to_owned()));
    }
    identifier(text, number.0, number.1)
}

#[derive(Default)]
pub(super) struct TextScan {
    pub quantity: bool,
    pub exemptions: Vec<(QuantityExemption, String)>,
}

/// Classify every number in a text value: a recognized quantity, or a
/// rule-based exemption that is reported instead of silently skipped.
pub(super) fn scan_text(text: &str) -> TextScan {
    let mut scan = TextScan::default();
    for c in QUANTITY.captures_iter(text) {
        if c.name("unit").is_none() && c.name("period").is_none() {
            continue;
        }
        // Period words (日次) carry no digits to exempt.
        let Some(number) = c.name("number").map(|n| (n.start(), n.end())) else {
            scan.quantity = true;
            continue;
        };
        let m = c.get(0).unwrap();
        let literal = m.as_str().trim();
        let start = m.start() + m.as_str().find(literal).unwrap();
        match exemption(text, (start, start + literal.len()), number, true) {
            Some(found) => scan.exemptions.push(found),
            None => scan.quantity = true,
        }
    }
    if text.contains("一度だけ")
        || text.contains("一回だけ")
        || text.contains("四半期ごと")
        || text.contains("四半期毎")
    {
        scan.quantity = true;
    }
    for n in NUMBER.find_iter(text) {
        if !OPAQUE_TEXT_QUANTITY.is_match(&text[n.start()..]) {
            continue;
        }
        let number = (n.start(), n.end());
        let found = exemption(text, number, number, false);
        match found {
            Some(found) => scan.exemptions.push(found),
            None => scan.quantity = true,
        }
    }
    scan.exemptions.sort();
    scan.exemptions.dedup();
    scan
}

// Decimal normalization avoids rounding distinct large integers through f64.
fn decimal(raw: &str, scale: usize) -> String {
    let raw = raw.replace(',', "");
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let mut digits = format!("{whole}{fraction}");
    let exponent = scale as isize - fraction.len() as isize;
    if exponent >= 0 {
        digits.push_str(&"0".repeat(exponent as usize));
    } else {
        let places = (-exponent) as usize;
        if digits.len() <= places {
            digits = format!("{}{}", "0".repeat(places + 1 - digits.len()), digits);
        }
        digits.insert(digits.len() - places, '.');
    }
    let digits = digits.trim_start_matches('0');
    let mut result = if digits.starts_with('.') {
        format!("0{digits}")
    } else {
        digits.to_owned()
    };
    if result.contains('.') {
        result = result
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned();
    }
    if result.is_empty() {
        "0".into()
    } else {
        result
    }
}

fn comparison_from_modifiers(prefix: &str, suffix: &str) -> Option<Comparison> {
    let prefix_comparison = match prefix {
        "約" => Some(Comparison::Approx),
        "上限" | "最大" => Some(Comparison::Lte),
        "最低" | "最小" => Some(Comparison::Gte),
        _ => None,
    };
    let suffix_comparison = match suffix {
        "以上" => Some(Comparison::Gte),
        "以下" | "以内" | "を上限" => Some(Comparison::Lte),
        "未満" => Some(Comparison::Lt),
        "超" | "を超えた" | "を超える" => Some(Comparison::Gt),
        "程度" => Some(Comparison::Approx),
        "" | "（平均）" | "(平均)" | "ごと" | "毎" => None,
        _ => return None,
    };
    if prefix_comparison.is_some()
        && suffix_comparison.is_some()
        && prefix_comparison != suffix_comparison
    {
        return None;
    }
    suffix_comparison.or(prefix_comparison)
}

fn normalized_quantity_issue(
    normalization: &QuantityNormalization,
    quote: &str,
    amount: &serde_json::Number,
    unit: &str,
    comparison: &Comparison,
    semantics: &QuantitySemantics,
) -> Option<&'static str> {
    let (quotes, expected_amount, expected_unit, expected_comparison, expected_semantics) =
        match normalization {
            QuantityNormalization::Once => (
                ["一度だけ", "一回だけ"].as_slice(),
                "1",
                "回",
                Comparison::Eq,
                QuantitySemantics::Scalar,
            ),
            QuantityNormalization::Quarterly => (
                ["四半期ごと", "四半期毎", "四半期ごとに"].as_slice(),
                "1",
                "四半期",
                Comparison::Eq,
                QuantitySemantics::Period,
            ),
        };
    if !quotes.contains(&quote) {
        return Some("unsupported_quantity_expression");
    }
    if decimal(&amount.to_string(), 0) != expected_amount
        || unit != expected_unit
        || comparison != &expected_comparison
        || semantics != &expected_semantics
    {
        Some("quantity_source_mismatch")
    } else {
        None
    }
}

/// Validate an explicitly source-bound quantity whose unit is not in the
/// built-in vocabulary. The numeric grammar remains strict, but the unit is
/// supplied by the model and must be cited exactly with unit_basis.
fn opaque_quantity_issue(
    basis: &super::Span,
    amount: &serde_json::Number,
    unit: &str,
    comparison: &Comparison,
    semantics_basis: Option<&super::Span>,
    semantics: &QuantitySemantics,
) -> &'static str {
    let quote = &basis.quote;
    let positions: Vec<_> = quote.match_indices(unit).map(|(start, _)| start).collect();
    let (number_text, suffix) = match positions.as_slice() {
        [unit_start] => {
            let unit_end = unit_start + unit.len();
            // Digits immediately before a unit belong to the amount (40問).
            // OPAQUE_NUMBER below validates the entire prefix, so accepting this
            // boundary cannot truncate a number or an unknown unit prefix.
            if (quote[..*unit_start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_digit())
                && unit.starts_with(|c: char| c.is_ascii_digit() || ".,".contains(c)))
                || quote[unit_end..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                return "unsupported_quantity_unit";
            }
            (&quote[..*unit_start], quote[unit_end..].trim())
        }
        [] => (quote.as_str(), ""),
        _ => return "ambiguous_quantity_basis",
    };
    if suffix.starts_with('/')
        || suffix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return "unsupported_quantity_unit";
    }
    let Some(captures) = OPAQUE_NUMBER.captures(number_text.trim()) else {
        return "unsupported_quantity_expression";
    };
    let prefix = captures.name("prefix").map_or("", |m| m.as_str());
    let suffix_comparison = comparison_from_modifiers(prefix, suffix);
    if !suffix.is_empty()
        && !matches!(suffix, "（平均）" | "(平均)" | "ごと" | "毎")
        && suffix_comparison.is_none()
    {
        return "unsupported_quantity_expression";
    }
    let expected_semantics =
        if prefix == "平均" || matches!(suffix, "（平均）" | "(平均)") || semantics_basis.is_some()
        {
            QuantitySemantics::Mean
        } else if matches!(suffix, "ごと" | "毎") {
            QuantitySemantics::Period
        } else {
            QuantitySemantics::Scalar
        };
    if expected_semantics == QuantitySemantics::Mean && matches!(suffix, "ごと" | "毎") {
        return "unsupported_quantity_expression";
    }
    let scale = match captures.name("scale").map(|m| m.as_str()) {
        Some("千") => 3,
        Some("万") => 4,
        Some("億") => 8,
        _ => 0,
    };
    let expected_amount = decimal(captures.name("number").unwrap().as_str(), scale);
    let expected_comparison = suffix_comparison.unwrap_or(Comparison::Eq);
    if decimal(&amount.to_string(), 0) != expected_amount
        || comparison != &expected_comparison
        || semantics != &expected_semantics
    {
        "quantity_source_mismatch"
    } else {
        // Prevent a unit supplied from another cell from silently replacing an
        // unknown residue in the quantity basis.
        "quantity_ok"
    }
}

pub(super) fn quantity_issue(
    item: &Item,
    sources: &BTreeMap<&str, &Source>,
) -> Option<&'static str> {
    let AtomicValue::Quantity {
        amount,
        unit,
        comparison,
        interpretation,
        basis,
        unit_basis,
        semantics_basis,
        semantics,
    } = &item.value
    else {
        if matches!(item.value, AtomicValue::QuantityExpression { .. }) {
            return Some("quantity_interpretation_required");
        }
        // Do not inspect unrelated evidence/conditions: a textual method can have numeric context.
        if let AtomicValue::Text { text } = &item.value
            && scan_text(text).quantity
        {
            return Some("quantity_in_text");
        }
        return None;
    };
    let Some(basis) = basis else {
        return Some("missing_quantity_basis");
    };
    // Moving an invented mean from semantics to the property must not bypass the check.
    if item.property.contains("平均") && *semantics != QuantitySemantics::Mean {
        return Some("quantity_semantics_in_property");
    }
    if *semantics != QuantitySemantics::Mean
        && item
            .name
            .as_deref()
            .is_some_and(|name| name.ends_with("平均") || name.ends_with("平均値"))
    {
        return Some("quantity_semantics_in_name");
    }
    let source = sources.get(basis.source.as_str())?;
    // Ranges are not repaired by selecting one endpoint.
    if basis.quote.chars().any(|c| "〜～~".contains(c)) {
        return Some("unsupported_quantity_expression");
    }
    // A separate mean label must be explicitly cited, not inferred from arbitrary evidence.
    // Restrict it to the same row or an earlier column heading in the same workbook/sheet.
    // This checks location and literal wording, not the semantic association of the cells.
    if let Some(label) = semantics_basis {
        let Some(label_source) = sources.get(label.source.as_str()) else {
            return Some("invalid_quantity_semantics_basis");
        };
        let related = match (&source.context, &label_source.context) {
            (Some(value_cell), Some(label_cell)) => {
                source.document == label_source.document
                    && value_cell.sheet == label_cell.sheet
                    && label.source != basis.source
                    && (value_cell.row == label_cell.row
                        || (value_cell.column == label_cell.column
                            && label_cell.row < value_cell.row))
            }
            _ => false,
        };
        if label.quote != "平均" || *semantics != QuantitySemantics::Mean || !related {
            return Some("invalid_quantity_semantics_basis");
        }
    }
    if let QuantityInterpretation::ReviewedLexical { rule } = interpretation {
        return normalized_quantity_issue(rule, &basis.quote, amount, unit, comparison, semantics);
    }
    if *interpretation == QuantityInterpretation::OpaqueUnit {
        let result = opaque_quantity_issue(
            basis,
            amount,
            unit,
            comparison,
            semantics_basis.as_deref(),
            semantics,
        );
        return (result != "quantity_ok").then_some(result);
    }
    for captures in QUANTITY.captures_iter(&source.text) {
        let full = captures.get(0).unwrap();
        let literal = full.as_str().trim();
        let offset = full.as_str().find(literal).unwrap();
        let start_byte = full.start() + offset;
        let end_byte = start_byte + literal.len();
        let start = source.text[..start_byte].chars().count();
        let end = source.text[..end_byte].chars().count();
        if start != basis.start || end != basis.end {
            continue;
        }
        // A second modifier must not be silently dropped (e.g. 平均約4明細).
        let before = source.text[..start_byte].trim_end();
        let after = source.text[end_byte..].trim_start();
        // Do not accept a known-unit prefix of an unknown unit or compound unit.
        if after.starts_with('/')
            || source.text[end_byte..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            return Some("unsupported_quantity_unit");
        }
        if ["平均", "約", "上限", "最大", "最低", "最小"]
            .iter()
            .any(|p| before.ends_with(p))
            || [
                "以上",
                "以下",
                "以内",
                "未満",
                "を超えた",
                "を超える",
                "を上限",
                "超",
                "程度",
                "ごと",
                "毎",
            ]
            .iter()
            .any(|s| after.starts_with(s))
        {
            return Some("unsupported_quantity_expression");
        }
        // Never accept a substring of an unsupported signed/decimal/range expression.
        let mut preceding = before.chars().rev();
        let previous = preceding.next();
        if previous.is_some_and(|c| c.is_ascii_digit() || "+-−〜～~".contains(c))
            || previous.is_some_and(|c| ".,".contains(c))
                && preceding.next().is_some_and(|c| c.is_ascii_digit())
        {
            return Some("unsupported_quantity_expression");
        }
        if source.text[end_byte..].chars().next().is_some_and(|c| {
            c.is_ascii_digit()
                || "〜～~".contains(c)
                || captures.name("unit").is_none() && ".,".contains(c)
        }) {
            return Some("unsupported_quantity_expression");
        }
        let (expected_amount, expected_unit, expected_comparison, expected_semantics) =
            if let Some(period) = captures.name("period") {
                (
                    "1".to_owned(),
                    match period.as_str() {
                        "日次" => "日",
                        "週次" => "週",
                        "月次" => "月",
                        _ => "年",
                    }
                    .to_owned(),
                    Comparison::Eq,
                    QuantitySemantics::Period,
                )
            } else {
                let prefix = captures.name("prefix").map_or("", |m| m.as_str());
                let suffix = captures.name("suffix").map_or("", |m| m.as_str());
                let prefix_comparison = comparison_from_modifiers(prefix, "");
                let suffix_comparison = comparison_from_modifiers("", suffix);
                if prefix_comparison.is_some()
                    && suffix_comparison.is_some()
                    && prefix_comparison != suffix_comparison
                {
                    return Some("unsupported_quantity_expression");
                }
                let meaning = if prefix == "平均"
                    || matches!(suffix, "（平均）" | "(平均)")
                    || semantics_basis.is_some()
                {
                    QuantitySemantics::Mean
                } else if matches!(suffix, "ごと" | "毎") {
                    QuantitySemantics::Period
                } else {
                    QuantitySemantics::Scalar
                };
                if meaning == QuantitySemantics::Mean && matches!(suffix, "ごと" | "毎") {
                    return Some("unsupported_quantity_expression");
                }
                let mut scale = match captures.name("scale").map(|m| m.as_str()) {
                    Some("千") => 3,
                    Some("万") => 4,
                    Some("億") => 8,
                    _ => 0,
                };
                let mut expected_unit = if let Some(found) = captures.name("unit") {
                    found.as_str().to_owned()
                } else if let Some(evidence) = unit_basis {
                    evidence.quote.clone()
                } else {
                    return Some("missing_quantity_unit_basis");
                };
                if let Some(scale_word) = captures.name("scale") {
                    let scaled_unit = format!("{}{expected_unit}", scale_word.as_str());
                    if *unit == scaled_unit {
                        expected_unit = scaled_unit;
                        scale = 0;
                    }
                }
                (
                    decimal(captures.name("number").unwrap().as_str(), scale),
                    expected_unit,
                    suffix_comparison
                        .or(prefix_comparison)
                        .unwrap_or(Comparison::Eq),
                    meaning,
                )
            };
        return if decimal(&amount.to_string(), 0) != expected_amount
            || *unit != expected_unit
            || comparison != &expected_comparison
            || semantics != &expected_semantics
        {
            Some("quantity_source_mismatch")
        } else {
            None
        };
    }
    let candidates: Vec<_> = QUANTITY
        .captures_iter(&source.text)
        .filter(|c| {
            let m = c.get(0).unwrap();
            let trimmed = m.as_str().trim();
            let start = source.text[..m.start()].chars().count()
                + m.as_str()
                    .find(trimmed)
                    .map_or(0, |n| m.as_str()[..n].chars().count());
            let end = start + trimmed.chars().count();
            start < basis.end
                && end > basis.start
                && (c.name("unit").is_some()
                    || c.name("period").is_some()
                    || c.name("device").is_some())
        })
        .collect();
    let bare_candidates = if candidates.is_empty() {
        QUANTITY
            .captures_iter(&source.text)
            .filter(|c| {
                let Some(number) = c.name("number") else {
                    return false;
                };
                let m = c.get(0).unwrap();
                let before = source.text[..number.start()].chars().next_back();
                let after = source.text[m.end()..].chars().next();
                let start = source.text[..number.start()].chars().count();
                let end = source.text[..number.end()].chars().count();
                start >= basis.start
                    && end <= basis.end
                    && !before.is_some_and(|c| c.is_ascii_alphanumeric() || "@./:-".contains(c))
                    && (end == basis.end || after.is_none_or(|c| "）)、。,;: ]}".contains(c)))
                    && bounded_date(&source.text, number.start(), number.end()).is_none()
            })
            .count()
    } else {
        0
    };
    if candidates.len() + bare_candidates > 1 {
        Some("ambiguous_quantity_basis")
    } else if candidates.len() + bare_candidates == 1 {
        Some("quantity_basis_mismatch")
    } else {
        Some("unsupported_quantity_expression")
    }
}

/// Numbers in a text value that fixed rules classified as non-quantities.
pub(super) fn quantity_exemptions(item: &Item) -> Vec<serde_json::Value> {
    let AtomicValue::Text { text } = &item.value else {
        return vec![];
    };
    scan_text(text)
        .exemptions
        .into_iter()
        .map(|(reason, quote)| serde_json::json!({"item":item.id,"quote":quote,"reason":reason}))
        .collect()
}

pub(super) fn quantity_diagnostic(
    item: &Item,
    sources: &BTreeMap<&str, &Source>,
) -> Option<serde_json::Value> {
    use serde_json::json;
    let code = quantity_issue(item, sources)?;
    let mut result = json!({"code":code,"item":item.id});
    result["message"] = json!(match code {
        "quantity_interpretation_required" =>
            "The source-bound quantity is preserved, but its amount/unit interpretation requires explicit review. Do not convert it to text or invent a normalization.",
        "quantity_basis_mismatch" =>
            "basis must select the complete quantity phrase, including comparison/mean/period modifiers. Keep surrounding meaning in evidence, condition and statement; do not change its meaning to match the parser.",
        "ambiguous_quantity_basis" =>
            "basis contains multiple quantity phrases. Select the intended one explicitly; retain before/after values and other independent claims. The CLI will not choose a number from the submitted amount.",
        "quantity_source_mismatch" =>
            "The selected quantity disagrees with amount, unit, comparison or semantics. Retention/duration is scalar; recurring intervals are period. Do not infer a comparison absent from the source.",
        "unsupported_quantity_unit" =>
            "The unit or compound unit is not fully supported; do not shorten it to a recognized prefix.",
        "unsupported_quantity_expression" =>
            "No complete supported quantity expression matched. This is a validator limitation, not permission to change the source meaning.",
        _ => "Check the quantity fields and their exact source evidence.",
    });
    if let AtomicValue::Quantity {
        basis: Some(basis), ..
    } = &item.value
    {
        result["basis"] = json!(basis);
        result["actual"] = json!(item.value);
        if let Some(source) = sources.get(basis.source.as_str()) {
            let candidates: Vec<_> = QUANTITY.captures_iter(&source.text).filter_map(|c| {
                let m = c.get(0)?;
                let quote = m.as_str().trim();
                let start_byte = m.start() + m.as_str().find(quote)?;
                let start = source.text[..start_byte].chars().count();
                let end = start + quote.chars().count();
                if start >= basis.end || end <= basis.start { return None; }
                Some(json!({"source":basis.source,"start":start,"end":end,"quote":quote,
                    "number":c.name("number").map(|v|v.as_str()),"unit":c.name("unit").map(|v|v.as_str()),
                    "prefix":c.name("prefix").map(|v|v.as_str()),"suffix":c.name("suffix").map(|v|v.as_str()),
                    "note":"lexical candidate only; not an approved interpretation"}))
            }).collect();
            result["candidates"] = json!(candidates);
        }
    } else if let AtomicValue::QuantityExpression { basis } = &item.value {
        result["basis"] = json!(basis);
        result["actual"] = json!(item.value);
    }
    Some(result)
}

/// Only fixed time units; never convert calendar months/years or ambiguous storage units.
pub(super) fn canonical_unit(number: &str, unit: &str) -> (String, String) {
    let factor: u128 = match unit {
        "ミリ秒" | "ms" => 1,
        "秒" => 1000,
        "分" => 60000,
        "時間" => 3600000,
        _ => return (number.into(), unit.into()),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let Some(product) = format!("{whole}{fraction}")
        .parse::<u128>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
    else {
        return (number.into(), unit.into());
    };
    let mut digits = product.to_string();
    if !fraction.is_empty() {
        if digits.len() <= fraction.len() {
            digits = format!(
                "{}{}",
                "0".repeat(fraction.len() + 1 - digits.len()),
                digits
            );
        }
        digits.insert(digits.len() - fraction.len(), '.');
    }
    (decimal(&digits, 0), "ミリ秒".into())
}
