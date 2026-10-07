//! Shape operations on PowerPoint slides: a shape, picture or connector
//! changed (place, size, rotation, fill, line, name), added or deleted, and a
//! picture's image replaced. Places and sizes are in points on the slide, as
//! the extraction's drawings give them.
use super::*;
use std::ops::Range;

const EMU_PER_POINT: f64 = 12_700.0;
const SHAPE_KINDS: &[&str] = &[
    "update_shape",
    "add_shape",
    "delete_shape",
    "add_picture",
    "replace_picture",
    "add_connector",
];
/// An edit of a part: a byte range and its replacement.
type Edit = (Range<usize>, String);

pub(super) const IMAGE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

/// Whether `value` is a shape operation.
pub fn is_shape_operation(value: &Value) -> bool {
    value["kind"]
        .as_str()
        .is_some_and(|kind| SHAPE_KINDS.contains(&kind))
}

/// What an operation sets on a shape. Places and sizes are points on the
/// slide, rotation degrees, colours `RRGGBB` or `none`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShapeProperties {
    pub left: Option<f64>,
    pub top: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub rotation: Option<f64>,
    pub fill: Option<String>,
    pub line: Option<String>,
    pub line_weight: Option<f64>,
    pub name: Option<String>,
}

impl ShapeProperties {
    fn parse(value: &Value) -> Result<Self> {
        let Some(object) = value.as_object() else {
            return Ok(Self::default());
        };
        let number = |key: &str, minimum: f64| -> Result<Option<f64>> {
            object
                .get(key)
                .map(|v| {
                    let n = v
                        .as_f64()
                        .with_context(|| format!("shape {key} must be a number"))?;
                    ensure!(
                        n.is_finite() && n >= minimum && n.abs() <= 100_000.0,
                        "shape {key} must be a number of points from {minimum} up to 100000"
                    );
                    Ok(n)
                })
                .transpose()
        };
        let color = |key: &str| -> Result<Option<String>> {
            object
                .get(key)
                .map(|v| {
                    let color =
                        string(v).with_context(|| format!("shape {key} must be RRGGBB or none"))?;
                    ensure!(
                        color == "none"
                            || (color.len() == 6
                                && color
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))),
                        "shape {key} must be six uppercase hexadecimal digits (RRGGBB) or none"
                    );
                    Ok(color.to_owned())
                })
                .transpose()
        };
        let name = object
            .get("name")
            .map(|v| {
                let name = string(v).context("shape name must be a string")?;
                ensure!(!name.trim().is_empty(), "shape name must not be empty");
                Ok::<_, anyhow::Error>(name.to_owned())
            })
            .transpose()?;
        Ok(Self {
            left: number("left", -100_000.0)?,
            top: number("top", -100_000.0)?,
            width: number("width", 0.0)?,
            height: number("height", 0.0)?,
            rotation: object
                .get("rotation")
                .map(|v| v.as_f64().context("shape rotation must be degrees"))
                .transpose()?,
            fill: color("fill")?,
            line: color("line")?,
            line_weight: number("line_weight", 0.0)?,
            name,
        })
    }

    fn placed(&self) -> bool {
        self.left.is_some() && self.top.is_some() && self.width.is_some() && self.height.is_some()
    }
}

/// The two ends of a connector: a shape and its connection site.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorEnd {
    pub shape: String,
    pub site: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShapeEdit {
    Update {
        shape: String,
        properties: ShapeProperties,
    },
    Add {
        name: String,
        shape_type: String,
        properties: ShapeProperties,
        text: Option<String>,
    },
    Delete {
        shape: String,
    },
    AddPicture {
        name: String,
        description: Option<String>,
        asset: String,
        properties: ShapeProperties,
    },
    ReplacePicture {
        shape: String,
        asset: String,
    },
    AddConnector {
        name: String,
        connector_type: String,
        begin: ConnectorEnd,
        end: ConnectorEnd,
        properties: ShapeProperties,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeOperation {
    pub id: String,
    pub slide: String,
    pub edit: ShapeEdit,
}

impl ShapeOperation {
    /// The image asset the operation adds, if any.
    pub fn asset(&self) -> Option<&str> {
        match &self.edit {
            ShapeEdit::AddPicture { asset, .. } | ShapeEdit::ReplacePicture { asset, .. } => {
                Some(asset)
            }
            _ => None,
        }
    }
}

/// The shape operations among `values`, in order.
pub fn parse_shape_operations(values: &[Value]) -> Result<Vec<ShapeOperation>> {
    let mut operations: Vec<ShapeOperation> = vec![];
    for value in values.iter().filter(|value| is_shape_operation(value)) {
        let id = string(&value["id"])?;
        identifier(id)?;
        ensure!(
            !string(&value["reason"])?.trim().is_empty(),
            "operation reason required"
        );
        let slide = string(&value["slide"])?.to_owned();
        let properties = ShapeProperties::parse(&value["properties"])
            .with_context(|| format!("shape operation {id}"))?;
        let name = || -> Result<String> {
            let name = string(&value["name"]).context("the new shape needs a name")?;
            ensure!(!name.trim().is_empty(), "the new shape needs a name");
            Ok(name.to_owned())
        };
        let asset = || -> Result<String> {
            let asset = string(&value["asset"])?;
            ensure!(
                asset.starts_with("assets/")
                    && !asset.contains('\\')
                    && !asset
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == ".."),
                "picture asset must be a project assets path"
            );
            Ok(asset.to_owned())
        };
        let end = |key: &str| -> Result<ConnectorEnd> {
            Ok(ConnectorEnd {
                shape: string(&value[key]["shape"])?.to_owned(),
                site: u32::try_from(
                    value[key]["site"]
                        .as_u64()
                        .context("connection site must be a number")?,
                )?,
            })
        };
        let edit = match string(&value["kind"])? {
            "update_shape" => {
                ensure!(
                    properties != ShapeProperties::default(),
                    "update_shape {id} changes no property"
                );
                ShapeEdit::Update {
                    shape: string(&value["shape"])?.to_owned(),
                    properties,
                }
            }
            "add_shape" => {
                ensure!(
                    properties.placed(),
                    "add_shape {id} takes left, top, width and height"
                );
                let shape_type = string(&value["shape_type"])?.to_owned();
                ensure!(
                    matches!(
                        shape_type.as_str(),
                        "rectangle" | "rounded_rectangle" | "ellipse" | "textbox" | "line"
                    ),
                    "shape_type must be rectangle, rounded_rectangle, ellipse, textbox or line"
                );
                let text = value["text"].as_str().map(str::to_owned);
                ensure!(
                    text.is_none() || shape_type != "line",
                    "a line holds no text"
                );
                ShapeEdit::Add {
                    name: name()?,
                    shape_type,
                    properties,
                    text,
                }
            }
            "delete_shape" => ShapeEdit::Delete {
                shape: string(&value["shape"])?.to_owned(),
            },
            "add_picture" => {
                ensure!(
                    properties.placed(),
                    "add_picture {id} takes left, top, width and height"
                );
                ShapeEdit::AddPicture {
                    name: name()?,
                    description: value["description"].as_str().map(str::to_owned),
                    asset: asset()?,
                    properties,
                }
            }
            "replace_picture" => ShapeEdit::ReplacePicture {
                shape: string(&value["shape"])?.to_owned(),
                asset: asset()?,
            },
            "add_connector" => {
                let connector_type = string(&value["connector_type"])?.to_owned();
                ensure!(
                    matches!(connector_type.as_str(), "straight" | "elbow" | "curve"),
                    "connector_type must be straight, elbow or curve"
                );
                ShapeEdit::AddConnector {
                    name: name()?,
                    connector_type,
                    begin: end("begin")?,
                    end: end("end")?,
                    properties,
                }
            }
            other => bail!("unknown shape operation {other}"),
        };
        operations.push(ShapeOperation {
            id: id.to_owned(),
            slide,
            edit,
        });
    }
    Ok(operations)
}

fn emu(points: f64) -> i64 {
    (points * EMU_PER_POINT).round() as i64
}

/// The non-visual properties (`p:cNvPr`) of an object.
fn properties_of<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.tag_name().name().starts_with("nv"))?
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "cNvPr")))
}

fn object(node: Node<'_, '_>) -> bool {
    node.tag_name().namespace() == Some(PRESENTATION)
        && matches!(
            node.tag_name().name(),
            "sp" | "pic" | "cxnSp" | "grpSp" | "graphicFrame"
        )
}

/// The shape properties element of an object (`p:spPr`, `p:grpSpPr`).
fn shape_properties<'a, 'input>(node: Node<'a, 'input>) -> Option<Node<'a, 'input>> {
    node.children().find(|n| {
        n.has_tag_name((PRESENTATION, "spPr")) || n.has_tag_name((PRESENTATION, "grpSpPr"))
    })
}

/// The group transforms (off, ext, chOff, chExt in EMU) of the groups that
/// hold `node`, outermost first.
fn groups(node: Node<'_, '_>) -> Vec<[f64; 8]> {
    let mut chain = vec![];
    for group in node
        .ancestors()
        .skip(1)
        .filter(|n| n.has_tag_name((PRESENTATION, "grpSp")))
    {
        let Some(xfrm) = shape_properties(group)
            .and_then(|p| p.children().find(|n| n.has_tag_name((DRAWING, "xfrm"))))
        else {
            continue;
        };
        let value = |name: &str, x: &str| {
            xfrm.children()
                .find(|n| n.has_tag_name((DRAWING, name)))
                .and_then(|n| n.attribute(x))
                .and_then(|v| v.parse::<f64>().ok())
        };
        if let (Some(ox), Some(oy), Some(ex), Some(ey), Some(cx), Some(cy), Some(cex), Some(cey)) = (
            value("off", "x"),
            value("off", "y"),
            value("ext", "cx"),
            value("ext", "cy"),
            value("chOff", "x"),
            value("chOff", "y"),
            value("chExt", "cx"),
            value("chExt", "cy"),
        ) {
            chain.push([ox, oy, ex, ey, cx, cy, cex, cey]);
        }
    }
    chain.reverse();
    chain
}

/// A box on the slide (EMU) in the coordinates of the groups holding `node`.
fn to_group(node: Node<'_, '_>, mut area: [f64; 4]) -> [f64; 4] {
    for [ox, oy, ex, ey, cx, cy, cex, cey] in groups(node) {
        let sx = if cex == 0.0 { 1.0 } else { ex / cex };
        let sy = if cey == 0.0 { 1.0 } else { ey / cey };
        area = [
            cx + (area[0] - ox) / sx,
            cy + (area[1] - oy) / sy,
            area[2] / sx,
            area[3] / sy,
        ];
    }
    area
}

/// The children of DrawingML shape properties in schema order (CT_ShapeProperties).
const SHAPE_ORDER: [&str; 15] = [
    "xfrm",
    "custGeom",
    "prstGeom",
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
    "ln",
    "effectLst",
    "effectDag",
    "scene3d",
    "sp3d",
    "extLst",
];
/// The children of line properties in schema order (CT_LineProperties).
const LINE_ORDER: [&str; 12] = [
    "noFill",
    "solidFill",
    "gradFill",
    "pattFill",
    "prstDash",
    "custDash",
    "round",
    "bevel",
    "miter",
    "headEnd",
    "tailEnd",
    "extLst",
];
const FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

fn fill_xml(prefix: &str, color: &str) -> String {
    if color == "none" {
        format!("<{prefix}noFill/>")
    } else {
        format!("<{prefix}solidFill><{prefix}srgbClr val=\"{color}\"/></{prefix}solidFill>")
    }
}

/// `<a:xfrm>` for a box in EMU with a rotation in degrees.
fn xfrm_xml(prefix: &str, area: [f64; 4], rotation: f64, flips: &str) -> String {
    let rotation = (rotation * 60_000.0).round() as i64;
    let rotation = if rotation == 0 {
        String::new()
    } else {
        format!(" rot=\"{rotation}\"")
    };
    format!(
        "<{prefix}xfrm{rotation}{flips}><{prefix}off x=\"{}\" y=\"{}\"/><{prefix}ext cx=\"{}\" cy=\"{}\"/></{prefix}xfrm>",
        area[0].round() as i64,
        area[1].round() as i64,
        area[2].round() as i64,
        area[3].round() as i64
    )
}

/// The edit of an existing object `node` of `source` that `properties` makes.
fn updated(
    source: &str,
    node: Node<'_, '_>,
    properties: &ShapeProperties,
) -> Result<Vec<(Range<usize>, String)>> {
    let mut edits = vec![];
    if let Some(name) = &properties.name {
        let element = properties_of(node).context("shape without properties")?;
        let opening = crate::fonts::opening_tag(source, element)?;
        let renamed = format!(
            "{} name=\"{}\"",
            crate::fonts::without_attribute(opening, "name")?,
            crate::fonts::attribute_text(name)
        );
        edits.push((
            element.range().start..element.range().start + opening.len(),
            renamed,
        ));
    }
    let frame = node.has_tag_name((PRESENTATION, "graphicFrame"));
    let place = properties.left.is_some()
        || properties.top.is_some()
        || properties.width.is_some()
        || properties.height.is_some()
        || properties.rotation.is_some();
    let colors =
        properties.fill.is_some() || properties.line.is_some() || properties.line_weight.is_some();
    if colors {
        ensure!(
            !frame && !node.has_tag_name((PRESENTATION, "grpSp")),
            "fill and line apply to shapes, pictures and connectors; a group or graphic frame has none"
        );
    }
    if !place && !colors {
        return Ok(edits);
    }
    if frame {
        // A graphic frame (table, chart) keeps its place in `p:xfrm`.
        let xfrm = node
            .children()
            .find(|n| n.has_tag_name((PRESENTATION, "xfrm")))
            .context("graphic frame without a place")?;
        let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, xfrm)).to_owned();
        let current = super::slide_drawings::slide_box(node);
        let area = placed_box(current, properties)?;
        let rotation = properties.rotation.unwrap_or_else(|| rotation_of(xfrm));
        let inner = xfrm_xml("a:", area, 0.0, "");
        let inner = &inner[inner.find('>').unwrap() + 1..inner.rfind("</").unwrap()];
        let rotation = (rotation * 60_000.0).round() as i64;
        let rot = if rotation == 0 {
            String::new()
        } else {
            format!(" rot=\"{rotation}\"")
        };
        edits.push((
            xfrm.range(),
            format!("<{prefix}xfrm{rot}>{inner}</{prefix}xfrm>"),
        ));
        return Ok(edits);
    }
    let spec = shape_properties(node).context("shape without shape properties")?;
    let name = crate::fonts::qualified_name(source, spec).to_owned();
    let prefix = "a:";
    let mut children = crate::fonts::children_of(source, spec);
    if place {
        let current = super::slide_drawings::slide_box(node);
        let area = placed_box(current, properties)?;
        let old = spec.children().find(|n| n.has_tag_name((DRAWING, "xfrm")));
        let rotation = properties
            .rotation
            .unwrap_or_else(|| old.map_or(0.0, rotation_of));
        let flips: String = ["flipH", "flipV"]
            .iter()
            .filter_map(|flip| {
                old.and_then(|o| o.attribute(*flip))
                    .map(|v| format!(" {flip}=\"{v}\""))
            })
            .collect();
        let mut xfrm = xfrm_xml(prefix, to_group(node, area), rotation, &flips);
        if let Some(old) = old {
            // A group keeps the child extent its shapes are laid out in.
            let keep: String = old
                .children()
                .filter(|n| {
                    n.has_tag_name((DRAWING, "chOff")) || n.has_tag_name((DRAWING, "chExt"))
                })
                .map(|n| source[n.range()].to_owned())
                .collect();
            if !keep.is_empty() {
                xfrm = xfrm.replacen(
                    &format!("</{prefix}xfrm>"),
                    &format!("{keep}</{prefix}xfrm>"),
                    1,
                );
            }
        }
        children.retain(|(child, _)| child != "xfrm");
        crate::fonts::insert_ordered(&mut children, ("xfrm".into(), xfrm), &SHAPE_ORDER);
    }
    if let Some(fill) = &properties.fill {
        children.retain(|(child, _)| !FILLS.contains(&child.as_str()));
        crate::fonts::insert_ordered(
            &mut children,
            ("solidFill".into(), fill_xml(prefix, fill)),
            &SHAPE_ORDER,
        );
    }
    if properties.line.is_some() || properties.line_weight.is_some() {
        let existing = spec.children().find(|n| n.has_tag_name((DRAWING, "ln")));
        let mut line_children = existing
            .map(|l| crate::fonts::children_of(source, l))
            .unwrap_or_default();
        let mut opening = match existing {
            Some(line) => crate::fonts::opening_tag(source, line)?.to_owned(),
            None => format!("<{prefix}ln"),
        };
        if let Some(weight) = properties.line_weight {
            opening = format!(
                "{} w=\"{}\"",
                crate::fonts::without_attribute(&opening, "w")?,
                emu(weight)
            );
        }
        if let Some(color) = &properties.line {
            line_children.retain(|(child, _)| !FILLS.contains(&child.as_str()));
            crate::fonts::insert_ordered(
                &mut line_children,
                ("solidFill".into(), fill_xml(prefix, color)),
                &LINE_ORDER,
            );
        }
        let line = if line_children.is_empty() {
            format!("{opening}/>")
        } else {
            format!(
                "{opening}>{}</{prefix}ln>",
                line_children
                    .into_iter()
                    .map(|(_, xml)| xml)
                    .collect::<String>()
            )
        };
        children.retain(|(child, _)| child != "ln");
        crate::fonts::insert_ordered(&mut children, ("ln".into(), line), &SHAPE_ORDER);
    }
    let opening = crate::fonts::opening_tag(source, spec)?;
    let content: String = children.into_iter().map(|(_, xml)| xml).collect();
    edits.push((spec.range(), format!("{opening}>{content}</{name}>")));
    Ok(edits)
}

fn rotation_of(xfrm: Node<'_, '_>) -> f64 {
    xfrm.attribute("rot")
        .and_then(|r| r.parse::<f64>().ok())
        .map_or(0.0, |r| r / 60_000.0)
}

/// The new box of an object (EMU on the slide): what `properties` set, the
/// rest as it is. A placeholder that takes its place from the layout has none.
fn placed_box(current: Option<[f64; 4]>, properties: &ShapeProperties) -> Result<[f64; 4]> {
    let current = match current {
        Some(current) => current,
        None => {
            ensure!(
                properties.placed()
                    || (properties.left.is_none()
                        && properties.top.is_none()
                        && properties.width.is_none()
                        && properties.height.is_none()),
                "the shape takes its place from the slide layout; give left, top, width and height together"
            );
            [0.0; 4]
        }
    };
    let set = |value: Option<f64>, old: f64| value.map_or(old, |v| v * EMU_PER_POINT);
    Ok([
        set(properties.left, current[0]),
        set(properties.top, current[1]),
        set(properties.width, current[2]),
        set(properties.height, current[3]),
    ])
}

/// A connection site of a shape with geometry `geometry` in box `area`
/// (EMU, with rotation in degrees), as DrawingML numbers them.
fn site(geometry: &str, area: [f64; 4], rotation: f64, site: u32) -> Result<(f64, f64)> {
    let [x, y, w, h] = area;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let local = match geometry {
        "rect" | "roundRect" => match site {
            0 => (cx, y),
            1 => (x, cy),
            2 => (cx, y + h),
            3 => (x + w, cy),
            _ => bail!(
                "a rectangle has connection sites 0 (top), 1 (left), 2 (bottom) and 3 (right)"
            ),
        },
        "ellipse" => {
            ensure!(
                site < 8,
                "an ellipse has connection sites 0 (top) to 7, counterclockwise"
            );
            // Counterclockwise from the top, every 45 degrees.
            let angle = std::f64::consts::FRAC_PI_2 + f64::from(site) * std::f64::consts::FRAC_PI_4;
            (cx + w / 2.0 * angle.cos(), cy - h / 2.0 * angle.sin())
        }
        other => bail!(
            "connectors join rectangles, rounded rectangles and ellipses; the shape is {other}"
        ),
    };
    if rotation == 0.0 {
        return Ok(local);
    }
    let (sin, cos) = rotation.to_radians().sin_cos();
    let (dx, dy) = (local.0 - cx, local.1 - cy);
    Ok((cx + dx * cos - dy * sin, cy + dx * sin + dy * cos))
}

/// The DrawingML preset geometry of a new shape's type.
fn geometry(shape_type: &str) -> &'static str {
    match shape_type {
        "rounded_rectangle" => "roundRect",
        "ellipse" => "ellipse",
        "line" => "line",
        _ => "rect",
    }
}

/// A shape known on the slide: one there or one an earlier operation adds.
struct Known {
    id: u64,
    geometry: String,
    area: [f64; 4],
    rotation: f64,
}

/// The edits the shape `operations` make to slide `part` (`source`, `xml`).
/// `origin` is the part the slide was copied from, whose drawing IDs name its
/// shapes too; `blocks` are its text blocks, whose shapes cannot be deleted;
/// `pictures` gives the relationship ID of each picture asset in the slide.
pub(super) fn slide_edits(
    source: &str,
    xml: &Document<'_>,
    part: &str,
    origin: &str,
    operations: &[&ShapeOperation],
    blocks: &[super::word::Block<'_, '_>],
    pictures: &BTreeMap<String, String>,
) -> Result<(Vec<Edit>, BTreeMap<String, u64>)> {
    let tree = xml
        .descendants()
        .find(|n| n.has_tag_name((PRESENTATION, "spTree")))
        .context("slide without a shape tree")?;
    let mut by_id: BTreeMap<u64, Node<'_, '_>> = BTreeMap::new();
    for node in tree
        .descendants()
        .filter(|n| object(*n) && !hidden_copy(*n, "pptx"))
    {
        if let Some(id) = properties_of(node)
            .and_then(|p| p.attribute("id"))
            .and_then(|id| id.parse().ok())
        {
            by_id.insert(id, node);
        }
    }
    let mut next = xml
        .descendants()
        .filter(|n| n.has_tag_name((PRESENTATION, "cNvPr")))
        .filter_map(|n| n.attribute("id")?.parse::<u64>().ok())
        .max()
        .unwrap_or(1)
        + 1;
    let mut known: BTreeMap<String, Known> = BTreeMap::new();
    let resolve = |name: &str, known: &BTreeMap<String, Known>| -> Result<u64> {
        if let Some(added) = known.get(name) {
            return Ok(added.id);
        }
        let (owner, id) = name
            .rsplit_once('#')
            .with_context(|| format!("{name} is not a shape: name it by its drawing ID (<slide part>#<id>) or by the operation that adds it"))?;
        ensure!(
            owner == part || owner == origin,
            "{name} is a shape of another slide"
        );
        let id: u64 = id
            .parse()
            .with_context(|| format!("{name} is not a drawing ID"))?;
        ensure!(
            by_id.contains_key(&id),
            "{name} is not a shape of the slide"
        );
        Ok(id)
    };
    let block_shapes: BTreeSet<usize> = blocks
        .iter()
        .flat_map(|b| b.runs.iter().copied().chain(b.empty))
        .filter_map(|n| {
            n.ancestors()
                .find(|a| object(*a) && !a.has_tag_name((PRESENTATION, "grpSp")))
        })
        .map(|n| n.range().start)
        .collect();
    let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, tree)).to_owned();
    let end = tree
        .children()
        .find(|n| n.has_tag_name((PRESENTATION, "extLst")))
        .map_or_else(
            || tree.range().start + source[tree.range()].rfind("</").unwrap_or(0),
            |n| n.range().start,
        );
    let mut edits = vec![];
    let mut deleted = BTreeSet::new();
    let mut added = String::new();
    // Boxes (EMU on the slide) and rotations earlier operations give shapes.
    let mut moved: BTreeMap<u64, ([f64; 4], f64)> = BTreeMap::new();
    for operation in operations {
        let context = || format!("shape operation {}", operation.id);
        match &operation.edit {
            ShapeEdit::Update { shape, properties } => {
                let id = resolve(shape, &known).with_context(context)?;
                let node = *by_id.get(&id).with_context(|| {
                    format!("{shape} is added by an earlier operation; set its properties there")
                })?;
                edits.extend(updated(source, node, properties).with_context(context)?);
                let current = moved
                    .get(&id)
                    .map(|(area, _)| *area)
                    .or_else(|| super::slide_drawings::slide_box(node));
                if let Ok(area) = placed_box(current, properties) {
                    let rotation = properties.rotation.unwrap_or_else(|| {
                        moved.get(&id).map_or_else(
                            || {
                                shape_properties(node)
                                    .and_then(|p| {
                                        p.children().find(|n| n.has_tag_name((DRAWING, "xfrm")))
                                    })
                                    .map_or(0.0, rotation_of)
                            },
                            |(_, rotation)| *rotation,
                        )
                    });
                    moved.insert(id, (area, rotation));
                }
            }
            ShapeEdit::Delete { shape } => {
                let id = resolve(shape, &known).with_context(context)?;
                let node = *by_id.get(&id).with_context(|| {
                    format!(
                        "{shape} is added by an earlier operation; remove that operation instead"
                    )
                })?;
                ensure!(
                    !node
                        .descendants()
                        .any(|n| block_shapes.contains(&n.range().start)),
                    "{shape} shows text of the slide's content; delete its paragraphs with documents rows delete and apply first, or delete the shape in PowerPoint"
                );
                let raw_id = id.to_string();
                // A connector an earlier operation deletes joins nothing.
                ensure!(
                    !tree.descendants().any(|n| {
                        (n.has_tag_name((DRAWING, "stCxn")) || n.has_tag_name((DRAWING, "endCxn")))
                            && n.attribute("id") == Some(raw_id.as_str())
                            && !n
                                .ancestors()
                                .find(|a| object(*a))
                                .and_then(properties_of)
                                .and_then(|p| p.attribute("id"))
                                .and_then(|id| id.parse::<u64>().ok())
                                .is_some_and(|id| deleted.contains(&id))
                    }),
                    "{shape} is joined by a connector; delete the connector first"
                );
                ensure!(deleted.insert(id), "{shape} is deleted twice");
                edits.push((node.range(), String::new()));
            }
            ShapeEdit::Add {
                name,
                shape_type,
                properties,
                text,
            } => {
                let id = next;
                next += 1;
                let area = [
                    properties.left.unwrap() * EMU_PER_POINT,
                    properties.top.unwrap() * EMU_PER_POINT,
                    properties.width.unwrap() * EMU_PER_POINT,
                    properties.height.unwrap() * EMU_PER_POINT,
                ];
                let rotation = properties.rotation.unwrap_or(0.0);
                let geometry = geometry(shape_type);
                let mut spec = format!(
                    "{}<a:prstGeom prst=\"{geometry}\"><a:avLst/></a:prstGeom>",
                    xfrm_xml("a:", area, rotation, "")
                );
                if let Some(fill) = &properties.fill {
                    spec.push_str(&fill_xml("a:", fill));
                } else if shape_type == "textbox" {
                    spec.push_str("<a:noFill/>");
                }
                if properties.line.is_some() || properties.line_weight.is_some() {
                    let weight = properties
                        .line_weight
                        .map(|w| format!(" w=\"{}\"", emu(w)))
                        .unwrap_or_default();
                    let color = properties
                        .line
                        .as_deref()
                        .map(|c| fill_xml("a:", c))
                        .unwrap_or_default();
                    spec.push_str(&format!("<a:ln{weight}>{color}</a:ln>"));
                }
                // New shapes take PowerPoint's default shape style: accent fill,
                // darker outline and light text; a text box has none.
                let style = if shape_type == "textbox" {
                    String::new()
                } else {
                    format!(
                        "<{prefix}style><a:lnRef idx=\"{}\"><a:schemeClr val=\"accent1\"><a:shade val=\"50000\"/></a:schemeClr></a:lnRef><a:fillRef idx=\"{}\"><a:schemeClr val=\"accent1\"/></a:fillRef><a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"{}\"/></a:fontRef></{prefix}style>",
                        if shape_type == "line" { 1 } else { 2 },
                        if shape_type == "line" { 0 } else { 1 },
                        if shape_type == "line" { "tx1" } else { "lt1" },
                    )
                };
                let body = if shape_type == "line" {
                    String::new()
                } else {
                    let paragraphs: String = text
                        .as_deref()
                        .unwrap_or("")
                        .split('\n')
                        .map(|line| {
                            if line.is_empty() {
                                Ok("<a:p><a:endParaRPr lang=\"ja-JP\"/></a:p>".to_owned())
                            } else {
                                Ok(format!(
                                    "<a:p><a:r><a:rPr lang=\"ja-JP\"/><a:t>{}</a:t></a:r></a:p>",
                                    xml_text(line)?
                                ))
                            }
                        })
                        .collect::<Result<_>>()?;
                    let anchor = if shape_type == "textbox" {
                        ""
                    } else {
                        " anchor=\"ctr\""
                    };
                    format!(
                        "<{prefix}txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"{anchor}/><a:lstStyle/>{paragraphs}</{prefix}txBody>"
                    )
                };
                let text_box = if shape_type == "textbox" {
                    " txBox=\"1\""
                } else {
                    ""
                };
                added.push_str(&format!(
                    "<{prefix}sp><{prefix}nvSpPr><{prefix}cNvPr id=\"{id}\" name=\"{}\"/><{prefix}cNvSpPr{text_box}/><{prefix}nvPr/></{prefix}nvSpPr><{prefix}spPr>{spec}</{prefix}spPr>{style}{body}</{prefix}sp>",
                    crate::fonts::attribute_text(name)
                ));
                known.insert(
                    operation.id.clone(),
                    Known {
                        id,
                        geometry: geometry.to_owned(),
                        area,
                        rotation,
                    },
                );
            }
            ShapeEdit::AddPicture {
                name,
                description,
                asset,
                properties,
            } => {
                let id = next;
                next += 1;
                let relationship = pictures.get(asset).context("picture asset missing")?;
                let area = [
                    properties.left.unwrap() * EMU_PER_POINT,
                    properties.top.unwrap() * EMU_PER_POINT,
                    properties.width.unwrap() * EMU_PER_POINT,
                    properties.height.unwrap() * EMU_PER_POINT,
                ];
                let description = description
                    .as_deref()
                    .map(|d| format!(" descr=\"{}\"", crate::fonts::attribute_text(d)))
                    .unwrap_or_default();
                added.push_str(&format!(
                    "<{prefix}pic><{prefix}nvPicPr><{prefix}cNvPr id=\"{id}\" name=\"{}\"{description}/><{prefix}cNvPicPr><a:picLocks noChangeAspect=\"1\"/></{prefix}cNvPicPr><{prefix}nvPr/></{prefix}nvPicPr><{prefix}blipFill><a:blip r:embed=\"{relationship}\"/><a:stretch><a:fillRect/></a:stretch></{prefix}blipFill><{prefix}spPr>{}<a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></{prefix}spPr></{prefix}pic>",
                    crate::fonts::attribute_text(name),
                    xfrm_xml("a:", area, properties.rotation.unwrap_or(0.0), "")
                ));
                known.insert(
                    operation.id.clone(),
                    Known {
                        id,
                        geometry: "rect".into(),
                        area,
                        rotation: properties.rotation.unwrap_or(0.0),
                    },
                );
            }
            ShapeEdit::ReplacePicture { shape, asset } => {
                let id = resolve(shape, &known).with_context(context)?;
                let node = *by_id.get(&id).with_context(|| {
                    format!("{shape} is added by an earlier operation; give the new image there")
                })?;
                ensure!(
                    node.has_tag_name((PRESENTATION, "pic")),
                    "{shape} is not a picture"
                );
                let blip = node
                    .descendants()
                    .find(|n| n.has_tag_name((DRAWING, "blip")))
                    .context("picture without an image")?;
                let attribute = blip
                    .attributes()
                    .find(|a| a.namespace() == Some(REL) && a.name() == "embed")
                    .context("picture without an embedded image")?;
                let relationship = pictures.get(asset).context("picture asset missing")?;
                let name = source[attribute.range()]
                    .split('=')
                    .next()
                    .unwrap_or("r:embed")
                    .trim()
                    .to_owned();
                edits.push((attribute.range(), format!("{name}=\"{relationship}\"")));
            }
            ShapeEdit::AddConnector {
                name,
                connector_type,
                begin,
                end: finish,
                properties,
            } => {
                let geometry_of = |end: &ConnectorEnd,
                                   known: &BTreeMap<String, Known>|
                 -> Result<(u64, (f64, f64))> {
                    let id = resolve(&end.shape, known)?;
                    if let Some(added) = known.get(&end.shape) {
                        return Ok((
                            id,
                            site(&added.geometry, added.area, added.rotation, end.site)?,
                        ));
                    }
                    let node = by_id[&id];
                    if let Some((area, rotation)) = moved.get(&id) {
                        let geometry = shape_properties(node)
                            .and_then(|p| {
                                p.children().find(|n| n.has_tag_name((DRAWING, "prstGeom")))
                            })
                            .and_then(|n| n.attribute("prst"))
                            .unwrap_or("custom");
                        return Ok((id, site(geometry, *area, *rotation, end.site)?));
                    }
                    let geometry = shape_properties(node)
                        .and_then(|p| p.children().find(|n| n.has_tag_name((DRAWING, "prstGeom"))))
                        .and_then(|n| n.attribute("prst"))
                        .unwrap_or("custom");
                    let area = super::slide_drawings::slide_box(node).with_context(|| {
                        format!(
                            "{} takes its place from the slide layout; place it first",
                            end.shape
                        )
                    })?;
                    let rotation = shape_properties(node)
                        .and_then(|p| p.children().find(|n| n.has_tag_name((DRAWING, "xfrm"))))
                        .map_or(0.0, rotation_of);
                    Ok((id, site(geometry, area, rotation, end.site)?))
                };
                let (from_id, from) = geometry_of(begin, &known).with_context(context)?;
                let (to_id, to) = geometry_of(finish, &known).with_context(context)?;
                let id = next;
                next += 1;
                let area = [
                    from.0.min(to.0),
                    from.1.min(to.1),
                    (to.0 - from.0).abs(),
                    (to.1 - from.1).abs(),
                ];
                let mut flips = String::new();
                if to.0 < from.0 {
                    flips.push_str(" flipH=\"1\"");
                }
                if to.1 < from.1 {
                    flips.push_str(" flipV=\"1\"");
                }
                let preset = match connector_type.as_str() {
                    "elbow" => "bentConnector3",
                    "curve" => "curvedConnector3",
                    _ => "straightConnector1",
                };
                let mut line = String::new();
                if properties.line.is_some() || properties.line_weight.is_some() {
                    let weight = properties
                        .line_weight
                        .map(|w| format!(" w=\"{}\"", emu(w)))
                        .unwrap_or_default();
                    let color = properties
                        .line
                        .as_deref()
                        .map(|c| fill_xml("a:", c))
                        .unwrap_or_default();
                    line = format!("<a:ln{weight}>{color}</a:ln>");
                }
                added.push_str(&format!(
                    "<{prefix}cxnSp><{prefix}nvCxnSpPr><{prefix}cNvPr id=\"{id}\" name=\"{}\"/><{prefix}cNvCxnSpPr><a:stCxn id=\"{from_id}\" idx=\"{}\"/><a:endCxn id=\"{to_id}\" idx=\"{}\"/></{prefix}cNvCxnSpPr><{prefix}nvPr/></{prefix}nvCxnSpPr><{prefix}spPr>{}<a:prstGeom prst=\"{preset}\"><a:avLst/></a:prstGeom>{line}</{prefix}spPr><{prefix}style><a:lnRef idx=\"1\"><a:schemeClr val=\"accent1\"/></a:lnRef><a:fillRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:fillRef><a:effectRef idx=\"0\"><a:schemeClr val=\"accent1\"/></a:effectRef><a:fontRef idx=\"minor\"><a:schemeClr val=\"tx1\"/></a:fontRef></{prefix}style></{prefix}cxnSp>",
                    crate::fonts::attribute_text(name),
                    begin.site,
                    finish.site,
                    xfrm_xml("a:", area, 0.0, &flips)
                ));
                known.insert(
                    operation.id.clone(),
                    Known {
                        id,
                        geometry: preset.to_owned(),
                        area,
                        rotation: 0.0,
                    },
                );
            }
        }
    }
    if !added.is_empty() {
        edits.push((end..end, added));
    }
    // Shapes inside a deleted group go with it.
    edits.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut kept: Vec<(Range<usize>, String)> = vec![];
    for edit in edits {
        if let Some(last) = kept.last()
            && last.1.is_empty()
            && last.0.start <= edit.0.start
            && edit.0.end <= last.0.end
            && !(edit.0.start == edit.0.end && edit.0.start == last.0.end)
        {
            continue;
        }
        kept.push(edit);
    }
    let added = known
        .into_iter()
        .map(|(id, shape)| (id, shape.id))
        .collect();
    Ok((kept, added))
}

/// Reads the written presentation back and checks the shapes `operations`
/// change, add and delete on its slides (`order` names them as written).
/// `added` gives the slide and drawing ID of each shape an operation adds.
pub(super) fn ensure_read_back(
    output: &Path,
    order: &[String],
    operations: &[ShapeOperation],
    added: &BTreeMap<String, (String, u64)>,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let written = Source::open(output)?;
    let sheets = written.sheets();
    let drawings = |slide: &str| -> Result<Vec<Value>> {
        let index = order
            .iter()
            .position(|name| name == slide)
            .with_context(|| format!("{slide} is not in the written presentation"))?;
        Ok(sheets
            .iter()
            .find(|sheet| sheet["name"] == format!("slide-{}", index + 1))
            .and_then(|sheet| sheet["drawings"].as_array().cloned())
            .unwrap_or_default())
    };
    let number = |operation: &ShapeOperation, shape: &str| -> Option<u64> {
        added
            .get(shape)
            .map(|(_, id)| *id)
            .or_else(|| shape.rsplit_once('#').and_then(|(_, id)| id.parse().ok()))
            .or_else(|| added.get(&operation.id).map(|(_, id)| *id))
    };
    let find = |list: &[Value], id: u64| -> Option<Value> {
        list.iter()
            .find(|d| {
                d["id"]
                    .as_str()
                    .is_some_and(|i| i.rsplit_once('#').is_some_and(|(_, n)| n == id.to_string()))
            })
            .cloned()
    };
    let deleted: BTreeSet<(String, u64)> = operations
        .iter()
        .filter_map(|o| match &o.edit {
            ShapeEdit::Delete { shape } => number(o, shape).map(|id| (o.slide.clone(), id)),
            _ => None,
        })
        .collect();
    let close = |value: &Value, expected: Option<f64>| {
        expected.is_none_or(|e| value.as_f64().is_some_and(|v| (v - e).abs() < 0.01))
    };
    for operation in operations {
        let list = drawings(&operation.slide)?;
        let fail = |what: &str| -> anyhow::Error {
            anyhow::anyhow!(
                "shape operation {} does not read back: {what}; nothing is written",
                operation.id
            )
        };
        let check_properties = |drawing: &Value, properties: &ShapeProperties| -> Result<()> {
            let anchor = &drawing["anchor"];
            ensure!(
                close(&anchor["left"], properties.left)
                    && close(&anchor["top"], properties.top)
                    && close(&anchor["width"], properties.width)
                    && close(&anchor["height"], properties.height)
                    && close(&anchor["rotation"], properties.rotation),
                fail(&format!("its place is {anchor}"))
            );
            if let Some(fill) = &properties.fill {
                ensure!(
                    drawing["fill"] == fill.as_str(),
                    fail(&format!("its fill is {}", drawing["fill"]))
                );
            }
            if let Some(line) = &properties.line {
                ensure!(
                    drawing["line"]["color"] == line.as_str(),
                    fail(&format!("its line is {}", drawing["line"]))
                );
            }
            ensure!(
                close(&drawing["line"]["width"], properties.line_weight),
                fail(&format!("its line is {}", drawing["line"]))
            );
            if let Some(name) = &properties.name {
                ensure!(drawing["name"] == name.as_str(), fail("its name differs"));
            }
            Ok(())
        };
        let image = |drawing: &Value, asset: &str| -> Result<()> {
            let bytes = assets.get(asset).context("picture asset missing")?;
            ensure!(
                drawing["image"]["sha256"] == hash(bytes).as_str(),
                fail("its image differs from the asset")
            );
            Ok(())
        };
        match &operation.edit {
            ShapeEdit::Update { shape, properties } => {
                let id = number(operation, shape).context("shape ID")?;
                if deleted.contains(&(operation.slide.clone(), id)) {
                    continue;
                }
                let drawing = find(&list, id).ok_or_else(|| fail("the shape is missing"))?;
                check_properties(&drawing, properties)?;
            }
            ShapeEdit::Delete { shape } => {
                let id = number(operation, shape).context("shape ID")?;
                ensure!(find(&list, id).is_none(), fail("the shape is still there"));
            }
            ShapeEdit::Add {
                name, properties, ..
            }
            | ShapeEdit::AddConnector {
                name, properties, ..
            } => {
                let id = added
                    .get(&operation.id)
                    .map(|(_, id)| *id)
                    .context("added shape ID")?;
                let drawing = find(&list, id).ok_or_else(|| fail("the new shape is missing"))?;
                ensure!(drawing["name"] == name.as_str(), fail("its name differs"));
                check_properties(&drawing, properties)?;
                if let ShapeEdit::AddConnector { begin, end, .. } = &operation.edit {
                    let targets: Vec<Option<u64>> = drawing["connections"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| {
                            c["target"]
                                .as_str()
                                .and_then(|t| t.rsplit_once('#'))
                                .and_then(|(_, n)| n.parse().ok())
                        })
                        .collect();
                    ensure!(
                        targets
                            == [
                                number(operation, &begin.shape),
                                number(operation, &end.shape)
                            ],
                        fail("it joins other shapes")
                    );
                }
            }
            ShapeEdit::AddPicture {
                name,
                asset,
                properties,
                ..
            } => {
                let id = added
                    .get(&operation.id)
                    .map(|(_, id)| *id)
                    .context("added picture ID")?;
                let drawing = find(&list, id).ok_or_else(|| fail("the new picture is missing"))?;
                ensure!(drawing["name"] == name.as_str(), fail("its name differs"));
                check_properties(&drawing, properties)?;
                image(&drawing, asset)?;
            }
            ShapeEdit::ReplacePicture { shape, asset } => {
                let id = number(operation, shape).context("shape ID")?;
                if deleted.contains(&(operation.slide.clone(), id)) {
                    continue;
                }
                let drawing = find(&list, id).ok_or_else(|| fail("the picture is missing"))?;
                image(&drawing, asset)?;
            }
        }
    }
    Ok(())
}
