//! The shapes, pictures, connectors, groups and graphic frames of a slide as
//! the drawings of its extraction sheet, like those of an Excel sheet: an ID
//! (`<slide part>#<cNvPr id>`), where it is on the slide in points, its
//! geometry, fill, line, text and font, and the image or chart it shows.
use super::slide_fonts::SlideFonts;
use super::*;
use crate::fonts::{Color, ColorContext};

const EMU_PER_POINT: f64 = 12_700.0;
const CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// Whether `node` is a drawing object of a shape tree.
fn object(node: Node<'_, '_>) -> bool {
    node.tag_name().namespace() == Some(PRESENTATION)
        && matches!(
            node.tag_name().name(),
            "sp" | "pic" | "cxnSp" | "grpSp" | "graphicFrame"
        )
}

/// The non-visual properties (`p:cNvPr`) of an object.
fn properties<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.tag_name().name().starts_with("nv"))?
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "cNvPr")))
}

/// The transform (`a:xfrm` / `p:xfrm`) of an object.
fn transform<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    let shape = node.children().find(|n| {
        n.has_tag_name((PRESENTATION, "spPr")) || n.has_tag_name((PRESENTATION, "grpSpPr"))
    });
    shape
        .and_then(|s| s.children().find(|n| n.has_tag_name((DRAWING, "xfrm"))))
        .or_else(|| {
            node.children()
                .find(|n| n.has_tag_name((PRESENTATION, "xfrm")))
        })
}

fn pair(node: Option<Node<'_, '_>>, x: &str, y: &str) -> Option<(f64, f64)> {
    let node = node?;
    Some((
        node.attribute(x)?.parse().ok()?,
        node.attribute(y)?.parse().ok()?,
    ))
}

/// An object's box in EMU as (left, top, width, height), mapped out of the
/// groups that hold it into slide coordinates.
pub(super) fn slide_box(node: Node<'_, '_>) -> Option<[f64; 4]> {
    let xfrm = transform(node)?;
    let child = |name: &str| xfrm.children().find(|n| n.has_tag_name((DRAWING, name)));
    let (x, y) = pair(child("off"), "x", "y")?;
    let (w, h) = pair(child("ext"), "cx", "cy")?;
    let mut area = [x, y, w, h];
    for group in node
        .ancestors()
        .skip(1)
        .filter(|n| n.has_tag_name((PRESENTATION, "grpSp")))
    {
        let Some(xfrm) = transform(group) else {
            continue;
        };
        let child = |name: &str| xfrm.children().find(|n| n.has_tag_name((DRAWING, name)));
        let (Some(off), Some(ext), Some(child_off), Some(child_ext)) = (
            pair(child("off"), "x", "y"),
            pair(child("ext"), "cx", "cy"),
            pair(child("chOff"), "x", "y"),
            pair(child("chExt"), "cx", "cy"),
        ) else {
            continue;
        };
        let scale = |ext: f64, child: f64| if child == 0.0 { 1.0 } else { ext / child };
        let (sx, sy) = (scale(ext.0, child_ext.0), scale(ext.1, child_ext.1));
        area = [
            off.0 + (area[0] - child_off.0) * sx,
            off.1 + (area[1] - child_off.1) * sy,
            area[2] * sx,
            area[3] * sy,
        ];
    }
    Some(area)
}

fn points(emu: f64) -> Value {
    crate::fonts::size_value(emu / EMU_PER_POINT)
}

fn attributes(node: Node<'_, '_>) -> Value {
    Value::Object(
        node.attributes()
            .map(|a| (a.name().to_owned(), json!(a.value())))
            .collect(),
    )
}

/// A fill (`a:noFill`, `a:solidFill`) among `node`'s children: `none`, a
/// colour, or None for other fills (gradients, pictures, patterns).
fn fill_of(node: Node<'_, '_>, context: &ColorContext<'_>) -> Option<Value> {
    for child in node.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "noFill" => return Some(json!("none")),
            "solidFill" => {
                return Some(
                    child
                        .children()
                        .find(Node::is_element)
                        .and_then(|c| crate::fonts::drawing_color(c, context))
                        .map_or(Value::Null, |color| match color {
                            Color::Rgb { rgb, .. } => json!(rgb),
                            Color::Auto => Value::Null,
                        }),
                );
            }
            "gradFill" | "blipFill" | "pattFill" | "grpFill" => return Some(Value::Null),
            _ => {}
        }
    }
    None
}

/// The colour a shape style reference (`a:fillRef`, `a:lnRef`) gives, or
/// `none` for index 0.
fn style_color(style: Option<Node<'_, '_>>, name: &str, context: &ColorContext<'_>) -> Value {
    let Some(reference) =
        style.and_then(|s| s.children().find(|n| n.has_tag_name((DRAWING, name))))
    else {
        return Value::Null;
    };
    if reference.attribute("idx") == Some("0") {
        return json!("none");
    }
    reference
        .children()
        .find(Node::is_element)
        .and_then(|c| crate::fonts::drawing_color(c, context))
        .map_or(Value::Null, |color| match color {
            Color::Rgb { rgb, .. } => json!(rgb),
            Color::Auto => Value::Null,
        })
}

/// The drawings of slide `part` (`xml`), with `images` (by part) giving the
/// bytes of the pictures they show and `fonts` the fonts of their text.
pub(super) fn drawings(
    parts: &BTreeMap<String, Vec<u8>>,
    images: &BTreeMap<String, Vec<u8>>,
    part: &str,
    xml: &Document<'_>,
    fonts: &SlideFonts,
) -> Result<Vec<Value>> {
    let context = fonts.color_context();
    let mut objects = vec![];
    let Some(tree) = xml
        .descendants()
        .find(|n| n.has_tag_name((PRESENTATION, "spTree")))
    else {
        return Ok(objects);
    };
    for node in tree
        .descendants()
        .filter(|n| object(*n) && !hidden_copy(*n, "pptx"))
    {
        let property = properties(node);
        let raw_id = property.and_then(|p| p.attribute("id")).unwrap_or("0");
        let kind = match node.tag_name().name() {
            "sp" => "shape",
            "pic" => "picture",
            "cxnSp" => "connector",
            "grpSp" => "group",
            _ => "graphic",
        };
        let group = node
            .ancestors()
            .skip(1)
            .find(|n| n.has_tag_name((PRESENTATION, "grpSp")) && *n != tree)
            .and_then(properties)
            .and_then(|p| p.attribute("id"))
            .map(|id| format!("{part}#{id}"));
        let xfrm = transform(node);
        let rotation = xfrm
            .and_then(|x| x.attribute("rot"))
            .and_then(|r| r.parse::<f64>().ok())
            .map_or(json!(0), |r| crate::fonts::size_value(r / 60_000.0));
        let anchor = match slide_box(node) {
            Some([left, top, width, height]) => {
                json!({"kind":"slide","left":points(left),"top":points(top),"width":points(width),"height":points(height),"rotation":rotation})
            }
            // A placeholder without a transform sits where its layout puts it.
            None => {
                json!({"kind":"slide","left":null,"top":null,"width":null,"height":null,"rotation":rotation})
            }
        };
        let transform = xfrm.map(|n| {
            let mut v = attributes(n);
            for child in n.children().filter(Node::is_element) {
                v[child.tag_name().name()] = attributes(child);
            }
            v
        });
        let shape_properties = node
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "spPr")));
        let style = node
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "style")));
        let body = node
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "txBody")));
        let text = body
            .map(|body| {
                body.children()
                    .filter(|n| n.has_tag_name((DRAWING, "p")))
                    .map(|p| {
                        p.descendants()
                            .filter_map(|n| {
                                if n.has_tag_name((DRAWING, "t")) {
                                    n.text()
                                } else if n.has_tag_name((DRAWING, "br")) {
                                    Some("\n")
                                } else {
                                    None
                                }
                            })
                            .collect::<String>()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let mut connections = vec![];
        if kind == "connector" {
            for c in node.descendants().filter(|n| {
                n.tag_name().namespace() == Some(DRAWING)
                    && matches!(n.tag_name().name(), "stCxn" | "endCxn")
            }) {
                connections.push(json!({"end":c.tag_name().name(),"target":c.attribute("id").map(|v|format!("{part}#{v}")),"site":c.attribute("idx"),"basis":"explicit"}));
            }
        }
        let mut image = Value::Null;
        let mut linked_image = Value::Null;
        if kind == "picture"
            && let Some(blip) = node
                .descendants()
                .find(|n| n.has_tag_name((DRAWING, "blip")))
        {
            if let Some(id) = blip.attribute((REL, "embed")) {
                let media = relation_target_with(parts, images, part, id)?;
                if let Some(bytes) = images.get(&media).or_else(|| parts.get(&media)) {
                    image = json!({"part":media,"sha256":hash(bytes)});
                }
            }
            linked_image = json!(blip.attribute((REL, "link")));
        }
        let chart_part = node
            .descendants()
            .find(|n| n.has_tag_name((CHART, "chart")))
            .and_then(|n| n.attribute((REL, "id")))
            .map(|id| relation_target_with(parts, images, part, id))
            .transpose()?;
        let placeholder = node
            .children()
            .find(|n| n.tag_name().name().starts_with("nv"))
            .and_then(|n| n.children().find(|n| n.has_tag_name((PRESENTATION, "nvPr"))))
            .and_then(|n| n.children().find(|n| n.has_tag_name((PRESENTATION, "ph"))))
            .map(|ph| json!({"type":ph.attribute("type").unwrap_or("obj"),"index":ph.attribute("idx")}));
        let mut object = json!({"id":format!("{part}#{raw_id}"),"part":part,"kind":kind,
            "macro":null,"control":null,
            "name":property.and_then(|n|n.attribute("name")).unwrap_or(""),"description":property.and_then(|n|n.attribute("descr")).unwrap_or(""),
            "text":text,"anchor":anchor,"group":group,"transform":transform,
            "geometry":shape_properties.and_then(|n|n.children().find(|n|n.has_tag_name((DRAWING,"prstGeom")))).and_then(|n|n.attribute("prst")),
            "connections":connections,"image":image,"linked_image":linked_image});
        if let Some(chart_part) = chart_part {
            object["chart_part"] = json!(chart_part);
        }
        if let Some(placeholder) = placeholder {
            object["placeholder"] = placeholder;
        }
        if matches!(kind, "shape" | "connector" | "picture") {
            let line = shape_properties
                .and_then(|p| p.children().find(|n| n.has_tag_name((DRAWING, "ln"))));
            object["fill"] = shape_properties
                .and_then(|p| fill_of(p, &context))
                .unwrap_or_else(|| style_color(style, "fillRef", &context));
            object["line"] = json!({
                "color": line
                    .and_then(|l| fill_of(l, &context))
                    .unwrap_or_else(|| style_color(style, "lnRef", &context)),
                "width": line
                    .and_then(|l| l.attribute("w"))
                    .and_then(|w| w.parse::<f64>().ok())
                    .map_or(Value::Null, points),
            });
        }
        if kind == "shape" && !text.is_empty() {
            let runs: Vec<Node<'_, '_>> = body
                .into_iter()
                .flat_map(|b| b.descendants())
                .filter(|n| n.has_tag_name((DRAWING, "r")) || n.has_tag_name((DRAWING, "fld")))
                .filter(|run| {
                    run.children()
                        .find(|n| n.has_tag_name((DRAWING, "t")))
                        .and_then(|t| t.text())
                        .is_some_and(|t| !t.is_empty())
                })
                .collect();
            object["font"] = fonts.runs_font(&runs, None, None);
        }
        objects.push(object);
    }
    Ok(objects)
}

/// The part an internal relationship of `part` names, which may be a media
/// part read apart from the XML parts (`images`).
fn relation_target_with(
    parts: &BTreeMap<String, Vec<u8>>,
    images: &BTreeMap<String, Vec<u8>>,
    part: &str,
    id: &str,
) -> Result<String> {
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let rels = format!("{directory}/_rels/{filename}.rels");
    let text = xml_part(parts, &rels)?;
    let xml = Document::parse(&text)?;
    let rel = xml
        .descendants()
        .find(|n| n.has_tag_name((PACKAGE_REL, "Relationship")) && n.attribute("Id") == Some(id))
        .context("missing Office relationship")?;
    ensure!(
        rel.attribute("TargetMode") != Some("External"),
        "external document part is not supported"
    );
    let target = rel
        .attribute("Target")
        .context("relationship target missing")?;
    resolve_target(part, target, |name| {
        parts.contains_key(name) || images.contains_key(name)
    })
}

/// The media parts of a presentation (`ppt/media/`), which the XML parts leave out.
pub(super) fn media(raw: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut archive = super::office_archive(raw)?;
    let mut media = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.name().starts_with("ppt/media/") || entry.is_dir() {
            continue;
        }
        ensure!(
            entry.size() <= 256 * 1024 * 1024,
            "PowerPoint image exceeds size budget"
        );
        let bytes = super::read_entry(&mut entry)?;
        media.insert(entry.name().to_owned(), bytes);
    }
    Ok(media)
}
