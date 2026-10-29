//! The drawings of an Excel sheet or a PowerPoint slide: each shape in its
//! geometry with its fill, line and text, each connector as the line it
//! draws with its arrowheads, and each picture with its image, in the box
//! its sheet or slide gives it.
use super::*;

/// A box on the page, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// How a drawing that does not resolve its fill or line, or the place of its
/// text, is drawn where it is.
pub(super) struct Defaults {
    pub fill: &'static str,
    pub line: &'static str,
    /// Where the text stands across the shape (CSS `text-align`) and down it
    /// (`justify-content` of its column).
    pub horizontal: &'static str,
    pub vertical: &'static str,
}

impl Defaults {
    /// The text placed where a DrawingML text body puts it: `algn` (l, ctr,
    /// r, just, dist) across and `anchor` (t, ctr, b) down; what it leaves
    /// unsaid where `horizontal` and `vertical` put it.
    pub(super) fn placed(mut self, algn: Option<&str>, anchor: Option<&str>) -> Self {
        if let Some(algn) = algn {
            self.horizontal = match algn {
                "ctr" => "center",
                "r" => "right",
                "just" | "dist" => "justify",
                _ => "left",
            };
        }
        if let Some(anchor) = anchor {
            self.vertical = match anchor {
                "ctr" => "center",
                "b" => "flex-end",
                _ => "flex-start",
            };
        }
        self
    }
}

fn flag(value: &Value) -> bool {
    matches!(value.as_str(), Some("1" | "true")) || *value == true
}

/// The element of `drawing` in `rect`. `text` is the HTML of its text,
/// `mark` the attributes of a difference, `rotation` its turn in degrees.
pub(super) fn drawing(
    drawing: &Value,
    rect: Rect,
    text: &str,
    mark: &str,
    rotation: f64,
    assets: &Path,
    defaults: &Defaults,
) -> String {
    let place = |extra: &str| {
        let turn = if rotation.abs() > 0.01 {
            format!("transform:rotate({rotation:.2}deg);")
        } else {
            String::new()
        };
        format!(
            "left:{:.1}px;top:{:.1}px;width:{:.1}px;height:{:.1}px;{turn}{extra}",
            rect.x,
            rect.y,
            rect.w.max(0.0),
            rect.h.max(0.0)
        )
    };
    let name = drawing["name"].as_str().unwrap_or_default();
    match drawing["kind"].as_str() {
        Some("connector") => connector(drawing, rect, defaults),
        Some("picture") => match drawing["image"]["asset"]
            .as_str()
            .and_then(|asset| image_url(assets, asset))
        {
            Some(url) => format!(
                r#"<div class="shape" style="{}"><img src="{url}" alt="{}"></div>"#,
                place(""),
                escape(name)
            ),
            None => format!(
                r#"<div class="shape ph" style="{}">画像 {}</div>"#,
                place(""),
                escape(name)
            ),
        },
        Some("graphic") => format!(
            r#"<div class="shape ph" style="{}">{} {}</div>"#,
            place(""),
            if drawing["chart_part"].is_string() {
                "グラフ"
            } else {
                "図表"
            },
            escape(name)
        ),
        Some("group") => String::new(),
        _ => {
            let control = &drawing["control"];
            if control.is_object() {
                return form_control(control, rect, text, mark, &place(""));
            }
            let fill = match drawing["fill"].as_str() {
                Some("none") => "none".to_owned(),
                _ => color(&drawing["fill"]).unwrap_or_else(|| defaults.fill.to_owned()),
            };
            let (stroke, width) = line(drawing, defaults);
            let flip = transform_flips(drawing);
            let outline = geometry(drawing["geometry"].as_str().unwrap_or("rect"), rect);
            let scale = match flip {
                (false, false) => String::new(),
                (h, v) => format!(
                    r#" style="transform:scale({},{})""#,
                    if h { -1 } else { 1 },
                    if v { -1 } else { 1 }
                ),
            };
            let dash = dash(drawing, width);
            let align = format!(
                "justify-content:{};text-align:{};",
                defaults.vertical, defaults.horizontal
            );
            format!(
                r#"<div class="shape" style="{}"{mark}><svg viewBox="0 0 {:.1} {:.1}"{scale}><g fill="{fill}" stroke="{stroke}" stroke-width="{width:.2}"{dash}>{outline}</g></svg><div class="t" style="{align}{}">{text}</div></div>"#,
                place(""),
                rect.w.max(1.0),
                rect.h.max(1.0),
                font_css(&drawing["font"], &drawing["style"]),
            )
        }
    }
}

/// The colour and width in pixels of a drawing's line, `none` for none.
fn line(drawing: &Value, defaults: &Defaults) -> (String, f64) {
    let stroke = match drawing["line"]["color"].as_str() {
        Some("none") => "none".to_owned(),
        _ => color(&drawing["line"]["color"]).unwrap_or_else(|| defaults.line.to_owned()),
    };
    let width = drawing["line"]["width"].as_f64().unwrap_or(0.75) * 4.0 / 3.0;
    (stroke, width.max(0.75))
}

/// The SVG dash of a drawing's line (`a:prstDash`), empty for a solid one.
fn dash(drawing: &Value, width: f64) -> String {
    let pattern: &[f64] = match drawing["dash"].as_str() {
        Some("dot" | "sysDot") => &[1.0, 1.0],
        Some("dash" | "sysDash") => &[4.0, 3.0],
        Some("lgDash") => &[8.0, 3.0],
        Some("dashDot" | "sysDashDot") => &[4.0, 3.0, 1.0, 3.0],
        Some("lgDashDot") => &[8.0, 3.0, 1.0, 3.0],
        Some("lgDashDotDot" | "sysDashDotDot") => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
        _ => return String::new(),
    };
    let scaled: Vec<String> = pattern
        .iter()
        .map(|p| format!("{:.1}", p * width.max(1.0)))
        .collect();
    format!(r#" stroke-dasharray="{}""#, scaled.join(" "))
}

/// Whether a drawing is flipped horizontally and vertically.
fn transform_flips(drawing: &Value) -> (bool, bool) {
    let transform = &drawing["transform"];
    (flag(&transform["flipH"]), flag(&transform["flipV"]))
}

/// The SVG outline of a preset geometry in a box of `rect`'s size; a
/// geometry not drawn here is drawn as a rectangle.
fn geometry(name: &str, rect: Rect) -> String {
    let (w, h) = (rect.w.max(1.0), rect.h.max(1.0));
    let polygon = |points: &[(f64, f64)]| {
        let points: Vec<String> = points
            .iter()
            .map(|(x, y)| format!("{:.1},{:.1}", x * w, y * h))
            .collect();
        format!(r#"<polygon points="{}"/>"#, points.join(" "))
    };
    match name {
        "ellipse" | "flowChartConnector" | "flowChartOr" | "flowChartSummingJunction" => format!(
            r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}"/>"#,
            w / 2.0,
            h / 2.0,
            w / 2.0,
            h / 2.0
        ),
        "roundRect" | "flowChartAlternateProcess" | "snipRoundRect" | "round2SameRect" => {
            let r = w.min(h) * 0.17;
            format!(r#"<rect width="{w:.1}" height="{h:.1}" rx="{r:.1}"/>"#)
        }
        "flowChartTerminator" => {
            let r = h.min(w) / 2.0;
            format!(r#"<rect width="{w:.1}" height="{h:.1}" rx="{r:.1}"/>"#)
        }
        "diamond" | "flowChartDecision" => {
            polygon(&[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)])
        }
        "triangle" | "flowChartExtract" => polygon(&[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        "flowChartMerge" => polygon(&[(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)]),
        "rtTriangle" => polygon(&[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        "parallelogram" | "flowChartInputOutput" => {
            polygon(&[(0.2, 0.0), (1.0, 0.0), (0.8, 1.0), (0.0, 1.0)])
        }
        "trapezoid" | "flowChartManualOperation" => {
            polygon(&[(0.0, 0.0), (1.0, 0.0), (0.8, 1.0), (0.2, 1.0)])
        }
        "hexagon" | "flowChartPreparation" => polygon(&[
            (0.2, 0.0),
            (0.8, 0.0),
            (1.0, 0.5),
            (0.8, 1.0),
            (0.2, 1.0),
            (0.0, 0.5),
        ]),
        "octagon" => polygon(&[
            (0.3, 0.0),
            (0.7, 0.0),
            (1.0, 0.3),
            (1.0, 0.7),
            (0.7, 1.0),
            (0.3, 1.0),
            (0.0, 0.7),
            (0.0, 0.3),
        ]),
        "pentagon" | "homePlate" => {
            polygon(&[(0.0, 0.0), (0.8, 0.0), (1.0, 0.5), (0.8, 1.0), (0.0, 1.0)])
        }
        "chevron" => polygon(&[
            (0.0, 0.0),
            (0.8, 0.0),
            (1.0, 0.5),
            (0.8, 1.0),
            (0.0, 1.0),
            (0.2, 0.5),
        ]),
        "flowChartManualInput" => polygon(&[(0.0, 0.2), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        "flowChartOffpageConnector" => {
            polygon(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.8), (0.5, 1.0), (0.0, 0.8)])
        }
        "flowChartPunchedCard" => {
            polygon(&[(0.2, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.2)])
        }
        "rightArrow" => polygon(&[
            (0.0, 0.25),
            (0.6, 0.25),
            (0.6, 0.0),
            (1.0, 0.5),
            (0.6, 1.0),
            (0.6, 0.75),
            (0.0, 0.75),
        ]),
        "leftArrow" => polygon(&[
            (1.0, 0.25),
            (0.4, 0.25),
            (0.4, 0.0),
            (0.0, 0.5),
            (0.4, 1.0),
            (0.4, 0.75),
            (1.0, 0.75),
        ]),
        "downArrow" => polygon(&[
            (0.25, 0.0),
            (0.75, 0.0),
            (0.75, 0.6),
            (1.0, 0.6),
            (0.5, 1.0),
            (0.0, 0.6),
            (0.25, 0.6),
        ]),
        "upArrow" => polygon(&[
            (0.25, 1.0),
            (0.75, 1.0),
            (0.75, 0.4),
            (1.0, 0.4),
            (0.5, 0.0),
            (0.0, 0.4),
            (0.25, 0.4),
        ]),
        "flowChartDocument" => format!(
            r#"<path d="M0,0H{w:.1}V{:.1}C{:.1},{:.1} {:.1},{:.1} 0,{:.1}Z"/>"#,
            h * 0.83,
            w * 0.6,
            h * 0.6,
            w * 0.4,
            h * 1.1,
            h * 0.88
        ),
        "can" | "flowChartMagneticDisk" => {
            let ry = (h * 0.1).min(w / 2.0);
            format!(
                r#"<path d="M0,{ry:.1}A{:.1},{ry:.1} 0 0 1 {w:.1},{ry:.1}V{:.1}A{:.1},{ry:.1} 0 0 1 0,{:.1}Z"/><path d="M0,{ry:.1}A{:.1},{ry:.1} 0 0 0 {w:.1},{ry:.1}" fill="none"/>"#,
                w / 2.0,
                h - ry,
                w / 2.0,
                h - ry,
                w / 2.0
            )
        }
        "line" | "straightConnector1" => {
            format!(r#"<line x1="0" y1="0" x2="{w:.1}" y2="{h:.1}"/>"#)
        }
        _ => format!(r#"<rect width="{w:.1}" height="{h:.1}"/>"#),
    }
}

/// A connector: the line from its start to its end, bent or curved as its
/// geometry draws it, with the arrowheads its ends carry.
fn connector(drawing: &Value, rect: Rect, defaults: &Defaults) -> String {
    let (flip_h, flip_v) = transform_flips(drawing);
    let (mut start, mut end) = ((rect.x, rect.y), (rect.x + rect.w, rect.y + rect.h));
    if flip_h {
        std::mem::swap(&mut start.0, &mut end.0);
    }
    if flip_v {
        std::mem::swap(&mut start.1, &mut end.1);
    }
    let (stroke, width) = line(drawing, defaults);
    // A connector without a line shows nothing, as in the original.
    if stroke == "none" {
        return String::new();
    }
    let geometry = drawing["geometry"].as_str().unwrap_or("line");
    let middle = (start.0 + end.0) / 2.0;
    let (path, first, last) = if geometry.starts_with("bentConnector2") {
        (
            format!("M{:.1},{:.1}H{:.1}V{:.1}", start.0, start.1, end.0, end.1),
            (end.0, start.1),
            (end.0, start.1),
        )
    } else if geometry.starts_with("bentConnector") {
        (
            format!(
                "M{:.1},{:.1}H{middle:.1}V{:.1}H{:.1}",
                start.0, start.1, end.1, end.0
            ),
            (middle, start.1),
            (middle, end.1),
        )
    } else if geometry.starts_with("curvedConnector") {
        (
            format!(
                "M{:.1},{:.1}C{middle:.1},{:.1} {middle:.1},{:.1} {:.1},{:.1}",
                start.0, start.1, start.1, end.1, end.0, end.1
            ),
            (middle, start.1),
            (middle, end.1),
        )
    } else {
        (
            format!("M{:.1},{:.1}L{:.1},{:.1}", start.0, start.1, end.0, end.1),
            end,
            start,
        )
    };
    let mut svg = format!(
        r#"<svg class="lines" width="1" height="1"><path d="{path}" fill="none" stroke="{stroke}" stroke-width="{width:.2}"{}/>"#,
        dash(drawing, width)
    );
    // The head is at the start, the tail at the end, each pointing out of
    // the line from the point before it.
    for (mark, tip, from) in [
        (&drawing["head_end"], start, first),
        (&drawing["tail_end"], end, last),
    ] {
        if mark.as_str().is_some_and(|m| m != "none") {
            svg.push_str(&arrowhead(tip, from, width, &stroke));
        }
    }
    svg.push_str("</svg>");
    svg
}

/// An arrowhead at `tip`, pointing away from `from`.
fn arrowhead(tip: (f64, f64), from: (f64, f64), width: f64, color: &str) -> String {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.01 {
        return String::new();
    }
    let (ux, uy) = (dx / length, dy / length);
    let size = (width * 4.0).max(7.0);
    let (bx, by) = (tip.0 - ux * size, tip.1 - uy * size);
    let (nx, ny) = (-uy * size / 2.0, ux * size / 2.0);
    format!(
        r#"<polygon points="{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}" fill="{color}"/>"#,
        tip.0,
        tip.1,
        bx + nx,
        by + ny,
        bx - nx,
        by - ny
    )
}

/// A form control: a check box or option button with its state and label,
/// another control as a button with its text.
fn form_control(control: &Value, rect: Rect, text: &str, mark: &str, place: &str) -> String {
    let _ = rect;
    let kind = control["kind"].as_str().unwrap_or_default();
    let symbol = match (kind, &control["checked"]) {
        ("CheckBox", Value::Bool(true)) => "☑",
        ("CheckBox", Value::Bool(false)) => "☐",
        ("CheckBox", _) => "▣",
        ("Radio", Value::Bool(true)) => "◉",
        ("Radio", _) => "○",
        _ => {
            return format!(
                r#"<div class="shape" style="{place}border:1px solid #888;background:#f0f0f0;border-radius:2px"{mark}><div class="t" style="justify-content:center;text-align:center">{text}</div></div>"#
            );
        }
    };
    let linked = control["linked_cell"]
        .as_str()
        .map(|cell| format!(r#" title="リンク先 {}""#, escape(cell)))
        .unwrap_or_default();
    format!(
        r#"<div class="shape" style="{place}"{mark}><div class="t" style="flex-direction:row;align-items:center;gap:4px;padding:0 2px"{linked}><span style="font-size:1.2em">{symbol}</span><span>{text}</span></div></div>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_flipped_connector_runs_between_the_other_corners_with_its_arrowhead() {
        let drawing = json!({"kind":"connector","geometry":"straightConnector1","tail_end":"triangle",
            "transform":{"flipV":"1"}});
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        let defaults = Defaults {
            fill: "#fff",
            line: "#404040",
            horizontal: "left",
            vertical: "flex-start",
        };
        let svg = connector(&drawing, rect, &defaults);
        assert!(svg.contains(r#"d="M10.0,70.0L110.0,20.0""#), "{svg}");
        assert_eq!(svg.matches("<polygon").count(), 1);
        assert!(svg.contains("110.0,20.0 "), "{svg}");
    }

    #[test]
    fn a_shape_without_its_own_fill_takes_the_default_and_a_check_box_its_state() {
        let defaults = Defaults {
            fill: "#ffffff",
            line: "#404040",
            horizontal: "center",
            vertical: "center",
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 20.0,
        };
        let shape = drawing(
            &json!({"kind":"shape","geometry":"diamond","fill":null}),
            rect,
            "Q",
            "",
            0.0,
            Path::new("."),
            &defaults,
        );
        assert!(shape.contains(r##"fill="#ffffff""##), "{shape}");
        assert!(shape.contains("<polygon"), "{shape}");
        let check = drawing(
            &json!({"kind":"shape","control":{"kind":"CheckBox","checked":true,"linked_cell":"$B$2"}}),
            rect,
            "同意",
            "",
            0.0,
            Path::new("."),
            &defaults,
        );
        assert!(check.contains("☑") && check.contains("同意") && check.contains("$B$2"));
    }
}
