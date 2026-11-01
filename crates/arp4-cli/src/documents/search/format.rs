//! The format of indexed text, which search filters by: the effective font of
//! a cell or shape text, and the emphasis its style records. Text whose runs
//! differ is in parts, each the characters in one format: of each font
//! property the one value they show, and each mark they show.
use super::analyzer::normalize;
use crate::fonts::{AUTO, MIXED};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

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

/// The font properties of a format, each with the member of an extraction
/// font it comes from.
const FONT_KEYS: [(&str, &str); 5] = [
    ("color", "color"),
    ("theme", "color_theme"),
    ("latin", "latin"),
    ("east_asian", "east_asian"),
    ("size", "size"),
];

/// The ranges of `[start, end]` pairs or `start`/`end` objects `shift`
/// characters on, cut at `length`.
fn shifted(range: &Value, shift: usize, length: usize) -> Option<Range<usize>> {
    let (start, end) = match range {
        Value::Array(pair) => (pair.first()?.as_u64()?, pair.get(1)?.as_u64()?),
        _ => (range["start"].as_u64()?, range["end"].as_u64()?),
    };
    let at = |offset: u64| (offset as usize).saturating_add(shift).min(length);
    Some(at(start)..at(end))
}

/// The parts of text `length` characters long with `font` (an extraction
/// font, null for none) and `style` (an extraction style, null for none), as
/// the index stores them: the characters of each part, and its format, which
/// holds the value of each font property its characters show, none that
/// cannot be resolved, and true for each mark they show. The font's runs and
/// the marks in part lie `shift` characters into the text, or `None` when
/// they cannot be placed in it; text they do not place shows only what the
/// whole text does. Parts in no format are left out, and parts next to each
/// other in one format are one.
pub(super) fn of(
    font: &Value,
    style: &Value,
    length: usize,
    shift: Option<usize>,
) -> Vec<(Range<usize>, Value)> {
    let runs: Vec<(Range<usize>, &Value)> = shift
        .into_iter()
        .flat_map(|shift| {
            font["runs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(move |run| Some((shifted(run, shift, length)?, run)))
        })
        .collect();
    let partial: Vec<(&str, Vec<Range<usize>>)> = shift
        .into_iter()
        .flat_map(|shift| {
            style["partial"]
                .as_object()
                .into_iter()
                .flatten()
                .map(move |(mark, ranges)| {
                    let ranges = ranges.as_array().into_iter().flatten();
                    let placed = ranges.filter_map(|r| shifted(r, shift, length)).collect();
                    (mark.as_str(), placed)
                })
        })
        .collect();
    let mut cuts = BTreeSet::from([0, length]);
    for range in runs
        .iter()
        .map(|(range, _)| range)
        .chain(partial.iter().flat_map(|(_, ranges)| ranges))
    {
        cuts.extend([range.start, range.end]);
    }
    let cuts: Vec<usize> = cuts.into_iter().collect();
    let mut parts: Vec<(Range<usize>, Value)> = vec![];
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let within = |range: &Range<usize>| range.start <= start && start < range.end;
        let run = runs
            .iter()
            .find(|(range, _)| within(range))
            .map(|(_, run)| *run);
        let mut format = Map::new();
        for (key, from) in FONT_KEYS {
            // The theme slot of a mixed color is each run's.
            let mixed = font[from] == MIXED || (from == "color_theme" && font["color"] == MIXED);
            let value = match run {
                Some(run) if mixed => &run[from],
                _ if mixed => &Value::Null,
                _ => &font[from],
            };
            if !value.is_null() {
                format.insert(key.into(), value.clone());
            }
        }
        for key in MARKS {
            // A style records the fill a cell has by number, 0 for none.
            let marked = match key {
                "fill" => style[key].as_u64().is_some_and(|fill| fill != 0),
                _ => {
                    style[key] == true
                        || partial
                            .iter()
                            .any(|(mark, ranges)| *mark == key && ranges.iter().any(within))
                }
            };
            if marked {
                format.insert(key.into(), json!(true));
            }
        }
        if format.is_empty() {
            continue;
        }
        let format = Value::Object(format);
        match parts.last_mut() {
            Some((last, seen)) if last.end == start && *seen == format => last.end = end,
            _ => parts.push((start..end, format)),
        }
    }
    parts
}

/// The family of a font color: by hue, or by lightness when the color has
/// little saturation. The automatic color, which applications show as black
/// on a white page, is black.
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
    /// `RRGGBB` or `auto`, as the extraction records it.
    Exact(String),
    Family(&'static str),
    Theme(&'static str),
}

/// Which cells a search keeps: every condition given holds for the same
/// characters of one cell's text. Several colors or fonts are alternatives;
/// every mark must show.
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
    /// `#`), a family, a theme slot or `auto`; font names; a size
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
                if lower == AUTO {
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
                    "unknown color {color}: use RRGGBB, {AUTO}, a family ({}) or a theme slot ({})",
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

    /// Whether characters in `format`, as [`of`] gives it, pass.
    pub(super) fn matches(&self, format: &Value) -> bool {
        let color = format["color"].as_str();
        let colored = self.colors.is_empty()
            || self.colors.iter().any(|spec| match spec {
                ColorSpec::Exact(exact) => color == Some(exact.as_str()),
                ColorSpec::Family(wanted) => color.and_then(family) == Some(*wanted),
                ColorSpec::Theme(slot) => format["theme"] == *slot,
            });
        let named = self.fonts.is_empty()
            || ["latin", "east_asian"].iter().any(|key| {
                format[*key]
                    .as_str()
                    .is_some_and(|name| self.fonts.contains(&normalize(name)))
            });
        let sized = self.size.is_none_or(|(low, high)| {
            format["size"].as_f64().is_some_and(|size| {
                low.is_none_or(|low| size >= low) && high.is_none_or(|high| size <= high)
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
        if !format[key].is_null() {
            add(property, &format[key]);
        }
    }
    for key in MARKS {
        if format[key] == true {
            add(key, &format[key]);
        }
    }
    found
}

/// The values of format properties the characters of a passage show, each
/// with the cells that show it, as an inventory counts them.
#[derive(Default)]
pub(super) struct Values(BTreeMap<(usize, String), i64>);

impl Values {
    /// Adds the values of the `formats` of the parts of a source's text, a
    /// cell when `cell`: a fragment continuing a cell counts none.
    pub fn add<'a>(&mut self, formats: impl Iterator<Item = &'a Value>, cell: bool) {
        let found: BTreeSet<_> = formats.flat_map(properties).collect();
        for value in found {
            *self.0.entry(value).or_default() += i64::from(cell);
        }
    }

    /// The values as the index stores them, `[property, value, cells]`
    /// each; `None` for none.
    pub fn stored(&self) -> Option<String> {
        let rows: Vec<Value> = self
            .0
            .iter()
            .map(|((position, value), cells)| {
                let value: Value = serde_json::from_str(value).expect("JSON property value");
                json!([PROPERTIES[*position], value, cells])
            })
            .collect();
        (!rows.is_empty()).then(|| Value::Array(rows).to_string())
    }
}

/// Each value of each property the `rows` of the index hold, as `(passage,
/// values)` with the values as [`Values::stored`] gives them: the passages
/// it occurs in (`hits`, what a search filtering by it alone returns) and the
/// cells that show it (`cells`). Properties in [`PROPERTIES`] order, then the
/// most hits first.
pub(super) fn inventory(rows: impl IntoIterator<Item = (i64, String)>) -> Result<Vec<Value>> {
    let mut counts: BTreeMap<(usize, String), (BTreeSet<i64>, i64)> = BTreeMap::new();
    for (passage, values) in rows {
        for row in serde_json::from_str::<Vec<(String, Value, i64)>>(&values)? {
            let (property, value, cells) = row;
            let position = PROPERTIES
                .iter()
                .position(|p| *p == property)
                .with_context(|| format!("search index holds unknown property {property}"))?;
            let (passages, count) = counts.entry((position, value.to_string())).or_default();
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
    Ok(items
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
        .collect())
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
    fn every_condition_holds_for_the_same_characters() {
        let filter = |colors: &[&str], fonts: &[&str], size: Option<&str>, bold: bool| {
            let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let marks: &[&str] = if bold { &["bold"] } else { &[] };
            FormatFilter::new(&owned(colors), &owned(fonts), size, marks).unwrap()
        };
        let parts = of(
            &json!({"color":"C00000","color_theme":"accent2","latin":"Arial","east_asian":"ＭＳ ゴシック","size":10.5}),
            &json!({"bold":true}),
            4,
            Some(0),
        );
        let red = json!({"color":"C00000","theme":"accent2","latin":"Arial",
            "east_asian":"ＭＳ ゴシック","size":10.5,"bold":true});
        assert_eq!(parts, vec![(0..4, red.clone())]);
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
        // A property that cannot be resolved matches nothing.
        let no_size = of(
            &json!({"size":null,"color":"FF0000"}),
            &Value::Null,
            1,
            Some(0),
        );
        assert_eq!(no_size, vec![(0..1, json!({"color":"FF0000"}))]);
        assert!(!filter(&[], &[], Some("..12"), false).matches(&no_size[0].1));
        assert!(!filter(&["red"], &[], None, true).matches(&no_size[0].1));
        // Text whose middle letter alone is red, 14 points and bold is in
        // three parts; every condition must hold for one of them.
        let parts = of(
            &json!({"color":"mixed","latin":"Arial","size":"mixed",
                "runs":[{"start":0,"end":1,"color":"auto","size":10.5},
                    {"start":1,"end":2,"color":"FF0000","color_theme":"accent2","size":14},
                    {"start":2,"end":3,"color":"auto","size":10.5}]}),
            &json!({"bold":false,"partial":{"bold":[[1,2]]}}),
            3,
            Some(0),
        );
        let plain = json!({"color":"auto","latin":"Arial","size":10.5});
        let marked =
            json!({"color":"FF0000","theme":"accent2","latin":"Arial","size":14,"bold":true});
        assert_eq!(
            parts,
            vec![
                (0..1, plain.clone()),
                (1..2, marked.clone()),
                (2..3, plain.clone())
            ]
        );
        let any = |filter: FormatFilter| parts.iter().any(|(_, format)| filter.matches(format));
        assert!(any(filter(&["red"], &[], Some("12.."), true)));
        assert!(any(filter(&["black"], &["arial"], Some("..11"), false)));
        assert!(any(filter(&["accent2"], &[], None, false)));
        assert!(!any(filter(&["blue"], &[], None, false)));
        assert!(!any(filter(&[], &[], Some("15.."), false)));
        assert!(!any(filter(&["red"], &[], Some("..11"), false)));
        assert!(!any(filter(&["black"], &[], None, true)));
        for (colors, size) in [
            (&["crimson"][..], None),
            (&["mixed"][..], None),
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
    fn parts_follow_the_runs_where_the_text_shows_them() {
        let font = json!({"color":"mixed","latin":"Arial",
            "runs":[{"start":0,"end":2,"color":"auto"},{"start":2,"end":4,"color":"FF0000"}]});
        let style = json!({"bold":false,"partial":{"italic":[[1,3]]}});
        // A number format's text section shows the value after a prefix of
        // three characters; the prefix has no run.
        assert_eq!(
            of(&font, &style, 7, Some(3)),
            vec![
                (0..3, json!({"latin":"Arial"})),
                (3..4, json!({"color":"auto","latin":"Arial"})),
                (4..5, json!({"color":"auto","latin":"Arial","italic":true})),
                (
                    5..6,
                    json!({"color":"FF0000","latin":"Arial","italic":true})
                ),
                (6..7, json!({"color":"FF0000","latin":"Arial"})),
            ]
        );
        // Runs that cannot be placed leave only what the whole text shows.
        assert_eq!(
            of(&font, &style, 5, None),
            vec![(0..5, json!({"latin":"Arial"}))]
        );
        // Text in no format has no parts; parts next to each other in one
        // format are one.
        assert_eq!(
            of(&Value::Null, &json!({"bold":false,"fill":0}), 3, Some(0)),
            vec![]
        );
        assert_eq!(
            of(
                &json!({"color":"mixed","runs":[{"start":0,"end":1,"color":"auto"},
                    {"start":2,"end":3,"color":"auto"}]}),
                &Value::Null,
                3,
                Some(0)
            ),
            vec![
                (0..1, json!({"color":"auto"})),
                (2..3, json!({"color":"auto"}))
            ]
        );
    }

    #[test]
    fn marks_come_from_the_style_and_must_all_show() {
        let parts = of(
            &json!({"color":"auto"}),
            &json!({"bold":false,"fill":3,"border":0,"strike":true,"partial":{"italic":[[0,1]]}}),
            2,
            Some(0),
        );
        // A mark only part of the text shows is a mark of that part.
        assert_eq!(
            parts,
            vec![
                (
                    0..1,
                    json!({"color":"auto","italic":true,"strike":true,"fill":true})
                ),
                (1..2, json!({"color":"auto","strike":true,"fill":true})),
            ]
        );
        // No fill is 0; a style without emphasis gives nothing to filter by.
        assert_eq!(
            of(
                &Value::Null,
                &json!({"bold":false,"fill":0,"border":1}),
                2,
                Some(0)
            ),
            vec![]
        );
        let marked = |marks: &[&str]| FormatFilter::new(&[], &[], None, marks).unwrap();
        assert!(marked(&["strike", "italic"]).matches(&parts[0].1));
        assert!(!marked(&["strike", "italic"]).matches(&parts[1].1));
        assert!(marked(&["fill"]).matches(&parts[1].1));
        assert!(!marked(&["italic", "underline"]).matches(&parts[0].1));
        assert!(!marked(&["highlight"]).matches(&parts[0].1));
        // The marks are pinned in one order, whatever order they are given in.
        assert_eq!(marked(&["strike", "italic"]), marked(&["italic", "strike"]));
        assert_eq!(
            marked(&["strike", "italic"]).describe()["marks"],
            json!(["italic", "strike"])
        );
    }

    #[test]
    fn an_inventory_counts_each_passage_once_per_value() {
        let bold = json!({"color":"FF0000","latin":"Arial","east_asian":"Arial","bold":true});
        let red = json!({"color":"FF0000","latin":"Arial","east_asian":"Arial"});
        let black = json!({"color":"auto","latin":"Arial","east_asian":"Arial"});
        // Passage 1: a cell bold and red, and a red one; passage 2: four
        // cells red in part, and a fragment continuing one of them.
        let mut first = Values::default();
        first.add([&bold].into_iter(), true);
        first.add([&red].into_iter(), true);
        let mut second = Values::default();
        for _ in 0..4 {
            second.add([&black, &red, &black].into_iter(), true);
        }
        second.add([&red].into_iter(), false);
        let items =
            inventory([(1, first.stored().unwrap()), (2, second.stored().unwrap())]).unwrap();
        // A cell red in part counts once as red, and once as each other
        // value it shows.
        assert_eq!(
            items,
            vec![
                json!({"property":"color","value":"FF0000","family":"red","hits":2,"cells":6}),
                json!({"property":"color","value":"auto","family":"black","hits":1,"cells":4}),
                json!({"property":"font","value":"Arial","hits":2,"cells":6}),
                json!({"property":"bold","value":true,"hits":1,"cells":1}),
            ]
        );
        assert_eq!(Values::default().stored(), None);
    }
}
