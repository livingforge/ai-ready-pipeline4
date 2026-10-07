//! Font name, size and colour of document text as readers see them: every
//! property is resolved through the styles and themes it inherits from. An
//! element holds runs; a property its runs disagree on is `mixed`, and one that
//! cannot be resolved is null.
use crate::data::string;
use anyhow::{Context, Result, bail, ensure};
use roxmltree::{Document, Node};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// The editable font properties, in the order of the `font` contract.
pub const PROPERTIES: [&str; 4] = ["latin", "east_asian", "size", "color"];
/// A property whose runs differ.
pub const MIXED: &str = "mixed";
/// The automatic text colour, which the application chooses.
pub const AUTO: &str = "auto";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    Auto,
    /// `RRGGBB`, with the theme slot (`dk1`, `accent1`, …) it was derived from.
    Rgb {
        rgb: String,
        theme: Option<String>,
    },
}

impl Color {
    pub fn rgb(rgb: impl Into<String>) -> Self {
        Self::Rgb {
            rgb: rgb.into(),
            theme: None,
        }
    }
    fn json(&self) -> Value {
        match self {
            Self::Auto => json!(AUTO),
            Self::Rgb { rgb, .. } => json!(rgb),
        }
    }
    fn theme(&self) -> Option<&str> {
        match self {
            Self::Rgb { theme, .. } => theme.as_deref(),
            Self::Auto => None,
        }
    }
}

/// The font of one run; `None` where it is not set at this level or cannot be
/// resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFont {
    pub latin: Option<String>,
    pub east_asian: Option<String>,
    pub size: Option<f64>,
    pub color: Option<Color>,
}

impl RunFont {
    /// This font with what it leaves unset taken from `inherited`.
    pub fn or(self, inherited: &RunFont) -> RunFont {
        RunFont {
            latin: self.latin.or_else(|| inherited.latin.clone()),
            east_asian: self.east_asian.or_else(|| inherited.east_asian.clone()),
            size: self.size.or(inherited.size),
            color: self.color.or_else(|| inherited.color.clone()),
        }
    }
}

/// A size in points as JSON: whole sizes as integers, so that `11` written by
/// hand and `11` read from a file compare equal.
pub fn size_value(points: f64) -> Value {
    let rounded = (points * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 && rounded.abs() < 1e15 {
        json!(rounded as i64)
    } else {
        json!(rounded)
    }
}

/// The font of an element from the fonts of its runs. With no runs, the
/// element shows `empty` (such as a cell's own font).
pub fn element_font(runs: &[RunFont], empty: &RunFont) -> Value {
    let runs = if runs.is_empty() {
        std::slice::from_ref(empty)
    } else {
        runs
    };
    fn agreed<T: PartialEq + Clone>(values: impl Iterator<Item = Option<T>>) -> Agreement<T> {
        let mut first = None;
        for value in values {
            match &first {
                None => first = Some(value),
                Some(seen) if *seen != value => return Agreement::Mixed,
                Some(_) => {}
            }
        }
        match first.flatten() {
            Some(value) => Agreement::Same(value),
            None => Agreement::Unknown,
        }
    }
    let text = |agreement: Agreement<String>| match agreement {
        Agreement::Same(value) => json!(value),
        Agreement::Mixed => json!(MIXED),
        Agreement::Unknown => Value::Null,
    };
    let mut font = Map::new();
    font.insert(
        "latin".into(),
        text(agreed(runs.iter().map(|r| r.latin.clone()))),
    );
    font.insert(
        "east_asian".into(),
        text(agreed(runs.iter().map(|r| r.east_asian.clone()))),
    );
    font.insert(
        "size".into(),
        match agreed(
            runs.iter()
                .map(|r| r.size.map(|s| (s * 100.0).round() as i64)),
        ) {
            Agreement::Same(hundredths) => size_value(hundredths as f64 / 100.0),
            Agreement::Mixed => json!(MIXED),
            Agreement::Unknown => Value::Null,
        },
    );
    match agreed(runs.iter().map(|r| r.color.clone())) {
        Agreement::Same(color) => {
            font.insert("color".into(), color.json());
            if let Some(theme) = color.theme() {
                font.insert("color_theme".into(), json!(theme));
            }
        }
        Agreement::Mixed => {
            font.insert("color".into(), json!(MIXED));
        }
        Agreement::Unknown => {
            font.insert("color".into(), Value::Null);
        }
    }
    Value::Object(font)
}

enum Agreement<T> {
    Same(T),
    Mixed,
    Unknown,
}

/// A requested font change: the properties to set, each to one value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontEdit {
    pub latin: Option<String>,
    pub east_asian: Option<String>,
    pub size: Option<f64>,
    /// `RRGGBB` or `auto`.
    pub color: Option<String>,
}

impl FontEdit {
    /// Reads the `after` of a font edit, which names only what changes.
    pub fn parse(after: &Value) -> Result<Self> {
        let object = after.as_object().context("font after must be an object")?;
        ensure!(!object.is_empty(), "font edit changes no property");
        let name = |key: &str| -> Result<Option<String>> {
            object
                .get(key)
                .map(|value| {
                    let name =
                        string(value).with_context(|| format!("font {key} must be a font name"))?;
                    ensure!(
                        !name.trim().is_empty() && name != MIXED && name.len() <= 255,
                        "font {key} must be one nonempty font name"
                    );
                    ensure!(
                        !name.chars().any(char::is_control),
                        "font {key} must not contain control characters"
                    );
                    Ok(name.to_owned())
                })
                .transpose()
        };
        let size = object
            .get("size")
            .map(|value| {
                let size = value
                    .as_f64()
                    .context("font size must be a number of points")?;
                ensure!(
                    (1.0..=4000.0).contains(&size) && (size * 100.0).fract().abs() < 1e-9,
                    "font size must be 1 to 4000 points in steps of 0.01"
                );
                Ok(size)
            })
            .transpose()?;
        let color = object
            .get("color")
            .map(|value| {
                let color = string(value).context("font color must be RRGGBB or auto")?;
                ensure!(
                    color == AUTO
                        || (color.len() == 6
                            && color
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))),
                    "font color must be six uppercase hexadecimal digits (RRGGBB) or auto"
                );
                Ok(color.to_owned())
            })
            .transpose()?;
        if let Some(key) = object
            .keys()
            .find(|key| !PROPERTIES.contains(&key.as_str()))
        {
            bail!("font {key} cannot be edited; edit latin, east_asian, size or color");
        }
        Ok(Self {
            latin: name("latin")?,
            east_asian: name("east_asian")?,
            size,
            color,
        })
    }

    /// `font` (an element's font) with this edit applied.
    pub fn applied(&self, font: &Value) -> Value {
        let mut out = font.clone();
        if let Some(latin) = &self.latin {
            out["latin"] = json!(latin);
        }
        if let Some(east_asian) = &self.east_asian {
            out["east_asian"] = json!(east_asian);
        }
        if let Some(size) = self.size {
            out["size"] = size_value(size);
        }
        if let Some(color) = &self.color {
            out["color"] = json!(color);
            if let Some(object) = out.as_object_mut() {
                object.remove("color_theme");
            }
        }
        out
    }

    /// The edit from `before` to `after`, two element fonts: the properties
    /// `after` sets to one value that `before` does not have.
    pub fn between(before: &Value, after: &Value) -> Result<Self> {
        let mut changed = Map::new();
        for key in PROPERTIES {
            if !same(&before[key], &after[key]) && !after[key].is_null() && after[key] != MIXED {
                changed.insert(key.to_owned(), after[key].clone());
            }
        }
        if changed.is_empty() {
            return Ok(Self::default());
        }
        Self::parse(&Value::Object(changed))
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The changed properties as JSON, as a font change reports them.
    pub fn json(&self) -> Value {
        let mut out = Map::new();
        if let Some(latin) = &self.latin {
            out.insert("latin".into(), json!(latin));
        }
        if let Some(east_asian) = &self.east_asian {
            out.insert("east_asian".into(), json!(east_asian));
        }
        if let Some(size) = self.size {
            out.insert("size".into(), size_value(size));
        }
        if let Some(color) = &self.color {
            out.insert("color".into(), json!(color));
        }
        Value::Object(out)
    }
}

/// Whether two font property values are equal, numbers by value.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 0.005,
        _ => a == b,
    }
}

/// Whether two element fonts agree on every editable property.
pub fn same_font(a: &Value, b: &Value) -> bool {
    PROPERTIES.iter().all(|key| same(&a[*key], &b[*key]))
}

/// The colours and fonts of a document theme (`a:theme`).
#[derive(Clone, Debug, Default)]
pub struct Theme {
    colors: BTreeMap<String, String>,
    /// Latin and East Asian typefaces of the major (headings) and minor (body) fonts.
    major: (Option<String>, Option<String>),
    minor: (Option<String>, Option<String>),
}

impl Theme {
    pub fn parse(xml: &str) -> Result<Self> {
        let doc = Document::parse(xml)?;
        let mut theme = Self::default();
        if let Some(scheme) = doc
            .descendants()
            .find(|n| n.has_tag_name((DRAWING, "clrScheme")))
        {
            for slot in scheme.children().filter(Node::is_element) {
                let color = slot.children().find(Node::is_element).and_then(|c| {
                    match c.tag_name().name() {
                        "srgbClr" => c.attribute("val"),
                        "sysClr" => c.attribute("lastClr"),
                        _ => None,
                    }
                });
                if let Some(color) = color.filter(|c| c.len() == 6) {
                    theme.colors.insert(
                        slot.tag_name().name().to_owned(),
                        color.to_ascii_uppercase(),
                    );
                }
            }
        }
        let typefaces = |name: &str| -> (Option<String>, Option<String>) {
            let Some(font) = doc.descendants().find(|n| n.has_tag_name((DRAWING, name))) else {
                return (None, None);
            };
            let face = |tag: &str| {
                font.children()
                    .find(|n| n.has_tag_name((DRAWING, tag)))
                    .and_then(|n| n.attribute("typeface"))
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
            };
            // An empty East Asian typeface leaves the choice to the script; the
            // documents ARP reads are Japanese.
            let east_asian = face("ea").or_else(|| {
                font.children()
                    .find(|n| {
                        n.has_tag_name((DRAWING, "font")) && n.attribute("script") == Some("Jpan")
                    })
                    .and_then(|n| n.attribute("typeface"))
                    .filter(|t| !t.is_empty())
                    .map(str::to_owned)
            });
            (face("latin"), east_asian)
        };
        theme.major = typefaces("majorFont");
        theme.minor = typefaces("minorFont");
        Ok(theme)
    }

    /// The colour of a theme slot (`dk1`, `lt1`, …, `accent6`, `hlink`, `folHlink`).
    pub fn color(&self, slot: &str) -> Option<&str> {
        self.colors.get(slot).map(String::as_str)
    }

    /// A typeface, resolving the theme references `+mj-lt`, `+mn-ea` and so on.
    pub fn typeface(&self, face: &str) -> Option<String> {
        let (scheme, script) = match face {
            "+mj-lt" => (&self.major, false),
            "+mj-ea" => (&self.major, true),
            "+mn-lt" => (&self.minor, false),
            "+mn-ea" => (&self.minor, true),
            "+mj-cs" | "+mn-cs" => return None,
            "" => return None,
            face => return Some(face.to_owned()),
        };
        if script {
            scheme.1.clone()
        } else {
            scheme.0.clone()
        }
    }

    /// The major (`true`) or minor theme font for a run.
    pub fn font(&self, major: bool) -> RunFont {
        let scheme = if major { &self.major } else { &self.minor };
        RunFont {
            latin: scheme.0.clone(),
            east_asian: scheme.1.clone(),
            ..RunFont::default()
        }
    }
}

fn rgb_components(rgb: &str) -> Option<[f64; 3]> {
    if rgb.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(&rgb[i..i + 2], 16)
            .ok()
            .map(|v| f64::from(v) / 255.0)
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn rgb_hex([r, g, b]: [f64; 3]) -> String {
    let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b))
}

fn to_hsl([r, g, b]: [f64; 3]) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f64::EPSILON {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    [h, s, l]
}

fn from_hsl([h, s, l]: [f64; 3]) -> [f64; 3] {
    if s == 0.0 {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0)]
}

/// An Excel or Word theme colour with a tint (`tint`, -1 to 1) applied to
/// its luminance, as Office does.
pub fn tinted(rgb: &str, tint: f64) -> String {
    let Some(components) = rgb_components(rgb) else {
        return rgb.to_owned();
    };
    if tint == 0.0 {
        return rgb.to_ascii_uppercase();
    }
    let [h, s, l] = to_hsl(components);
    let l = if tint < 0.0 {
        l * (1.0 + tint)
    } else {
        l * (1.0 - tint) + tint
    };
    rgb_hex(from_hsl([h, s, l.clamp(0.0, 1.0)]))
}

/// Word's `themeShade` and `themeTint` (hexadecimal 00–FF): a shade darkens
/// and a tint lightens the theme colour's luminance.
pub fn word_theme_color(rgb: &str, tint: Option<&str>, shade: Option<&str>) -> String {
    let ratio = |value: Option<&str>| {
        value
            .and_then(|v| u8::from_str_radix(v, 16).ok())
            .map(|v| f64::from(v) / 255.0)
    };
    match (ratio(tint), ratio(shade)) {
        (_, Some(shade)) => tinted(rgb, shade - 1.0),
        (Some(tint), None) => tinted(rgb, 1.0 - tint),
        (None, None) => rgb.to_ascii_uppercase(),
    }
}

fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// How DrawingML colours resolve in a part: the theme, the colour map from
/// the scheme names text uses (`tx1`, `bg1`, …) to theme slots, and the
/// colour a shape style supplies for `phClr`.
pub struct ColorContext<'a> {
    pub theme: Option<&'a Theme>,
    pub map: &'a BTreeMap<String, String>,
    pub placeholder: Option<&'a Color>,
}

/// The default colour map (`p:clrMap`) of Office documents.
pub fn default_color_map() -> BTreeMap<String, String> {
    [
        ("bg1", "lt1"),
        ("tx1", "dk1"),
        ("bg2", "lt2"),
        ("tx2", "dk2"),
    ]
    .into_iter()
    .chain(
        [
            "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink",
        ]
        .map(|slot| (slot, slot)),
    )
    .map(|(name, slot)| (name.to_owned(), slot.to_owned()))
    .collect()
}

/// The colour of a DrawingML colour element (`a:srgbClr`, `a:schemeClr`, …)
/// with its modifiers (`lumMod`, `lumOff`, `tint`, `shade`, …) applied.
pub fn drawing_color(node: Node<'_, '_>, context: &ColorContext<'_>) -> Option<Color> {
    let val = node.attribute("val");
    let (rgb, theme) = match node.tag_name().name() {
        "srgbClr" => (val?.to_ascii_uppercase(), None),
        "sysClr" => (node.attribute("lastClr")?.to_ascii_uppercase(), None),
        "schemeClr" if val == Some("phClr") => match context.placeholder? {
            Color::Rgb { rgb, theme } => (rgb.clone(), theme.clone()),
            Color::Auto => return Some(Color::Auto),
        },
        "schemeClr" => {
            let name = val?;
            let slot = context.map.get(name).map_or(name, String::as_str);
            (
                context.theme?.color(slot)?.to_owned(),
                Some(slot.to_owned()),
            )
        }
        "prstClr" => (
            match val? {
                "black" => "000000",
                "white" => "FFFFFF",
                "red" => "FF0000",
                "green" => "008000",
                "blue" => "0000FF",
                "yellow" => "FFFF00",
                "gray" => "808080",
                _ => return None,
            }
            .to_owned(),
            None,
        ),
        "scrgbClr" => {
            let percent = |name: &str| -> Option<f64> {
                Some(node.attribute(name)?.parse::<f64>().ok()? / 100_000.0)
            };
            (
                rgb_hex([
                    gamma(percent("r")?),
                    gamma(percent("g")?),
                    gamma(percent("b")?),
                ]),
                None,
            )
        }
        _ => return None,
    };
    let mut components = rgb_components(&rgb)?;
    for modifier in node.children().filter(Node::is_element) {
        let Some(amount) = modifier
            .attribute("val")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100_000.0)
        else {
            continue;
        };
        match modifier.tag_name().name() {
            "lumMod" | "lumOff" | "satMod" | "satOff" => {
                let mut hsl = to_hsl(components);
                match modifier.tag_name().name() {
                    "lumMod" => hsl[2] *= amount,
                    "lumOff" => hsl[2] += amount,
                    "satMod" => hsl[1] *= amount,
                    _ => hsl[1] += amount,
                }
                hsl[1] = hsl[1].clamp(0.0, 1.0);
                hsl[2] = hsl[2].clamp(0.0, 1.0);
                components = from_hsl(hsl);
            }
            "shade" => components = components.map(|c| gamma(linear(c) * amount)),
            "tint" => components = components.map(|c| gamma(1.0 - (1.0 - linear(c)) * amount)),
            _ => {}
        }
    }
    Some(Color::Rgb {
        rgb: rgb_hex(components),
        theme,
    })
}

/// The font a DrawingML run property element (`a:rPr`, `a:defRPr`,
/// `a:endParaRPr`) sets.
pub fn drawing_run(properties: Node<'_, '_>, context: &ColorContext<'_>) -> RunFont {
    let face = |tag: &str| {
        properties
            .children()
            .find(|n| n.has_tag_name((DRAWING, tag)))
            .and_then(|n| n.attribute("typeface"))
            .map(|face| match context.theme {
                Some(theme) => theme.typeface(face),
                None => (!face.starts_with('+') && !face.is_empty()).then(|| face.to_owned()),
            })
    };
    let color = properties
        .children()
        .find(|n| n.has_tag_name((DRAWING, "solidFill")))
        .and_then(|fill| fill.children().find(Node::is_element))
        .and_then(|color| drawing_color(color, context));
    RunFont {
        latin: face("latin").flatten(),
        east_asian: face("ea").flatten(),
        size: properties
            .attribute("sz")
            .and_then(|sz| sz.parse::<f64>().ok())
            .map(|sz| sz / 100.0),
        color,
    }
}

/// The font a shape style (`p:style` / `xdr:style`) gives its text through
/// `a:fontRef`: a theme font and a colour.
pub fn style_font(style: Node<'_, '_>, context: &ColorContext<'_>) -> (RunFont, Option<Color>) {
    let Some(reference) = style
        .children()
        .find(|n| n.has_tag_name((DRAWING, "fontRef")))
    else {
        return (RunFont::default(), None);
    };
    let mut font = match (context.theme, reference.attribute("idx")) {
        (Some(theme), Some("major")) => theme.font(true),
        (Some(theme), Some("minor")) => theme.font(false),
        _ => RunFont::default(),
    };
    let color = reference
        .children()
        .find(Node::is_element)
        .and_then(|c| drawing_color(c, context));
    font.color = color.clone();
    (font, color)
}

/// The run properties a DrawingML list style (`a:lstStyle`, `p:titleStyle`, …)
/// gives paragraphs of `level` (1–9) through `a:lvlNpPr/a:defRPr`.
pub fn list_level(list: Option<Node<'_, '_>>, level: usize, context: &ColorContext<'_>) -> RunFont {
    list.and_then(|list| {
        list.children()
            .find(|n| n.has_tag_name((DRAWING, format!("lvl{level}pPr").as_str())))
    })
    .and_then(|level| {
        level
            .children()
            .find(|n| n.has_tag_name((DRAWING, "defRPr")))
    })
    .map(|properties| drawing_run(properties, context))
    .unwrap_or_default()
}

/// The level (1–9) of a DrawingML paragraph, from `a:pPr@lvl` (0–8).
pub fn paragraph_level(paragraph: Node<'_, '_>) -> usize {
    paragraph
        .children()
        .find(|n| n.has_tag_name((DRAWING, "pPr")))
        .and_then(|p| p.attribute("lvl"))
        .and_then(|l| l.parse::<usize>().ok())
        .map_or(1, |l| l.clamp(0, 8) + 1)
}

/// The fonts of the runs with text of a DrawingML paragraph (`a:r`, `a:fld`),
/// with what a run leaves unset from `inherited` for the paragraph's level, and
/// the font of its end (`a:endParaRPr`), which an empty paragraph shows.
pub fn paragraph_runs(
    paragraph: Node<'_, '_>,
    inherited: &dyn Fn(usize) -> RunFont,
    context: &ColorContext<'_>,
) -> (Vec<RunFont>, RunFont) {
    let base = inherited(paragraph_level(paragraph));
    let own = |run: Node<'_, '_>, tag: &str| {
        run.children()
            .find(|n| n.has_tag_name((DRAWING, tag)))
            .map(|p| drawing_run(p, context))
            .unwrap_or_default()
            .or(&base)
    };
    let runs = paragraph
        .children()
        .filter(|n| n.has_tag_name((DRAWING, "r")) || n.has_tag_name((DRAWING, "fld")))
        .filter(|run| {
            run.children()
                .find(|n| n.has_tag_name((DRAWING, "t")))
                .and_then(|t| t.text())
                .is_some_and(|t| !t.is_empty())
        })
        .map(|run| own(run, "rPr"))
        .collect();
    let end = paragraph
        .children()
        .find(|n| n.has_tag_name((DRAWING, "endParaRPr")))
        .map(|p| drawing_run(p, context).or(&base))
        .unwrap_or_else(|| base.clone());
    (runs, end)
}

/// The DrawingML children of run properties in schema order (CT_TextCharacterProperties).
const DRAWING_RUN_ORDER: [&str; 19] = [
    "ln",
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
    "effectLst",
    "effectDag",
    "highlight",
    "uLnTx",
    "uLn",
    "uFillTx",
    "uFill",
    "latin",
    "ea",
    "cs",
    "sym",
    "hlinkClick",
];
const DRAWING_FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

/// One child element of a property element: its local name and XML.
pub(crate) type Child = (String, String);

/// Inserts `child` before the first child that the schema `order` places after it.
pub(crate) fn insert_ordered(children: &mut Vec<Child>, child: Child, order: &[&str]) {
    let rank = |name: &str| order.iter().position(|n| *n == name).unwrap_or(order.len());
    let own = rank(&child.0);
    let at = children
        .iter()
        .position(|(name, _)| rank(name) > own)
        .unwrap_or(children.len());
    children.insert(at, child);
}

/// The children of `element` in `source` with their names.
pub(crate) fn children_of(source: &str, element: Node<'_, '_>) -> Vec<Child> {
    element
        .children()
        .filter(Node::is_element)
        .map(|c| (c.tag_name().name().to_owned(), source[c.range()].to_owned()))
        .collect()
}

/// The qualified tag name of an element in `source`, such as `a:rPr`.
pub fn qualified_name<'a>(source: &'a str, element: Node<'_, '_>) -> &'a str {
    source[element.range()][1..]
        .split([' ', '\t', '\r', '\n', '/', '>'])
        .next()
        .unwrap_or("")
}

/// The namespace prefix of a qualified name with its colon (`a:`), or nothing.
pub fn prefix_of(name: &str) -> &str {
    name.rfind(':').map_or("", |at| &name[..=at])
}

/// Escapes text for an XML attribute value.
pub fn attribute_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// The opening tag of `element` without its closing `>` or `/>`.
pub(crate) fn opening_tag<'a>(source: &'a str, element: Node<'_, '_>) -> Result<&'a str> {
    let raw = &source[element.range()];
    let end = raw.find('>').context("invalid XML element")?;
    Ok(raw[..end].trim_end_matches('/').trim_end())
}

/// `opening` (an opening tag without its end) with attribute `name` removed.
pub(crate) fn without_attribute(opening: &str, name: &str) -> Result<String> {
    let pattern = regex::Regex::new(&format!(
        r#"\s+{}\s*=\s*(?:"[^"]*"|'[^']*')"#,
        regex::escape(name)
    ))?;
    Ok(pattern.replace(opening, "").into_owned())
}

/// DrawingML run properties (`a:rPr`, `a:endParaRPr`, `a:defRPr`) with `edit`
/// applied. `element` is the existing properties, or `None` to create `tag`
/// (in the namespace prefix `prefix`) for a run that has none.
pub fn edited_drawing_run(
    source: &str,
    element: Option<Node<'_, '_>>,
    tag: &str,
    prefix: &str,
    edit: &FontEdit,
) -> Result<String> {
    let (mut opening, mut children, name) = match element {
        Some(element) => (
            opening_tag(source, element)?.to_owned(),
            children_of(source, element),
            qualified_name(source, element).to_owned(),
        ),
        None => (format!("<{prefix}{tag}"), vec![], format!("{prefix}{tag}")),
    };
    if let Some(size) = edit.size {
        opening = format!(
            "{} sz=\"{}\"",
            without_attribute(&opening, "sz")?,
            (size * 100.0).round() as i64
        );
    }
    let prefix = prefix_of(&name).to_owned();
    if let Some(color) = &edit.color {
        ensure!(
            color != AUTO,
            "shape and slide text has no automatic color; give the color as RRGGBB"
        );
        children.retain(|(child, _)| !DRAWING_FILLS.contains(&child.as_str()));
        let fill =
            format!("<{prefix}solidFill><{prefix}srgbClr val=\"{color}\"/></{prefix}solidFill>");
        insert_ordered(
            &mut children,
            ("solidFill".into(), fill),
            &DRAWING_RUN_ORDER,
        );
    }
    for (key, value) in [("latin", &edit.latin), ("ea", &edit.east_asian)] {
        if let Some(face) = value {
            children.retain(|(child, _)| child != key);
            insert_ordered(
                &mut children,
                (
                    key.into(),
                    format!("<{prefix}{key} typeface=\"{}\"/>", attribute_text(face)),
                ),
                &DRAWING_RUN_ORDER,
            );
        }
    }
    if children.is_empty() {
        return Ok(format!("{opening}/>"));
    }
    let content: String = children.into_iter().map(|(_, xml)| xml).collect();
    Ok(format!("{opening}>{content}</{name}>"))
}

/// Excel font properties (`font` in styles, `rPr` of rich text) with `edit`
/// applied. Their children may come in any order; Excel has one font name
/// (`name_tag`: `name` or `rFont`), so `latin` and `east_asian` must agree.
pub fn edited_excel_font(
    source: &str,
    element: Node<'_, '_>,
    name_tag: &str,
    edit: &FontEdit,
) -> Result<String> {
    let name = qualified_name(source, element).to_owned();
    let prefix = prefix_of(&name).to_owned();
    let opening = opening_tag(source, element)?;
    let mut children = children_of(source, element);
    if let Some(face) = excel_name(edit)? {
        children.retain(|(child, _)| child != name_tag && child != "scheme");
        children.push((
            name_tag.into(),
            format!("<{prefix}{name_tag} val=\"{}\"/>", attribute_text(&face)),
        ));
    }
    if let Some(size) = edit.size {
        children.retain(|(child, _)| child != "sz");
        children.push((
            "sz".into(),
            format!("<{prefix}sz val=\"{}\"/>", size_value(size)),
        ));
    }
    if let Some(color) = &edit.color {
        children.retain(|(child, _)| child != "color");
        if color != AUTO {
            children.push((
                "color".into(),
                format!("<{prefix}color rgb=\"FF{color}\"/>"),
            ));
        }
    }
    if children.is_empty() {
        return Ok(format!("{opening}/>"));
    }
    let content: String = children.into_iter().map(|(_, xml)| xml).collect();
    Ok(format!("{opening}>{content}</{name}>"))
}

/// The one font name an Excel font takes from `edit`.
pub fn excel_name(edit: &FontEdit) -> Result<Option<String>> {
    match (&edit.latin, &edit.east_asian) {
        (None, None) => Ok(None),
        (Some(latin), Some(east_asian)) if latin == east_asian => Ok(Some(latin.clone())),
        _ => {
            bail!("Excel has one font name for all text; set latin and east_asian to the same name")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_fonts_report_disagreeing_runs_as_mixed() {
        let run = |latin: &str, size: f64, color: &str| RunFont {
            latin: Some(latin.into()),
            east_asian: Some("游ゴシック".into()),
            size: Some(size),
            color: Some(Color::rgb(color)),
        };
        let font = element_font(
            &[run("Arial", 11.0, "FF0000"), run("Arial", 12.0, "FF0000")],
            &RunFont::default(),
        );
        assert_eq!(
            font,
            json!({"latin":"Arial","east_asian":"游ゴシック","size":"mixed","color":"FF0000"})
        );
        let empty = element_font(&[], &run("Calibri", 10.5, "000000"));
        assert_eq!(empty["size"], 10.5);
        assert_eq!(
            element_font(&[RunFont::default()], &RunFont::default())["latin"],
            Value::Null
        );
    }

    #[test]
    fn theme_colors_resolve_tints_and_drawing_modifiers() {
        assert_eq!(tinted("4472C4", 0.0), "4472C4");
        assert_eq!(tinted("000000", 0.5), "808080");
        assert_eq!(tinted("FFFFFF", -0.5), "808080");
        let theme = Theme::parse(&format!(
            r#"<a:theme xmlns:a="{DRAWING}"><a:themeElements><a:clrScheme name="x"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:srgbClr val="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme><a:fontScheme name="x"><a:majorFont><a:latin typeface="Aptos Display"/><a:ea typeface=""/><a:font script="Jpan" typeface="游ゴシック Light"/></a:majorFont><a:minorFont><a:latin typeface="Aptos"/><a:ea typeface="游明朝"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#
        ))
        .unwrap();
        assert_eq!(
            theme.typeface("+mj-ea").as_deref(),
            Some("游ゴシック Light")
        );
        assert_eq!(theme.typeface("+mn-ea").as_deref(), Some("游明朝"));
        let map = default_color_map();
        let context = ColorContext {
            theme: Some(&theme),
            map: &map,
            placeholder: None,
        };
        let xml = format!(
            r#"<a:rPr xmlns:a="{DRAWING}" sz="1050"><a:solidFill><a:schemeClr val="tx1"><a:lumMod val="50000"/><a:lumOff val="50000"/></a:schemeClr></a:solidFill><a:latin typeface="+mn-lt"/></a:rPr>"#
        );
        let doc = Document::parse(&xml).unwrap();
        let font = drawing_run(doc.root_element(), &context);
        assert_eq!(font.size, Some(10.5));
        assert_eq!(font.latin.as_deref(), Some("Aptos"));
        assert_eq!(
            font.color,
            Some(Color::Rgb {
                rgb: "808080".into(),
                theme: Some("dk1".into())
            })
        );
    }

    #[test]
    fn drawing_run_edits_keep_schema_order_and_other_properties() {
        let xml = format!(
            r#"<a:p xmlns:a="{DRAWING}"><a:rPr lang="ja-JP" sz="1100" b="1"><a:ln w="1"/><a:solidFill><a:schemeClr val="lt1"/></a:solidFill><a:ea typeface="游ゴシック"/><a:hlinkClick r:id="x" xmlns:r="r"/></a:rPr></a:p>"#
        );
        let doc = Document::parse(&xml).unwrap();
        let rpr = doc.root_element().first_element_child().unwrap();
        let edit = FontEdit {
            latin: Some("Arial".into()),
            size: Some(14.0),
            color: Some("FF0000".into()),
            ..FontEdit::default()
        };
        let edited = edited_drawing_run(&xml, Some(rpr), "rPr", "a:", &edit).unwrap();
        assert_eq!(
            edited,
            r#"<a:rPr lang="ja-JP" b="1" sz="1400"><a:ln w="1"/><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:latin typeface="Arial"/><a:ea typeface="游ゴシック"/><a:hlinkClick r:id="x" xmlns:r="r"/></a:rPr>"#
        );
        let created = edited_drawing_run(
            "",
            None,
            "rPr",
            "a:",
            &FontEdit {
                size: Some(9.5),
                ..FontEdit::default()
            },
        )
        .unwrap();
        assert_eq!(created, r#"<a:rPr sz="950"/>"#);
    }

    #[test]
    fn excel_font_edits_replace_name_size_and_color() {
        let xml = r#"<font><b/><sz val="11"/><color theme="1"/><name val="游ゴシック"/><family val="3"/><charset val="128"/><scheme val="minor"/></font>"#;
        let doc = Document::parse(xml).unwrap();
        let edit = FontEdit {
            latin: Some("Arial".into()),
            east_asian: Some("Arial".into()),
            size: Some(10.5),
            color: Some("1F3864".into()),
        };
        assert_eq!(
            edited_excel_font(xml, doc.root_element(), "name", &edit).unwrap(),
            r#"<font><b/><family val="3"/><charset val="128"/><name val="Arial"/><sz val="10.5"/><color rgb="FF1F3864"/></font>"#
        );
        assert!(
            excel_name(&FontEdit {
                latin: Some("Arial".into()),
                ..FontEdit::default()
            })
            .is_err()
        );
    }

    #[test]
    fn font_edits_parse_only_editable_single_values() {
        assert!(FontEdit::parse(&json!({})).is_err());
        assert!(FontEdit::parse(&json!({"size":"mixed"})).is_err());
        assert!(FontEdit::parse(&json!({"color":"ff0000"})).is_err());
        assert!(FontEdit::parse(&json!({"color_theme":"accent1"})).is_err());
        let edit = FontEdit::parse(&json!({"size":12,"color":"auto"})).unwrap();
        assert_eq!(edit.json(), json!({"size":12,"color":"auto"}));
        let font = json!({"latin":"A","east_asian":"B","size":11,"color":"4472C4","color_theme":"accent1"});
        assert_eq!(
            edit.applied(&font),
            json!({"latin":"A","east_asian":"B","size":12,"color":"auto"})
        );
        let between = FontEdit::between(
            &font,
            &json!({"latin":"A","east_asian":"B","size":11.0,"color":"FF0000"}),
        )
        .unwrap();
        assert_eq!(between.json(), json!({"color":"FF0000"}));
    }
}
