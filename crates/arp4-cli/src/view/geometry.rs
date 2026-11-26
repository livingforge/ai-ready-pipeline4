//! The outlines DrawingML's preset geometries draw (`a:prstGeom`), with the
//! adjustments a shape gives them (`a:avLst`), and the lines of its
//! connectors, as SVG in a box of the shape's own size.
use serde_json::Value;
use std::fmt::Write as _;

/// The adjustments of a shape: each `a:gd` by its name, in the 100,000ths
/// (or 60,000ths of a degree) DrawingML writes.
pub(super) struct Adjust<'a>(pub &'a Value);

impl Adjust<'_> {
    /// The adjustment `name`, `default` when the shape leaves it.
    fn get(&self, name: &str, default: f64) -> f64 {
        self.0[name].as_f64().unwrap_or(default)
    }

    /// The adjustment `name` as a share of a length, between `min` and `max`.
    fn share(&self, name: &str, default: f64, min: f64, max: f64) -> f64 {
        (self.get(name, default) / 100_000.0).clamp(min, max)
    }
}

fn polygon(points: &[(f64, f64)]) -> String {
    let points: Vec<String> = points
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect();
    format!(r#"<polygon points="{}"/>"#, points.join(" "))
}

/// A polygon of `points` given as shares of the width and height.
fn shares(points: &[(f64, f64)], w: f64, h: f64) -> String {
    let points: Vec<(f64, f64)> = points.iter().map(|(x, y)| (x * w, y * h)).collect();
    polygon(&points)
}

/// The corners of a regular polygon or, with `inner` under 1, a star of
/// `count` points, the first `turn` of a corner's step past the top,
/// stretched to fill the box as Office stretches them.
fn star(count: usize, inner: f64, turn: f64, w: f64, h: f64) -> String {
    use std::f64::consts::PI;
    let steps = if inner < 1.0 { count * 2 } else { count };
    let round: Vec<(f64, f64)> = (0..steps)
        .map(|k| {
            let angle = -PI / 2.0 + 2.0 * PI * (k as f64 + turn) / steps as f64;
            let radius = if inner < 1.0 && k % 2 == 1 {
                inner
            } else {
                1.0
            };
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect();
    let bound = |pick: fn(&(f64, f64)) -> f64| {
        let values = round.iter().map(pick);
        (
            values.clone().fold(f64::INFINITY, f64::min),
            values.fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let ((x0, x1), (y0, y1)) = (bound(|p| p.0), bound(|p| p.1));
    let points: Vec<(f64, f64)> = round
        .iter()
        .map(|(x, y)| ((x - x0) / (x1 - x0) * w, (y - y0) / (y1 - y0) * h))
        .collect();
    polygon(&points)
}

/// A rectangle whose corners are each rounded by `r`, cut by `-r` or square
/// for 0: top left, top right, bottom right, bottom left.
fn corners(w: f64, h: f64, r: [f64; 4]) -> String {
    let mut d = String::new();
    let corner = |d: &mut String, r: f64, (x, y): (f64, f64), (ex, ey): (f64, f64)| {
        // From the point before the corner at (x, y) to the point after it.
        if r > 0.0 {
            let _ = write!(d, "A{r:.1},{r:.1} 0 0 1 {ex:.1},{ey:.1}");
        } else if r < 0.0 {
            let _ = write!(d, "L{ex:.1},{ey:.1}");
        } else {
            let _ = write!(d, "L{x:.1},{y:.1}");
        }
    };
    let a: Vec<f64> = r.iter().map(|r| r.abs()).collect();
    let _ = write!(d, "M{:.1},0L{:.1},0", a[0], w - a[1]);
    corner(&mut d, r[1], (w, 0.0), (w, a[1]));
    let _ = write!(d, "L{w:.1},{:.1}", h - a[2]);
    corner(&mut d, r[2], (w, h), (w - a[2], h));
    let _ = write!(d, "L{:.1},{h:.1}", a[3]);
    corner(&mut d, r[3], (0.0, h), (0.0, h - a[3]));
    let _ = write!(d, "L0,{:.1}", a[0]);
    corner(&mut d, r[0], (0.0, 0.0), (a[0], 0.0));
    format!(r#"<path d="{d}Z"/>"#)
}

/// The SVG outline of the preset geometry `name` in a box `w` by `h`; a
/// geometry not drawn here is drawn as a rectangle.
pub(super) fn outline(name: &str, w: f64, h: f64, adjust: &Adjust<'_>) -> String {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let short = w.min(h);
    let rect = || format!(r#"<rect width="{w:.1}" height="{h:.1}"/>"#);
    let ellipse = || {
        format!(
            r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{:.1}"/>"#,
            w / 2.0,
            h / 2.0,
            w / 2.0,
            h / 2.0
        )
    };
    let open = |d: String| format!(r#"<path d="{d}" fill="none"/>"#);
    match name {
        "ellipse" | "flowChartConnector" => ellipse(),
        "flowChartOr" => format!(
            r#"{}<path d="M{:.1},0V{h:.1}M0,{:.1}H{w:.1}" fill="none"/>"#,
            ellipse(),
            w / 2.0,
            h / 2.0
        ),
        "flowChartSummingJunction" => {
            let slant = std::f64::consts::FRAC_1_SQRT_2;
            let (dx, dy) = (w / 2.0 * slant, h / 2.0 * slant);
            format!(
                r#"{}<path d="M{:.1},{:.1}L{:.1},{:.1}M{:.1},{:.1}L{:.1},{:.1}" fill="none"/>"#,
                ellipse(),
                w / 2.0 - dx,
                h / 2.0 - dy,
                w / 2.0 + dx,
                h / 2.0 + dy,
                w / 2.0 + dx,
                h / 2.0 - dy,
                w / 2.0 - dx,
                h / 2.0 + dy
            )
        }
        "roundRect" => {
            let r = short * adjust.share("adj", 16_667.0, 0.0, 0.5);
            format!(r#"<rect width="{w:.1}" height="{h:.1}" rx="{r:.1}"/>"#)
        }
        "flowChartAlternateProcess" => {
            let r = short / 6.0;
            format!(r#"<rect width="{w:.1}" height="{h:.1}" rx="{r:.1}"/>"#)
        }
        "flowChartTerminator" => {
            let r = short / 2.0;
            format!(r#"<rect width="{w:.1}" height="{h:.1}" rx="{r:.1}"/>"#)
        }
        "round1Rect" => corners(
            w,
            h,
            [
                0.0,
                short * adjust.share("adj", 16_667.0, 0.0, 0.5),
                0.0,
                0.0,
            ],
        ),
        "round2SameRect" => {
            let top = short * adjust.share("adj1", 16_667.0, 0.0, 0.5);
            let bottom = short * adjust.share("adj2", 0.0, 0.0, 0.5);
            corners(w, h, [top, top, bottom, bottom])
        }
        "round2DiagRect" => {
            let a = short * adjust.share("adj1", 16_667.0, 0.0, 0.5);
            let b = short * adjust.share("adj2", 0.0, 0.0, 0.5);
            corners(w, h, [a, b, a, b])
        }
        "snip1Rect" => corners(
            w,
            h,
            [
                0.0,
                -short * adjust.share("adj", 16_667.0, 0.0, 0.5),
                0.0,
                0.0,
            ],
        ),
        "snip2SameRect" => {
            let top = -short * adjust.share("adj1", 16_667.0, 0.0, 0.5);
            let bottom = -short * adjust.share("adj2", 0.0, 0.0, 0.5);
            corners(w, h, [top, top, bottom, bottom])
        }
        "snip2DiagRect" => {
            let a = -short * adjust.share("adj1", 0.0, 0.0, 0.5);
            let b = -short * adjust.share("adj2", 16_667.0, 0.0, 0.5);
            corners(w, h, [a, b, a, b])
        }
        "snipRoundRect" => corners(
            w,
            h,
            [
                short * adjust.share("adj1", 16_667.0, 0.0, 0.5),
                -short * adjust.share("adj2", 16_667.0, 0.0, 0.5),
                0.0,
                0.0,
            ],
        ),
        "diamond" | "flowChartDecision" => {
            shares(&[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)], w, h)
        }
        "triangle" => {
            let apex = adjust.share("adj", 50_000.0, 0.0, 1.0);
            shares(&[(apex, 0.0), (1.0, 1.0), (0.0, 1.0)], w, h)
        }
        "flowChartExtract" => shares(&[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)], w, h),
        "flowChartMerge" => shares(&[(0.0, 0.0), (1.0, 0.0), (0.5, 1.0)], w, h),
        "rtTriangle" => shares(&[(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)], w, h),
        "parallelogram" => {
            let x = short * adjust.share("adj", 25_000.0, 0.0, w / short);
            polygon(&[(x, 0.0), (w, 0.0), (w - x, h), (0.0, h)])
        }
        "flowChartInputOutput" => shares(&[(0.2, 0.0), (1.0, 0.0), (0.8, 1.0), (0.0, 1.0)], w, h),
        "trapezoid" => {
            let x = short * adjust.share("adj", 25_000.0, 0.0, w / short / 2.0);
            polygon(&[(x, 0.0), (w - x, 0.0), (w, h), (0.0, h)])
        }
        "flowChartManualOperation" => {
            shares(&[(0.0, 0.0), (1.0, 0.0), (0.8, 1.0), (0.2, 1.0)], w, h)
        }
        "hexagon" => {
            let x = short * adjust.share("adj", 25_000.0, 0.0, w / short / 2.0);
            polygon(&[
                (x, 0.0),
                (w - x, 0.0),
                (w, h / 2.0),
                (w - x, h),
                (x, h),
                (0.0, h / 2.0),
            ])
        }
        "flowChartPreparation" => shares(
            &[
                (0.2, 0.0),
                (0.8, 0.0),
                (1.0, 0.5),
                (0.8, 1.0),
                (0.2, 1.0),
                (0.0, 0.5),
            ],
            w,
            h,
        ),
        "octagon" => {
            let x = short * adjust.share("adj", 29_289.0, 0.0, 0.5);
            polygon(&[
                (x, 0.0),
                (w - x, 0.0),
                (w, x),
                (w, h - x),
                (w - x, h),
                (x, h),
                (0.0, h - x),
                (0.0, x),
            ])
        }
        "pentagon" => star(5, 1.0, 0.0, w, h),
        "heptagon" => star(7, 1.0, 0.0, w, h),
        "decagon" => star(10, 1.0, 0.5, w, h),
        "dodecagon" => star(12, 1.0, 0.5, w, h),
        "flowChartOffpageConnector" => shares(
            &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.8), (0.5, 1.0), (0.0, 0.8)],
            w,
            h,
        ),
        "homePlate" => {
            let x = w - short * adjust.share("adj", 50_000.0, 0.0, w / short);
            polygon(&[(0.0, 0.0), (x, 0.0), (w, h / 2.0), (x, h), (0.0, h)])
        }
        "chevron" => {
            let d = short * adjust.share("adj", 50_000.0, 0.0, w / short);
            polygon(&[
                (0.0, 0.0),
                (w - d, 0.0),
                (w, h / 2.0),
                (w - d, h),
                (0.0, h),
                (d, h / 2.0),
            ])
        }
        "flowChartManualInput" => shares(&[(0.0, 0.2), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], w, h),
        "flowChartPunchedCard" => shares(
            &[(0.2, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.2)],
            w,
            h,
        ),
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" | "leftRightArrow" | "upDownArrow" => {
            // Drawn pointing right in a box `l` long and `t` thick, then
            // turned: the shaft is `adj1` of the thickness, the head `adj2`
            // of the shorter side long.
            let across = matches!(name, "rightArrow" | "leftArrow" | "leftRightArrow");
            let (l, t) = if across { (w, h) } else { (h, w) };
            let double = matches!(name, "leftRightArrow" | "upDownArrow");
            let shaft = t * adjust.share("adj1", 50_000.0, 0.0, 1.0);
            let most = if double { l / 2.0 } else { l };
            let head = (short * adjust.share("adj2", 50_000.0, 0.0, 100.0)).min(most);
            let (y0, y1) = ((t - shaft) / 2.0, (t + shaft) / 2.0);
            let mut points = vec![
                (l - head, y0),
                (l - head, 0.0),
                (l, t / 2.0),
                (l - head, t),
                (l - head, y1),
            ];
            if double {
                points.extend([
                    (head, y1),
                    (head, t),
                    (0.0, t / 2.0),
                    (head, 0.0),
                    (head, y0),
                ]);
            } else {
                points.extend([(0.0, y1), (0.0, y0)]);
            }
            let points: Vec<(f64, f64)> = points
                .into_iter()
                .map(|(x, y)| match name {
                    "leftArrow" => (l - x, y),
                    "downArrow" | "upDownArrow" => (y, x),
                    "upArrow" => (y, l - x),
                    _ => (x, y),
                })
                .collect();
            polygon(&points)
        }
        "plus" => {
            let x = short * adjust.share("adj", 25_000.0, 0.0, 0.5);
            polygon(&[
                (x, 0.0),
                (w - x, 0.0),
                (w - x, x),
                (w, x),
                (w, h - x),
                (w - x, h - x),
                (w - x, h),
                (x, h),
                (x, h - x),
                (0.0, h - x),
                (0.0, x),
                (x, x),
            ])
        }
        "flowChartCollate" => shares(&[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)], w, h),
        "flowChartSort" => format!(
            r#"{}<path d="M0,{:.1}H{w:.1}" fill="none"/>"#,
            shares(&[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)], w, h),
            h / 2.0
        ),
        "star4" => star(4, adjust.share("adj", 12_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star5" => star(5, adjust.share("adj", 19_098.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star6" => star(6, adjust.share("adj", 28_868.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star7" => star(7, adjust.share("adj", 34_601.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star8" => star(8, adjust.share("adj", 37_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star10" => star(10, adjust.share("adj", 42_533.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star12" => star(12, adjust.share("adj", 37_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star16" => star(16, adjust.share("adj", 37_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star24" => star(24, adjust.share("adj", 37_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "star32" => star(32, adjust.share("adj", 37_500.0, 0.0, 0.5) * 2.0, 0.0, w, h),
        "wedgeRectCallout" | "wedgeRoundRectCallout" | "wedgeEllipseCallout" => {
            // The tip of the wedge, from the centre of the box.
            let tip = (
                w / 2.0 + w * adjust.get("adj1", -20_833.0) / 100_000.0,
                h / 2.0 + h * adjust.get("adj2", 62_500.0) / 100_000.0,
            );
            callout(name, w, h, tip, adjust)
        }
        "cloud" | "cloudCallout" => cloud(w, h),
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
            let ry = if name == "can" {
                short * adjust.share("adj", 25_000.0, 0.0, 0.5) / 2.0
            } else {
                h / 6.0
            };
            format!(
                r#"<path d="M0,{ry:.1}A{:.1},{ry:.1} 0 0 1 {w:.1},{ry:.1}V{:.1}A{:.1},{ry:.1} 0 0 1 0,{:.1}Z"/><path d="M0,{ry:.1}A{:.1},{ry:.1} 0 0 0 {w:.1},{ry:.1}" fill="none"/>"#,
                w / 2.0,
                h - ry,
                w / 2.0,
                h - ry,
                w / 2.0
            )
        }
        "cube" => {
            let d = short * adjust.share("adj", 25_000.0, 0.0, 1.0);
            format!(
                r#"{}<path d="M0,{d:.1}H{:.1}V{h:.1}M{:.1},{d:.1}L{w:.1},0" fill="none"/>"#,
                polygon(&[
                    (d, 0.0),
                    (w, 0.0),
                    (w, h - d),
                    (w - d, h),
                    (0.0, h),
                    (0.0, d)
                ]),
                w - d,
                w - d
            )
        }
        "donut" => {
            let t = short * adjust.share("adj", 25_000.0, 0.0, 0.5);
            format!(
                r#"<path fill-rule="evenodd" d="M0,{cy:.1}A{rx:.1},{ry:.1} 0 1 1 {w:.1},{cy:.1}A{rx:.1},{ry:.1} 0 1 1 0,{cy:.1}ZM{t:.1},{cy:.1}A{ix:.1},{iy:.1} 0 1 0 {:.1},{cy:.1}A{ix:.1},{iy:.1} 0 1 0 {t:.1},{cy:.1}Z"/>"#,
                w - t,
                cy = h / 2.0,
                rx = w / 2.0,
                ry = h / 2.0,
                ix = (w / 2.0 - t).max(0.0),
                iy = (h / 2.0 - t).max(0.0)
            )
        }
        "frame" => {
            let t = short * adjust.share("adj1", 12_500.0, 0.0, 0.5);
            format!(
                r#"<path fill-rule="evenodd" d="M0,0H{w:.1}V{h:.1}H0ZM{t:.1},{t:.1}V{:.1}H{:.1}V{t:.1}Z"/>"#,
                h - t,
                w - t
            )
        }
        "foldedCorner" => {
            let d = short * adjust.share("adj", 16_667.0, 0.0, 0.5);
            format!(
                r#"{}<path d="M{:.1},{h:.1}L{:.1},{:.1}L{w:.1},{:.1}" fill="none"/>"#,
                polygon(&[(0.0, 0.0), (w, 0.0), (w, h - d), (w - d, h), (0.0, h)]),
                w - d,
                w - d * 0.8,
                h - d * 0.8,
                h - d
            )
        }
        "heart" => format!(
            r#"<path d="M{:.1},{:.1}C{:.1},{:.1} {:.1},{:.1} {:.1},{h:.1}C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}Z"/>"#,
            w / 2.0,
            h / 4.0,
            w * 0.6,
            -h * 0.2,
            w * 1.25,
            h * 0.25,
            w / 2.0,
            -w * 0.25,
            h * 0.25,
            w * 0.4,
            -h * 0.2,
            w / 2.0,
            h / 4.0
        ),
        "flowChartPredefinedProcess" => format!(
            r#"{}<path d="M{:.1},0V{h:.1}M{:.1},0V{h:.1}" fill="none"/>"#,
            rect(),
            w / 8.0,
            w * 7.0 / 8.0
        ),
        "flowChartInternalStorage" => format!(
            r#"{}<path d="M{:.1},0V{h:.1}M0,{:.1}H{w:.1}" fill="none"/>"#,
            rect(),
            w / 8.0,
            h / 8.0
        ),
        "flowChartDelay" => format!(
            r#"<path d="M0,0H{:.1}A{:.1},{:.1} 0 0 1 {:.1},{h:.1}H0Z"/>"#,
            w / 2.0,
            w / 2.0,
            h / 2.0,
            w / 2.0
        ),
        "flowChartDisplay" => format!(
            r#"<path d="M0,{:.1}L{:.1},0H{:.1}A{:.1},{:.1} 0 0 1 {:.1},{h:.1}H{:.1}Z"/>"#,
            h / 2.0,
            w / 6.0,
            w * 5.0 / 6.0,
            w / 6.0,
            h / 2.0,
            w * 5.0 / 6.0,
            w / 6.0
        ),
        "flowChartMultidocument" => format!(
            r#"<path d="M0,{:.1}H{:.1}V{:.1}C{:.1},{:.1} {:.1},{:.1} 0,{:.1}Z"/><path d="M{:.1},{:.1}V{:.1}H{:.1}V{:.1}M{:.1},{:.1}V0H{w:.1}V{:.1}H{:.1}" fill="none"/>"#,
            h * 0.17,
            w * 0.86,
            h * 0.85,
            w * 0.5,
            h * 0.65,
            w * 0.35,
            h * 1.1,
            h * 0.9,
            w * 0.07,
            h * 0.17,
            h * 0.085,
            w * 0.93,
            h * 0.77,
            w * 0.14,
            h * 0.085,
            h * 0.69,
            w * 0.93
        ),
        "flowChartMagneticDrum" => {
            let rx = w / 6.0;
            format!(
                r#"<path d="M{rx:.1},0H{:.1}A{rx:.1},{:.1} 0 0 1 {:.1},{h:.1}H{rx:.1}A{rx:.1},{:.1} 0 0 1 {rx:.1},0Z"/><path d="M{:.1},{h:.1}A{rx:.1},{:.1} 0 0 1 {:.1},0" fill="none"/>"#,
                w - rx,
                h / 2.0,
                w - rx,
                h / 2.0,
                w - rx,
                h / 2.0,
                w - rx
            )
        }
        "leftBracket" | "rightBracket" => {
            let r = (short * adjust.share("adj", 8_333.0, 0.0, 0.5)).min(w);
            let d = format!(
                "M{w:.1},0A{w:.1},{r:.1} 0 0 0 0,{r:.1}V{:.1}A{w:.1},{r:.1} 0 0 0 {w:.1},{h:.1}",
                h - r
            );
            mirrored(name == "rightBracket", w, open(d))
        }
        "leftBrace" | "rightBrace" => {
            let r = (short * adjust.share("adj1", 8_333.0, 0.0, 0.25)).min(w / 2.0);
            let middle = h * adjust.share("adj2", 50_000.0, 0.0, 1.0);
            let x = w / 2.0;
            let d = format!(
                "M{w:.1},0A{x:.1},{r:.1} 0 0 0 {x:.1},{r:.1}V{:.1}A{x:.1},{r:.1} 0 0 1 0,{middle:.1}A{x:.1},{r:.1} 0 0 1 {x:.1},{:.1}V{:.1}A{x:.1},{r:.1} 0 0 0 {w:.1},{h:.1}",
                middle - r,
                middle + r,
                h - r
            );
            mirrored(name == "rightBrace", w, open(d))
        }
        "bracketPair" => {
            let r = short * adjust.share("adj", 16_667.0, 0.0, 0.5);
            open(format!(
                "M{r:.1},0A{r:.1},{r:.1} 0 0 0 0,{r:.1}V{:.1}A{r:.1},{r:.1} 0 0 0 {r:.1},{h:.1}M{:.1},0A{r:.1},{r:.1} 0 0 1 {w:.1},{r:.1}V{:.1}A{r:.1},{r:.1} 0 0 1 {:.1},{h:.1}",
                h - r,
                w - r,
                h - r,
                w - r
            ))
        }
        "bracePair" => {
            let r = short * adjust.share("adj", 8_333.0, 0.0, 0.25);
            let c = h / 2.0;
            open(format!(
                "M{:.1},0A{r:.1},{r:.1} 0 0 0 {r:.1},{r:.1}V{:.1}A{r:.1},{r:.1} 0 0 1 0,{c:.1}A{r:.1},{r:.1} 0 0 1 {r:.1},{:.1}V{:.1}A{r:.1},{r:.1} 0 0 0 {:.1},{h:.1}M{:.1},0A{r:.1},{r:.1} 0 0 1 {:.1},{r:.1}V{:.1}A{r:.1},{r:.1} 0 0 0 {w:.1},{c:.1}A{r:.1},{r:.1} 0 0 0 {:.1},{:.1}V{:.1}A{r:.1},{r:.1} 0 0 1 {:.1},{h:.1}",
                r * 2.0,
                c - r,
                c + r,
                h - r,
                r * 2.0,
                w - r * 2.0,
                w - r,
                c - r,
                w - r,
                c + r,
                h - r,
                w - r * 2.0
            ))
        }
        "arc" => open(format!(
            "M{:.1},0A{:.1},{:.1} 0 0 1 {w:.1},{:.1}",
            w / 2.0,
            w / 2.0,
            h / 2.0,
            h / 2.0
        )),
        "line" | "straightConnector1" => {
            format!(r#"<line x1="0" y1="0" x2="{w:.1}" y2="{h:.1}"/>"#)
        }
        _ => rect(),
    }
}

/// `svg` mirrored left to right in a box `w` wide when `mirror`.
fn mirrored(mirror: bool, w: f64, svg: String) -> String {
    if mirror {
        format!(r#"<g transform="translate({w:.1},0) scale(-1,1)">{svg}</g>"#)
    } else {
        svg
    }
}

/// A callout: its body with a wedge to `tip`, from the edge nearest it.
fn callout(name: &str, w: f64, h: f64, tip: (f64, f64), adjust: &Adjust<'_>) -> String {
    use std::f64::consts::PI;
    if name == "wedgeEllipseCallout" {
        // The wedge leaves the ellipse a little to each side of the line
        // from its centre to the tip.
        let angle = ((tip.1 - h / 2.0) / h).atan2((tip.0 - w / 2.0) / w);
        let at = |a: f64| (w / 2.0 * (1.0 + a.cos()), h / 2.0 * (1.0 + a.sin()));
        let (a, b) = (at(angle + PI / 15.0), at(angle - PI / 15.0));
        return format!(
            r#"<path d="M{:.1},{:.1}A{:.1},{:.1} 0 1 1 {:.1},{:.1}L{:.1},{:.1}Z"/>"#,
            a.0,
            a.1,
            w / 2.0,
            h / 2.0,
            b.0,
            b.1,
            tip.0,
            tip.1
        );
    }
    let r = if name == "wedgeRoundRectCallout" {
        w.min(h) * adjust.share("adj3", 16_667.0, 0.0, 0.5)
    } else {
        0.0
    };
    // The side the tip is beyond, by the diagonals of the box, and the part
    // of that side the wedge leaves from: the half the tip is nearer.
    let (dx, dy) = ((tip.0 - w / 2.0) / w, (tip.1 - h / 2.0) / h);
    let side = if dy.abs() >= dx.abs() {
        if dy > 0.0 { 2 } else { 0 }
    } else if dx > 0.0 {
        1
    } else {
        3
    };
    let inside = tip.0 >= 0.0 && tip.0 <= w && tip.1 >= 0.0 && tip.1 <= h;
    // The wedge on a side from `a` to `b` of its length, both from its start
    // as the outline runs clockwise.
    let wedge = |length: f64, near: bool| {
        if near {
            (length * 7.0 / 12.0, length * 5.0 / 6.0)
        } else {
            (length / 6.0, length * 5.0 / 12.0)
        }
    };
    let mut d = format!("M{r:.1},0");
    let edge = |d: &mut String,
                index: usize,
                length: f64,
                point: &dyn Fn(f64) -> (f64, f64),
                near: bool| {
        if side == index && !inside {
            let (a, b) = wedge(length, near);
            let (a, b) = (point(a), point(b));
            let _ = write!(
                d,
                "L{:.1},{:.1}L{:.1},{:.1}L{:.1},{:.1}",
                a.0, a.1, tip.0, tip.1, b.0, b.1
            );
        }
        let end = point(length - r);
        let _ = write!(d, "L{:.1},{:.1}", end.0, end.1);
        if r > 0.0 {
            let corner = point(length);
            let after = match index {
                0 => (corner.0, corner.1 + r),
                1 => (corner.0 - r, corner.1),
                2 => (corner.0, corner.1 - r),
                _ => (corner.0 + r, corner.1),
            };
            let _ = write!(d, "A{r:.1},{r:.1} 0 0 1 {:.1},{:.1}", after.0, after.1);
        }
    };
    edge(&mut d, 0, w, &|t| (t, 0.0), dx > 0.0);
    edge(&mut d, 1, h, &|t| (w, t), dy > 0.0);
    edge(&mut d, 2, w, &|t| (w - t, h), dx < 0.0);
    edge(&mut d, 3, h, &|t| (0.0, h - t), dy < 0.0);
    format!(r#"<path d="{d}Z"/>"#)
}

/// A cloud: bulges around the ellipse of the box.
fn cloud(w: f64, h: f64) -> String {
    use std::f64::consts::PI;
    // The ends of the bulges, around an ellipse a little inside the box.
    let bulges = 11;
    let at = |k: usize| {
        let angle = 2.0 * PI * k as f64 / bulges as f64 - PI * 0.9;
        (
            w / 2.0 * (1.0 + 0.82 * angle.cos()),
            h / 2.0 * (1.0 + 0.78 * angle.sin()),
        )
    };
    let start = at(0);
    let mut d = format!("M{:.1},{:.1}", start.0, start.1);
    for k in 1..=bulges {
        let (from, to) = (at(k - 1), at(k));
        let chord = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let r = chord * 0.62;
        let _ = write!(d, "A{r:.1},{r:.1} 0 0 1 {:.1},{:.1}", to.0, to.1);
    }
    format!(r#"<path d="{d}Z"/>"#)
}

/// The line a connector draws in a box `w` by `h`, from its top left to its
/// bottom right: the SVG path, and the points its ends leave from, by which
/// their arrowheads point.
pub(super) struct Route {
    pub path: String,
    /// The point the line comes to its start from, and to its end from.
    pub before_start: (f64, f64),
    pub before_end: (f64, f64),
}

pub(super) fn route(name: &str, w: f64, h: f64, adjust: &Adjust<'_>) -> Route {
    let share = |name: &str| adjust.get(name, 50_000.0) / 100_000.0;
    let line = |points: &[(f64, f64)]| {
        let mut path = String::new();
        for (index, (x, y)) in points.iter().enumerate() {
            let _ = write!(path, "{}{x:.1},{y:.1}", if index == 0 { 'M' } else { 'L' });
        }
        // An end points along the last stretch of the line that has a
        // length: a connector between shapes in a row has bends of none.
        let far = |from: (f64, f64), mut others: Box<dyn Iterator<Item = &(f64, f64)> + '_>| {
            others
                .find(|p| (p.0 - from.0).abs() + (p.1 - from.1).abs() > 0.05)
                .copied()
                .unwrap_or(from)
        };
        let (first, last) = (points[0], points[points.len() - 1]);
        Route {
            path,
            before_start: far(first, Box::new(points.iter().skip(1))),
            before_end: far(last, Box::new(points.iter().rev().skip(1))),
        }
    };
    let curve = |points: &[(f64, f64)]| {
        // The first point, then three for each Bézier curve.
        let mut path = format!("M{:.1},{:.1}", points[0].0, points[0].1);
        for c in points[1..].chunks(3) {
            let _ = write!(
                path,
                "C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                c[0].0, c[0].1, c[1].0, c[1].1, c[2].0, c[2].1
            );
        }
        Route {
            path,
            before_start: points[1],
            before_end: points[points.len() - 2],
        }
    };
    match name {
        "bentConnector2" => line(&[(0.0, 0.0), (w, 0.0), (w, h)]),
        "bentConnector3" => {
            let x = w * share("adj1");
            line(&[(0.0, 0.0), (x, 0.0), (x, h), (w, h)])
        }
        "bentConnector4" => {
            let (x, y) = (w * share("adj1"), h * share("adj2"));
            line(&[(0.0, 0.0), (x, 0.0), (x, y), (w, y), (w, h)])
        }
        "bentConnector5" => {
            let (x1, y, x3) = (w * share("adj1"), h * share("adj2"), w * share("adj3"));
            line(&[(0.0, 0.0), (x1, 0.0), (x1, y), (x3, y), (x3, h), (w, h)])
        }
        "curvedConnector2" => curve(&[(0.0, 0.0), (w / 2.0, 0.0), (w, h / 2.0), (w, h)]),
        "curvedConnector3" => {
            let x2 = w * share("adj1");
            curve(&[
                (0.0, 0.0),
                (x2 / 2.0, 0.0),
                (x2, h / 4.0),
                (x2, h / 2.0),
                (x2, h * 0.75),
                ((w + x2) / 2.0, h),
                (w, h),
            ])
        }
        "curvedConnector4" => {
            let (x2, y4) = (w * share("adj1"), h * share("adj2"));
            let (x1, x3) = (x2 / 2.0, (w + x2) / 2.0);
            let (x4, x5) = ((x2 + x3) / 2.0, (x3 + w) / 2.0);
            let y1 = y4 / 2.0;
            curve(&[
                (0.0, 0.0),
                (x1, 0.0),
                (x2, y1 / 2.0),
                (x2, y1),
                (x2, (y1 + y4) / 2.0),
                (x4, y4),
                (x3, y4),
                (x5, y4),
                (w, (h + y4) / 2.0),
                (w, h),
            ])
        }
        "curvedConnector5" => {
            let (x3, y4, x6) = (w * share("adj1"), h * share("adj2"), w * share("adj3"));
            let x1 = (x3 + x6) / 2.0;
            let (y1, y5) = (y4 / 2.0, (h + y4) / 2.0);
            curve(&[
                (0.0, 0.0),
                (x3 / 2.0, 0.0),
                (x3, y1 / 2.0),
                (x3, y1),
                (x3, (y1 + y4) / 2.0),
                ((x3 + x1) / 2.0, y4),
                (x1, y4),
                ((x6 + x1) / 2.0, y4),
                (x6, (y5 + y4) / 2.0),
                (x6, y5),
                (x6, (y5 + h) / 2.0),
                ((x6 + w) / 2.0, h),
                (w, h),
            ])
        }
        _ => line(&[(0.0, 0.0), (w, h)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_bent_connector_bends_where_its_adjustment_puts_it() {
        let none = json!({});
        let middle = route("bentConnector3", 100.0, 40.0, &Adjust(&none));
        assert_eq!(middle.path, "M0.0,0.0L50.0,0.0L50.0,40.0L100.0,40.0");
        assert_eq!(middle.before_end, (50.0, 40.0));
        // Beyond the box, as a connector between two right sides is.
        let adjusted = json!({"adj1":120_000.0});
        let beyond = route("bentConnector3", 100.0, 40.0, &Adjust(&adjusted));
        assert_eq!(beyond.path, "M0.0,0.0L120.0,0.0L120.0,40.0L100.0,40.0");
        assert_eq!(beyond.before_end, (120.0, 40.0));
        // A bend of no length is passed over for the way an end points.
        let flat = route("bentConnector3", 0.0, 40.0, &Adjust(&none));
        assert_eq!(flat.before_end, (0.0, 0.0));
        assert_eq!(flat.before_start, (0.0, 40.0));
    }

    #[test]
    fn arrows_and_polygons_take_their_adjustments() {
        let none = json!({});
        let arrow = outline("leftRightArrow", 100.0, 40.0, &Adjust(&none));
        assert_eq!(arrow.matches(',').count(), 10, "{arrow}");
        assert!(arrow.contains("100.0,20.0") && arrow.contains("0.0,20.0"));
        let pentagon = outline("pentagon", 100.0, 100.0, &Adjust(&none));
        assert!(pentagon.contains("50.0,0.0"), "{pentagon}");
        let wide = json!({"adj":50_000.0});
        let round = outline("roundRect", 100.0, 40.0, &Adjust(&wide));
        assert!(round.contains(r#"rx="20.0""#), "{round}");
        assert!(outline("unknownShape", 10.0, 10.0, &Adjust(&none)).starts_with("<rect"));
    }
}
