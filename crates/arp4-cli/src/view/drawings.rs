//! The drawings of an Excel sheet or a PowerPoint slide: each shape in its
//! geometry with its fill, line and text, each connector as the line it
//! draws with its arrowheads, and each picture with its image, in the box
//! its sheet or slide gives it.
use super::geometry::{self, Adjust};
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

/// Gives `drawing` what its shape says beyond the extraction, for
/// [`drawing`] to draw it by.
pub(super) fn extras(drawing: &mut Value, extras: &crate::excel::ShapeExtras) {
    use serde_json::json;
    drawing["adjust"] = extras
        .adjust
        .iter()
        .map(|(name, value)| (name.clone(), json!(value)))
        .collect::<serde_json::Map<_, _>>()
        .into();
    drawing["paths"] = extras
        .paths
        .iter()
        .map(|path| {
            json!({"width":path.width,"height":path.height,"d":path.d,
                "filled":path.filled,"stroked":path.stroked})
        })
        .collect();
    drawing["text_direction"] = json!(extras.text_direction);
    drawing["head_size"] = json!(extras.head_size);
    drawing["tail_size"] = json!(extras.tail_size);
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
        Some("connector") => connector(drawing, rect, rotation, defaults),
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
            let mut fill = match drawing["fill"].as_str() {
                Some("none") => "none".to_owned(),
                _ => color(&drawing["fill"]).unwrap_or_else(|| defaults.fill.to_owned()),
            };
            let (stroke, width) = line(drawing, defaults);
            let flip = transform_flips(drawing);
            let (w, h) = (rect.w.max(1.0), rect.h.max(1.0));
            let outline = match drawing["paths"].as_array() {
                Some(paths) if !paths.is_empty() => custom(paths, w, h),
                _ => geometry::outline(
                    drawing["geometry"].as_str().unwrap_or("rect"),
                    w,
                    h,
                    &Adjust(&drawing["adjust"]),
                ),
            };
            let scale = match flip {
                (false, false) => String::new(),
                (h, v) => format!(
                    r#" style="transform:scale({},{})""#,
                    if h { -1 } else { 1 },
                    if v { -1 } else { 1 }
                ),
            };
            // A gradient is a paint of the page, named after the drawing.
            let mut paints = String::new();
            if let Some(gradient) = gradient(&drawing["gradient"], &drawing["id"]) {
                fill = format!("url(#{})", gradient.0);
                paints = gradient.1;
            }
            let opacity = drawing["fill_opacity"]
                .as_f64()
                .map(|o| format!(r#" fill-opacity="{o:.2}""#))
                .unwrap_or_default();
            let dash = dash(drawing, width);
            let mut align = format!(
                "justify-content:{};text-align:{};",
                defaults.vertical, defaults.horizontal
            );
            // Text running down the shape: its lines go from right to left,
            // so what places it down the shape places it across.
            align.push_str(match drawing["text_direction"].as_str() {
                Some("eaVert" | "mongolianVert") => "writing-mode:vertical-rl;",
                Some("wordArtVert" | "wordArtVertRtl") => {
                    "writing-mode:vertical-rl;text-orientation:upright;"
                }
                Some("vert") => "writing-mode:vertical-rl;text-orientation:sideways;",
                Some("vert270") => "writing-mode:sideways-lr;",
                _ => "",
            });
            format!(
                r#"<div class="shape" style="{}"{mark}><svg viewBox="0 0 {w:.1} {h:.1}"{scale}>{paints}<g fill="{fill}"{opacity} stroke="{stroke}" stroke-width="{width:.2}"{dash}>{outline}</g></svg><div class="t" style="{align}{}">{text}</div></div>"#,
                place(""),
                font_css(&drawing["font"], &drawing["style"]),
            )
        }
    }
}

/// The paths of a custom geometry in a box `w` by `h`: each is drawn in the
/// size its points are given in and stretched to the box.
fn custom(paths: &[Value], w: f64, h: f64) -> String {
    let mut svg = String::new();
    for path in paths {
        let (Some(width), Some(height), Some(d)) = (
            path["width"].as_f64().filter(|w| *w > 0.0),
            path["height"].as_f64().filter(|h| *h > 0.0),
            path["d"].as_str(),
        ) else {
            continue;
        };
        let _ = write!(
            svg,
            r#"<path d="{}" transform="scale({:.6},{:.6})" vector-effect="non-scaling-stroke"{}{}/>"#,
            escape(d),
            w / width,
            h / height,
            if path["filled"] == false {
                r#" fill="none""#
            } else {
                ""
            },
            if path["stroked"] == false {
                r#" stroke="none""#
            } else {
                ""
            },
        );
    }
    svg
}

/// The SVG paint of a gradient fill, with the name the drawing `id` gives
/// it; none when the drawing has no gradient.
fn gradient(gradient: &Value, id: &Value) -> Option<(String, String)> {
    let stops = gradient["stops"].as_array().filter(|s| s.len() > 1)?;
    let name: String = format!("g-{}", id.as_str().unwrap_or_default())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let mut inner = String::new();
    for stop in stops {
        let _ = write!(
            inner,
            r#"<stop offset="{:.3}" stop-color="{}"/>"#,
            stop[0].as_f64()?,
            color(&stop[1])?
        );
    }
    let svg = match gradient["angle"].as_f64() {
        Some(angle) => format!(
            r#"<defs><linearGradient id="{name}" gradientTransform="rotate({angle:.1},0.5,0.5)">{inner}</linearGradient></defs>"#
        ),
        None => format!(r#"<defs><radialGradient id="{name}">{inner}</radialGradient></defs>"#),
    };
    Some((name, svg))
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

/// A connector: the line from its start to its end, bent or curved as its
/// geometry and adjustments draw it in its own box, which its flips and its
/// turn (`rotation`, degrees clockwise) then place, with the arrowheads its
/// ends carry.
fn connector(drawing: &Value, rect: Rect, rotation: f64, defaults: &Defaults) -> String {
    let (stroke, width) = line(drawing, defaults);
    // A connector without a line shows nothing, as in the original.
    if stroke == "none" {
        return String::new();
    }
    let (w, h) = (rect.w.max(0.0), rect.h.max(0.0));
    let route = geometry::route(
        drawing["geometry"].as_str().unwrap_or("line"),
        w,
        h,
        &Adjust(&drawing["adjust"]),
    );
    let mut transform = format!("translate({:.1},{:.1})", rect.x, rect.y);
    if rotation.abs() > 0.01 {
        let _ = write!(
            transform,
            " rotate({rotation:.2},{:.1},{:.1})",
            w / 2.0,
            h / 2.0
        );
    }
    let (flip_h, flip_v) = transform_flips(drawing);
    if flip_h {
        let _ = write!(transform, " translate({w:.1},0) scale(-1,1)");
    }
    if flip_v {
        let _ = write!(transform, " translate(0,{h:.1}) scale(1,-1)");
    }
    let mut svg = format!(
        r#"<svg class="lines" width="1" height="1"><g transform="{transform}"><path d="{}" fill="none" stroke="{stroke}" stroke-width="{width:.2}"{}/>"#,
        route.path,
        dash(drawing, width)
    );
    // The head is at the start, the tail at the end, each pointing out of
    // the line from the point before it.
    for (kind, size, tip, from) in [
        (
            &drawing["head_end"],
            &drawing["head_size"],
            (0.0, 0.0),
            route.before_start,
        ),
        (
            &drawing["tail_end"],
            &drawing["tail_size"],
            (w, h),
            route.before_end,
        ),
    ] {
        if let Some(kind) = kind.as_str().filter(|kind| *kind != "none") {
            svg.push_str(&arrowhead(kind, size, tip, from, width, &stroke));
        }
    }
    svg.push_str("</g></svg>");
    svg
}

/// An arrowhead of `kind` (triangle, stealth, arrow, diamond, oval) at
/// `tip`, pointing away from `from`, as wide and long as `size` (`w` and
/// `len`: sm, med, lg) makes it for a line `width` pixels wide.
fn arrowhead(
    kind: &str,
    size: &Value,
    tip: (f64, f64),
    from: (f64, f64),
    width: f64,
    color: &str,
) -> String {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.01 {
        return String::new();
    }
    let (ux, uy) = (dx / length, dy / length);
    // Office makes a head so many times the width of its line, a thin line
    // counting as one of 2 points.
    let times = |size: &Value| match size.as_str() {
        Some("sm") => 2.0,
        Some("lg") => 5.0,
        _ => 3.0,
    };
    let unit = width.max(8.0 / 3.0);
    let (across, along) = (unit * times(&size[0]), unit * times(&size[1]));
    let (bx, by) = (tip.0 - ux * along, tip.1 - uy * along);
    let (nx, ny) = (-uy * across / 2.0, ux * across / 2.0);
    let points = |points: &[(f64, f64)]| {
        points
            .iter()
            .map(|(x, y)| format!("{x:.1},{y:.1}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    match kind {
        "arrow" => format!(
            r#"<polyline points="{}" fill="none" stroke="{color}" stroke-width="{width:.2}"/>"#,
            points(&[(bx + nx, by + ny), tip, (bx - nx, by - ny)])
        ),
        "stealth" => format!(
            r#"<polygon points="{}" fill="{color}"/>"#,
            points(&[
                tip,
                (bx + nx, by + ny),
                (tip.0 - ux * along * 0.6, tip.1 - uy * along * 0.6),
                (bx - nx, by - ny)
            ])
        ),
        // A diamond and an oval sit on the end of the line.
        "diamond" => format!(
            r#"<polygon points="{}" fill="{color}"/>"#,
            points(&[
                (tip.0 + ux * along / 2.0, tip.1 + uy * along / 2.0),
                (tip.0 + nx, tip.1 + ny),
                (tip.0 - ux * along / 2.0, tip.1 - uy * along / 2.0),
                (tip.0 - nx, tip.1 - ny)
            ])
        ),
        "oval" => format!(
            r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}" transform="rotate({:.1},{:.1},{:.1})" fill="{color}"/>"#,
            tip.0,
            tip.1,
            along / 2.0,
            across / 2.0,
            uy.atan2(ux).to_degrees(),
            tip.0,
            tip.1
        ),
        _ => format!(
            r#"<polygon points="{}" fill="{color}"/>"#,
            points(&[tip, (bx + nx, by + ny), (bx - nx, by - ny)])
        ),
    }
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
    fn a_connector_is_flipped_and_turned_in_its_own_box_with_its_arrowheads() {
        let drawing = json!({"kind":"connector","geometry":"bentConnector3","tail_end":"triangle",
            "head_end":"oval","adjust":{"adj1":25_000.0},"transform":{"flipV":"1"}});
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
        let svg = connector(&drawing, rect, 90.0, &defaults);
        assert!(
            svg.contains(
                r#"transform="translate(10.0,20.0) rotate(90.00,50.0,25.0) translate(0,50.0) scale(1,-1)""#
            ),
            "{svg}"
        );
        // The bend is a quarter of the way along, where the adjustment puts it.
        assert!(
            svg.contains(r#"d="M0.0,0.0L25.0,0.0L25.0,50.0L100.0,50.0""#),
            "{svg}"
        );
        // The tail is a triangle at the end, the head an oval at the start.
        assert_eq!(svg.matches("<polygon").count(), 1);
        assert!(svg.contains(r#"points="100.0,50.0 "#), "{svg}");
        assert!(svg.contains(r#"<ellipse cx="0.0" cy="0.0""#), "{svg}");
        // A connector without a line shows nothing.
        let mut plain = drawing.clone();
        plain["line"] = json!({"color":"none"});
        assert_eq!(connector(&plain, rect, 0.0, &defaults), "");
    }

    #[test]
    fn a_shape_draws_its_custom_paths_its_gradient_and_its_text_running_down() {
        let defaults = Defaults {
            fill: "#ffffff",
            line: "#404040",
            horizontal: "center",
            vertical: "center",
        };
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 40.0,
        };
        let shape = drawing(
            &json!({"kind":"shape","id":"xl/drawings/drawing1.xml#7","fill":null,
                "paths":[{"width":800.0,"height":400.0,"d":"M0,0L800,0L400,400Z","filled":true,"stroked":true}],
                "gradient":{"stops":[[0.0,"FF0000"],[1.0,"FFFFFF"]],"angle":90.0},
                "fill_opacity":0.4,"text_direction":"eaVert"}),
            rect,
            "縦",
            "",
            0.0,
            Path::new("."),
            &defaults,
        );
        assert!(
            shape.contains(r#"<path d="M0,0L800,0L400,400Z" transform="scale(0.100000,0.100000)""#),
            "{shape}"
        );
        assert!(shape.contains(r#"<linearGradient id="g-xl-drawings-drawing1-xml-7" gradientTransform="rotate(90.0,0.5,0.5)">"#), "{shape}");
        assert!(
            shape.contains(r#"fill="url(#g-xl-drawings-drawing1-xml-7)" fill-opacity="0.40""#),
            "{shape}"
        );
        assert!(shape.contains("writing-mode:vertical-rl;"), "{shape}");
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
