//! The format of indexed text, which search filters by: the effective font of
//! a cell or shape text, and the emphasis its style records. A cell's format
//! holds every value its runs show: of a property they disagree on, `mixed`
//! and the value of each run, and each mark some run shows.
use super::analyzer::normalize;
use crate::fonts::{AUTO, MIXED};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The families a color filter names, each a range of hues, or of lightness
/// for colors with little saturation.
pub const FAMILIES: [&str; 11] = [
    "red", "orange", "yellow", "green", "cyan", "blue", "purple", "pink", "gray", "black", "white",
];

/// The theme color slots a font color can come from.
const THEME_SLOTS: [&str; 12] = [
    "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6",
    "hlink", "folHlink",
];

/// The emphasis a filter can ask for, in the order an inventory lists it: each
/// is true in a format whose text shows it. `fill` is a cell painted behind
/// its text.
pub const MARKS: [&str; 6] = ["bold", "italic", "underline", "strike", "highlight", "fill"];

/// The properties an inventory lists, in its order.
const PROPERTIES: [&str; 10] = [
    "color",
    "theme",
    "font",
    "size",
    "bold",
    "italic",
    "underline",
    "strike",
    "highlight",
    "fill",
];

/// The format of text with `font` (an extraction font, null for none) and
/// `style` (an extraction style, null for none), as the index stores it: each
/// font property as the values the text shows, and each mark the whole text
/// or part of it shows. `None` when neither records anything.
pub(super) fn of(font: &Value, style: &Value) -> Option<Value> {
    let mut format = Map::new();
    for (key, from) in [
        ("color", "color"),
        ("theme", "color_theme"),
        ("latin", "latin"),
        ("east_asian", "east_asian"),
        ("size", "size"),
    ] {
        let mut values: Vec<Value> = vec![];
        let runs = font["runs"][from].as_array().into_iter().flatten();
        for value in std::iter::once(&font[from]).chain(runs) {
            if !value.is_null() && !values.contains(value) {
                values.push(value.clone());
            }
        }
        if !values.is_empty() {
            format.insert(key.into(), json!(values));
        }
    }
    let partial = style["partial"].as_array();
    for key in MARKS {
        // A style records the fill a cell has by number, 0 for none.
        let marked = match key {
            "fill" => style[key].as_u64().is_some_and(|fill| fill != 0),
            _ => style[key] == true || partial.is_some_and(|marks| marks.contains(&json!(key))),
        };
        if marked {
            format.insert(key.into(), json!(true));
        }
    }
    (!format.is_empty()).then_some(Value::Object(format))
}

/// The values a format holds of `key`.
fn values<'a>(format: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    format[key].as_array().into_iter().flatten()
}

/// The family of a font color: by hue, or by lightness when the color has
/// little saturation. The automatic color, which applications show as black
/// on a white page, is black; a mixed color has none.
pub fn family(color: &str) -> Option<&'static str> {
    if color == AUTO {
        return Some("black");
    }
    if color.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(color, 16).ok()?;
    let [r, g, b] = [16, 8, 0].map(|shift| f64::from((rgb >> shift) & 0xFF) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let lightness = (max + min) / 2.0;
    if lightness < 0.12 {
        return Some("black");
    }
    if lightness > 0.94 {
        return Some("white");
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    if saturation < 0.2 {
        return Some(if lightness < 0.25 {
            "black"
        } else if lightness > 0.85 {
            "white"
        } else {
            "gray"
        });
    }
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    Some(match hue {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 50.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 195.0 => "cyan",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    })
}

#[derive(Clone, Debug, PartialEq)]
enum ColorSpec {
    /// `RRGGBB`, `auto` or `mixed`, as the extraction records it.
    Exact(String),
    Family(&'static str),
    Theme(&'static str),
}

/// Which cells a search keeps: every condition given holds for some of one
/// cell's text, not necessarily the same part. Several colors or fonts are
/// alternatives; every mark must show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatFilter {
    colors: Vec<ColorSpec>,
    fonts: Vec<String>,
    size: Option<(Option<f64>, Option<f64>)>,
    /// In [`MARKS`] order.
    marks: Vec<&'static str>,
}

/// A size bound in points: positive and finite.
fn points(text: &str) -> Result<f64> {
    let size: f64 = text
        .trim()
        .parse()
        .with_context(|| format!("font size must be a number of points: {text}"))?;
    ensure!(
        size.is_finite() && size > 0.0,
        "font size must be a positive number of points: {text}"
    );
    Ok(size)
}

impl FormatFilter {
    /// A filter from command-line values: colors as `RRGGBB` (an optional
    /// `#`), a family, a theme slot, `auto` or `mixed`; font names; a size
    /// as `N`, `N..M`, `N..` or `..M` points; marks of [`MARKS`].
    pub fn new(
        colors: &[String],
        fonts: &[String],
        size: Option<&str>,
        marks: &[&str],
    ) -> Result<Self> {
        if let Some(unknown) = marks.iter().find(|mark| !MARKS.contains(mark)) {
            bail!("unknown mark {unknown}: use {}", MARKS.join(", "));
        }
        let colors = colors
            .iter()
            .map(|color| {
                let lower = color.trim().to_ascii_lowercase();
                let hex = lower.strip_prefix('#').unwrap_or(&lower);
                if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Ok(ColorSpec::Exact(hex.to_ascii_uppercase()));
                }
                if [AUTO, MIXED].contains(&lower.as_str()) {
                    return Ok(ColorSpec::Exact(lower));
                }
                if let Some(family) = FAMILIES.iter().find(|family| **family == lower) {
                    return Ok(ColorSpec::Family(family));
                }
                if let Some(slot) = THEME_SLOTS
                    .iter()
                    .find(|slot| slot.eq_ignore_ascii_case(&lower))
                {
                    return Ok(ColorSpec::Theme(slot));
                }
                bail!(
                    "unknown color {color}: use RRGGBB, {AUTO}, {MIXED}, a family ({}) or a theme slot ({})",
                    FAMILIES.join(", "),
                    THEME_SLOTS.join(", ")
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let fonts = fonts
            .iter()
            .map(|font| {
                let name = normalize(font);
                ensure!(!name.is_empty(), "font name must not be empty");
                Ok(name)
            })
            .collect::<Result<Vec<_>>>()?;
        let size = size
            .map(|size| -> Result<_> {
                let Some((low, high)) = size.split_once("..") else {
                    let size = points(size)?;
                    return Ok((Some(size), Some(size)));
                };
                let low = (!low.trim().is_empty()).then(|| points(low)).transpose()?;
                let high = (!high.trim().is_empty())
                    .then(|| points(high))
                    .transpose()?;
                ensure!(
                    low.is_some() || high.is_some(),
                    "font size range needs a bound: N..M, N.. or ..M"
                );
                ensure!(
                    low.zip(high).is_none_or(|(low, high)| low <= high),
                    "font size range runs backwards: {size}"
                );
                Ok((low, high))
            })
            .transpose()?;
        Ok(Self {
            colors,
            fonts,
            size,
            marks: MARKS
                .into_iter()
                .filter(|mark| marks.contains(mark))
                .collect(),
        })
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether text in `format`, as [`of`] gives it, passes.
    pub(super) fn matches(&self, format: &Value) -> bool {
        let colored = self.colors.is_empty()
            || self.colors.iter().any(|spec| match spec {
                ColorSpec::Exact(exact) => values(format, "color").any(|color| color == exact),
                ColorSpec::Family(wanted) => values(format, "color")
                    .any(|color| color.as_str().and_then(family) == Some(wanted)),
                ColorSpec::Theme(slot) => values(format, "theme").any(|theme| theme == slot),
            });
        let named = self.fonts.is_empty()
            || ["latin", "east_asian"].iter().any(|key| {
                values(format, key).any(|name| {
                    name.as_str()
                        .is_some_and(|name| self.fonts.contains(&normalize(name)))
                })
            });
        let sized = self.size.is_none_or(|(low, high)| {
            values(format, "size").any(|size| {
                size.as_f64().is_some_and(|size| {
                    low.is_none_or(|low| size >= low) && high.is_none_or(|high| size <= high)
                })
            })
        });
        colored && named && sized && self.marks.iter().all(|mark| format[*mark] == true)
    }

    /// The filter as a search revision pins it; null for none.
    pub(super) fn describe(&self) -> Value {
        if self.is_empty() {
            return Value::Null;
        }
        let colors: Vec<_> = self
            .colors
            .iter()
            .map(|spec| match spec {
                ColorSpec::Exact(exact) => exact.clone(),
                ColorSpec::Family(family) => format!("family:{family}"),
                ColorSpec::Theme(slot) => format!("theme:{slot}"),
            })
            .collect();
        json!({"colors":colors,"fonts":self.fonts,"size":self.size,"marks":self.marks})
    }
}

/// The properties of a format an inventory counts it under, each once.
fn properties(format: &Value) -> BTreeSet<(usize, String)> {
    let mut found = BTreeSet::new();
    let mut add = |property: &str, value: &Value| {
        let position = PROPERTIES.iter().position(|p| *p == property).unwrap();
        found.insert((position, value.to_string()));
    };
    for (property, key) in [
        ("color", "color"),
        ("theme", "theme"),
        ("font", "latin"),
        ("font", "east_asian"),
        ("size", "size"),
    ] {
        for value in values(format, key) {
            add(property, value);
        }
    }
    for key in MARKS {
        if format[key] == true {
            add(key, &format[key]);
        }
    }
    found
}

/// Each value of each property the `rows` of the index hold, as
/// `(passage, format, cells)`: the passages it occurs in (`hits`, what a
/// search filtering by it alone returns) and the cells (`cells`). Properties
/// in [`PROPERTIES`] order, then the most hits first.
pub(super) fn inventory<'a>(rows: impl IntoIterator<Item = (i64, &'a Value, i64)>) -> Vec<Value> {
    let mut counts: BTreeMap<(usize, String), (BTreeSet<i64>, i64)> = BTreeMap::new();
    for (passage, format, cells) in rows {
        for property in properties(format) {
            let (passages, count) = counts.entry(property).or_default();
            passages.insert(passage);
            *count += cells;
        }
    }
    let mut items: Vec<_> = counts.into_iter().collect();
    items.sort_by(|((a, a_value), (a_hits, _)), ((b, b_value), (b_hits, _))| {
        (a, std::cmp::Reverse(a_hits.len()), a_value).cmp(&(
            b,
            std::cmp::Reverse(b_hits.len()),
            b_value,
        ))
    });
    items
        .into_iter()
        .map(|((position, value), (passages, cells))| {
            let value: Value = serde_json::from_str(&value).expect("JSON property value");
            let mut item = json!({"property":PROPERTIES[position],"value":value,
                "hits":passages.len(),"cells":cells});
            if position == 0
                && let Some(family) = value.as_str().and_then(family)
            {
                item["family"] = json!(family);
            }
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_follow_the_standard_colors_of_office() {
        for (color, expected) in [
            ("C00000", "red"),
            ("FF0000", "red"),
            ("FFC000", "orange"),
            ("FFFF00", "yellow"),
            ("92D050", "green"),
            ("00B050", "green"),
            ("00B0F0", "blue"),
            ("0070C0", "blue"),
            ("002060", "blue"),
            ("7030A0", "purple"),
            ("FF00FF", "pink"),
            ("000000", "black"),
            ("262626", "black"),
            ("595959", "gray"),
            ("FFFFFF", "white"),
            ("auto", "black"),
        ] {
            assert_eq!(family(color), Some(expected), "{color}");
        }
        assert_eq!(family(MIXED), None);
    }

    #[test]
    fn every_condition_holds_for_one_format() {
        let filter = |colors: &[&str], fonts: &[&str], size: Option<&str>, bold: bool| {
            let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let marks: &[&str] = if bold { &["bold"] } else { &[] };
            FormatFilter::new(&owned(colors), &owned(fonts), size, marks).unwrap()
        };
        let red = of(
            &json!({"color":"C00000","color_theme":"accent2","latin":"Arial","east_asian":"ＭＳ ゴシック","size":10.5}),
            &json!({"bold":true}),
        )
        .unwrap();
        assert!(filter(&["red"], &[], None, false).matches(&red));
        assert!(filter(&["#c00000"], &[], None, false).matches(&red));
        assert!(filter(&["ACCENT2"], &[], None, false).matches(&red));
        assert!(!filter(&["FF0000"], &[], None, false).matches(&red));
        assert!(filter(&["blue", "red"], &[], None, false).matches(&red));
        // Font names compare as search text does: full-width letters and
        // spaces read as their half-width forms.
        assert!(filter(&[], &["MS ゴシック"], None, false).matches(&red));
        assert!(filter(&[], &["arial"], Some("10..11"), true).matches(&red));
        assert!(!filter(&["red"], &[], Some("11.."), false).matches(&red));
        let plain = |font: Value| of(&font, &Value::Null).unwrap();
        assert!(!filter(&[], &[], Some("..12"), false).matches(&plain(json!({"size":"mixed"}))));
        assert!(!filter(&["red"], &[], None, true).matches(&plain(json!({"color":"FF0000"}))));
        // Text in part red, in part 14 points and in part bold has those
        // formats, and each condition may hold for another part of it.
        let parts = of(
            &json!({"color":"mixed","latin":"Arial","size":"mixed",
                "runs":{"color":["auto","FF0000"],"color_theme":["accent2"],"size":[10.5,14]}}),
            &json!({"bold":false,"partial":["bold"]}),
        )
        .unwrap();
        assert!(filter(&["mixed"], &[], None, false).matches(&parts));
        assert!(filter(&["red"], &[], Some("12.."), true).matches(&parts));
        assert!(filter(&["black"], &["arial"], Some("..11"), false).matches(&parts));
        assert!(filter(&["accent2"], &[], None, false).matches(&parts));
        assert!(!filter(&["blue"], &[], None, false).matches(&parts));
        assert!(!filter(&[], &[], Some("15.."), false).matches(&parts));
        for (colors, size) in [
            (&["crimson"][..], None),
            (&[][..], Some("12..10")),
            (&[][..], Some("..")),
        ] {
            assert!(
                FormatFilter::new(
                    &colors.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                    &[],
                    size,
                    &[]
                )
                .is_err()
            );
        }
        assert!(FormatFilter::new(&[], &[], None, &["shadow"]).is_err());
    }

    #[test]
    fn marks_come_from_the_style_and_must_all_show() {
        let format = of(
            &json!({"color":"auto"}),
            &json!({"bold":false,"fill":3,"border":0,"strike":true,"partial":["italic"]}),
        )
        .unwrap();
        // A mark only part of the text shows is a mark of the text.
        assert_eq!(
            format,
            json!({"color":["auto"],"italic":true,"strike":true,"fill":true})
        );
        // No fill is 0; a style without emphasis gives nothing to filter by.
        assert_eq!(
            of(&Value::Null, &json!({"bold":false,"fill":0,"border":1})),
            None
        );
        let marked = |marks: &[&str]| FormatFilter::new(&[], &[], None, marks).unwrap();
        assert!(marked(&["strike", "italic"]).matches(&format));
        assert!(marked(&["fill"]).matches(&format));
        assert!(!marked(&["italic", "underline"]).matches(&format));
        assert!(!marked(&["highlight"]).matches(&format));
        // The marks are pinned in one order, whatever order they are given in.
        assert_eq!(marked(&["strike", "italic"]), marked(&["italic", "strike"]));
        assert_eq!(
            marked(&["strike", "italic"]).describe()["marks"],
            json!(["italic", "strike"])
        );
    }

    #[test]
    fn an_inventory_counts_each_passage_once_per_value() {
        let format = |font: Value, style: Value| of(&font, &style).unwrap();
        let (bold, red, parts) = (
            format(
                json!({"color":"FF0000","latin":"Arial","east_asian":"Arial"}),
                json!({"bold":true}),
            ),
            format(
                json!({"color":"FF0000","latin":"Arial","east_asian":"Arial"}),
                Value::Null,
            ),
            format(
                json!({"color":"mixed","latin":"Arial","east_asian":"Arial",
                    "runs":{"color":["auto","FF0000"]}}),
                Value::Null,
            ),
        );
        let items = inventory([(1, &bold, 2), (1, &red, 1), (2, &parts, 4)]);
        // Text in part red counts as red, and as each other color it shows.
        assert_eq!(
            items,
            vec![
                json!({"property":"color","value":"FF0000","family":"red","hits":2,"cells":7}),
                json!({"property":"color","value":"auto","family":"black","hits":1,"cells":4}),
                json!({"property":"color","value":"mixed","hits":1,"cells":4}),
                json!({"property":"font","value":"Arial","hits":2,"cells":7}),
                json!({"property":"bold","value":true,"hits":1,"cells":2}),
            ]
        );
    }
}
