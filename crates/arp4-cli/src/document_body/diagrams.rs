//! Diagrams of a content page: drawings joined by lines, written as one
//! `diagram` element at the place of the first of their elements. The texts
//! of its shapes and lines are its only values; its shapes, where they are and
//! what each line joins follow the original's drawings. A check box is
//! written the same way as one `checkbox` element: its label, the only value
//! it holds, with its state and the cell it is linked to.
use super::*;

/// The kind of the element a diagram is written as.
pub(crate) const DIAGRAM: &str = "diagram";
/// The kind of the element a check box is written as.
pub(crate) const CHECKBOX: &str = "checkbox";

/// What a check box shows besides its label: whether the original saved it
/// checked (`None` when mixed, neither on nor off), and the cell it is
/// linked to, as the original names it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CheckBox {
    pub(crate) checked: Option<bool>,
    pub(crate) linked_cell: Option<String>,
}

/// A shape of a diagram: its key, the name of its geometry, where it is on
/// the page, the shape holding it, and the elements of its text, by their
/// places in a page's metadata (`T` = `usize`) or by their IDs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiagramShape<T = usize> {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) at: Option<String>,
    pub(crate) inside: Option<String>,
    pub(crate) texts: Vec<T>,
}

/// What an end of a line meets: a shape of the diagram by its key, or
/// something else (a cell, a picture, nothing) by its label; `near` when the
/// original does not connect it and it meets what it touches or lies on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LineEnd {
    Shape { key: String, near: bool },
    Other { label: String, near: bool },
}

/// The arrowheads of a line: at its end, at its start, at both, or none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Arrow {
    Forward,
    Backward,
    Both,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiagramLine<T = usize> {
    pub(crate) start: LineEnd,
    pub(crate) end: LineEnd,
    pub(crate) arrow: Arrow,
    pub(crate) dashed: bool,
    pub(crate) texts: Vec<T>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Diagram<T = usize> {
    pub(crate) at: Option<String>,
    pub(crate) shapes: Vec<DiagramShape<T>>,
    pub(crate) lines: Vec<DiagramLine<T>>,
    /// Set for a check box, written alone as a `checkbox` element: its one
    /// shape is the box and holds the label, and it has no lines.
    pub(crate) check_box: Option<CheckBox>,
    /// Its elements: its drawings, and for a page the texts of its shapes
    /// and lines too.
    pub(crate) members: Vec<T>,
}

/// The diagrams of a page, by the places of their elements.
#[derive(Default)]
pub(crate) struct Diagrams {
    list: Vec<Diagram>,
    of: HashMap<usize, usize>,
}

impl Diagrams {
    pub(crate) fn new(mut list: Vec<Diagram>) -> Result<Self> {
        let mut of = HashMap::new();
        for (index, diagram) in list.iter_mut().enumerate() {
            diagram.members.sort_unstable();
            diagram.members.dedup();
            ensure!(!diagram.members.is_empty(), "diagram without elements");
            for &place in &diagram.members {
                ensure!(of.insert(place, index).is_none(), "element in two diagrams");
            }
        }
        Ok(Self { list, of })
    }

    /// The diagram holding the element at `place`, and whether the element
    /// is its first, where the diagram is written.
    pub(crate) fn of(&self, place: usize) -> Option<(&Diagram, bool)> {
        let diagram = &self.list[*self.of.get(&place)?];
        Some((diagram, diagram.members[0] == place))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A value written with its members in a set order.
pub(crate) enum Ordered<'v> {
    Map(Vec<(&'static str, Ordered<'v>)>),
    List(Vec<Ordered<'v>>),
    Text(String),
    Flag(Option<bool>),
    Value(&'v Value),
}

impl Ordered<'_> {
    fn to_json(&self) -> Value {
        match self {
            Self::Map(members) => Value::Object(
                members
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.to_json()))
                    .collect(),
            ),
            Self::List(items) => Value::Array(items.iter().map(Ordered::to_json).collect()),
            Self::Text(text) => Value::String(text.clone()),
            Self::Flag(flag) => json!(flag),
            Self::Value(value) => (*value).clone(),
        }
    }
}

impl Serialize for Ordered<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Map(members) => {
                let mut map = s.serialize_map(Some(members.len()))?;
                for (key, value) in members {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Self::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Text(text) => s.serialize_str(text),
            Self::Flag(flag) => flag.serialize(s),
            Self::Value(value) => value.serialize(s),
        }
    }
}

/// A text value as a label shows it: on one line.
fn shown(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Diagram {
    /// The kind of the element it is written as.
    pub(crate) fn kind(&self) -> &'static str {
        if self.check_box.is_some() {
            CHECKBOX
        } else {
            DIAGRAM
        }
    }

    /// Why an element of it may not be written apart from it.
    pub(crate) fn written_apart(&self) -> &'static str {
        if self.check_box.is_some() {
            "a check box is written as one checkbox element with its label; edit the label there"
        } else {
            "the drawings lines join are written as one diagram; edit their texts there"
        }
    }

    /// Where the diagram is, as messages name it.
    fn name(&self) -> String {
        let kind = if self.check_box.is_some() {
            "check box"
        } else {
            "diagram"
        };
        match (&self.at, self.shapes.first()) {
            (Some(at), _) => format!("{kind} at {at}"),
            (None, Some(shape)) => format!("{kind} of {}", shape.key),
            (None, None) => kind.into(),
        }
    }

    /// The body of the diagram, `text` giving the value of each element of
    /// a text by its place.
    pub(crate) fn body<'v>(
        &self,
        text: &dyn Fn(usize) -> Result<&'v Value>,
    ) -> Result<Ordered<'v>> {
        let texts = |places: &[usize]| -> Result<Option<Ordered<'v>>> {
            Ok(match places {
                [] => None,
                [place] => Some(Ordered::Value(text(*place)?)),
                places => Some(Ordered::List(
                    places
                        .iter()
                        .map(|place| Ok(Ordered::Value(text(*place)?)))
                        .collect::<Result<_>>()?,
                )),
            })
        };
        if let Some(check_box) = &self.check_box {
            let mut members = vec![];
            if let Some(text) = texts(self.shapes.first().map_or(&[], |s| &s.texts))? {
                members.push(("text", text));
            }
            members.push(("checked", Ordered::Flag(check_box.checked)));
            if let Some(cell) = &check_box.linked_cell {
                members.push(("linked_cell", Ordered::Text(cell.clone())));
            }
            if let Some(at) = &self.at {
                members.push(("at", Ordered::Text(at.clone())));
            }
            return Ok(Ordered::Map(members));
        }
        // A shape by its text, or by its key when it has none or another
        // shape shows the same.
        let mut labels: HashMap<&str, String> = HashMap::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for shape in &self.shapes {
            let label = shape
                .texts
                .iter()
                .map(|&place| Ok(shown(text(place)?)))
                .collect::<Result<Vec<_>>>()?
                .join(" ")
                .trim()
                .to_owned();
            *counts.entry(label.clone()).or_default() += 1;
            labels.insert(&shape.key, label);
        }
        let label = |key: &str| -> String {
            match labels.get(key) {
                Some(label) if label.is_empty() => key.to_owned(),
                Some(label) if counts[label] > 1 => format!("{label}({key})"),
                Some(label) => label.clone(),
                None => key.to_owned(),
            }
        };
        let end_label = |end: &LineEnd| -> (String, bool) {
            match end {
                LineEnd::Shape { key, near } => (label(key), *near),
                LineEnd::Other { label, near } => (label.clone(), *near),
            }
        };
        let mut shapes = vec![];
        for shape in &self.shapes {
            let mut members = vec![
                ("id", Ordered::Text(shape.key.clone())),
                ("shape", Ordered::Text(shape.name.clone())),
            ];
            if let Some(at) = &shape.at {
                members.push(("at", Ordered::Text(at.clone())));
            }
            if let Some(inside) = &shape.inside {
                members.push(("in", Ordered::Text(inside.clone())));
            }
            if let Some(text) = texts(&shape.texts)? {
                members.push(("text", text));
            }
            shapes.push(Ordered::Map(members));
        }
        let mut lines = vec![];
        for line in &self.lines {
            let (start, end) = (end_label(&line.start), end_label(&line.end));
            let (first, second, mark) = match line.arrow {
                Arrow::Forward => (start, end, "→"),
                Arrow::Backward => (end, start, "→"),
                Arrow::Both => (start, end, "↔"),
                Arrow::None => (start, end, "—"),
            };
            let mut written = format!("{} {mark} {}", first.0, second.0);
            let mut notes = vec![];
            if line.dashed {
                notes.push("破線".to_owned());
            }
            match (first.1, second.1) {
                (true, true) => notes.push("両端は近接で推定".into()),
                (true, false) => notes.push(format!("{}側は近接で推定", first.0)),
                (false, true) => notes.push(format!("{}側は近接で推定", second.0)),
                (false, false) => {}
            }
            if !notes.is_empty() {
                written.push_str(&format!("（{}）", notes.join("・")));
            }
            lines.push(match texts(&line.texts)? {
                Some(text) => Ordered::Map(vec![("line", Ordered::Text(written)), ("text", text)]),
                None => Ordered::Text(written),
            });
        }
        let mut members = vec![];
        if let Some(at) = &self.at {
            members.push(("at", Ordered::Text(at.clone())));
        }
        if !shapes.is_empty() {
            members.push(("shapes", Ordered::List(shapes)));
        }
        members.push(("lines", Ordered::List(lines)));
        Ok(Ordered::Map(members))
    }

    /// The values of the elements of the diagram, by their places, read from
    /// its body `value`: the texts it holds, after checking that the rest of
    /// it is as the original's drawings make it.
    pub(crate) fn values(&self, value: Value) -> Result<Vec<(usize, Value)>> {
        let name = self.name();
        let changed = || {
            format!(
                "{name}: its shapes and lines follow the original's drawings; edit only their texts"
            )
        };
        ensure!(
            value.is_object(),
            "{name} must be a map of {}",
            if self.check_box.is_some() {
                "its label and state"
            } else {
                "its shapes and lines"
            }
        );
        let mut texts: HashMap<usize, Value> = HashMap::new();
        let mut take = |item: &Value, places: &[usize], owner: &str| -> Result<()> {
            let text = &item["text"];
            match places {
                [] => ensure!(item.get("text").is_none(), "{owner} has no text"),
                [place] => {
                    ensure!(item.get("text").is_some(), "{owner} lost its text");
                    ensure!(
                        scalar(text),
                        "the text of {owner} must be text, a number, a boolean or null"
                    );
                    texts.insert(*place, text.clone());
                }
                places => {
                    let given = text
                        .as_array()
                        .filter(|given| given.len() == places.len())
                        .with_context(|| {
                            format!(
                                "{owner} must keep its {} paragraphs as a list",
                                places.len()
                            )
                        })?;
                    for (place, text) in places.iter().zip(given) {
                        ensure!(
                            scalar(text),
                            "the paragraphs of {owner} must be text, numbers, booleans or null"
                        );
                        texts.insert(*place, text.clone());
                    }
                }
            }
            Ok(())
        };
        if self.check_box.is_some() {
            take(&value, self.shapes.first().map_or(&[], |s| &s.texts), &name)?;
            let expected = self
                .body(&|place| texts.get(&place).context("check box label missing"))?
                .to_json();
            ensure!(
                expected == value,
                "{name}: its state, linked cell and place follow the original's check box; edit only its text"
            );
            return Ok(self
                .members
                .iter()
                .map(|place| (*place, texts.remove(place).unwrap_or(Value::Null)))
                .collect());
        }
        let shapes = value["shapes"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        ensure!(shapes.len() == self.shapes.len(), changed());
        for (shape, item) in self.shapes.iter().zip(shapes) {
            ensure!(item["id"] == shape.key.as_str(), changed());
            take(item, &shape.texts, &format!("shape {}", shape.key))?;
        }
        let lines = value["lines"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        ensure!(lines.len() == self.lines.len(), changed());
        for (index, (line, item)) in self.lines.iter().zip(lines).enumerate() {
            if !line.texts.is_empty() || item.is_object() {
                take(item, &line.texts, &format!("line {} of {name}", index + 1))?;
            }
        }
        let expected = self
            .body(&|place| texts.get(&place).context("diagram text missing"))?
            .to_json();
        // What a line joins is written with the texts of the shapes, which
        // an edit of a text leaves behind until the page is written again.
        let mut value = value;
        if let (Some(given), Some(lines)) =
            (value["lines"].as_array_mut(), expected["lines"].as_array())
        {
            for (given, line) in given.iter_mut().zip(lines) {
                match (given, line) {
                    (given @ Value::String(_), Value::String(_)) => *given = line.clone(),
                    (Value::Object(given), Value::Object(line)) if given.contains_key("line") => {
                        given.insert("line".into(), line["line"].clone());
                    }
                    _ => {}
                }
            }
        }
        ensure!(expected == value, changed());
        Ok(self
            .members
            .iter()
            .map(|place| (*place, texts.remove(place).unwrap_or(Value::Null)))
            .collect())
    }
}

/// The elements `elements` of a content page as the metadata places them: a
/// page writes the elements of each diagram together at the place of the
/// first, and this puts each of them back in its own place.
pub(crate) fn expand(
    elements: Vec<BodyElement>,
    metadata: &[Meta],
    diagrams: &Diagrams,
) -> Result<Vec<BodyElement>> {
    if diagrams.is_empty()
        && elements
            .iter()
            .all(|e| !matches!(e.kind, DIAGRAM | CHECKBOX))
    {
        return Ok(elements);
    }
    let mut written = elements.into_iter();
    let mut pending: HashMap<usize, Value> = HashMap::new();
    let mut out = Vec::with_capacity(metadata.len());
    for (place, e) in metadata.iter().enumerate() {
        let kind = BodyElement::kind_named(string(&e["type"])?).context("unknown element type")?;
        if let Some(value) = pending.remove(&place) {
            out.push(BodyElement { kind, value });
            continue;
        }
        let element = written.next().context("element count changed")?;
        if matches!(element.kind, DIAGRAM | CHECKBOX) {
            let (diagram, first) = diagrams.of(place).with_context(|| {
                format!(
                    "a {} is written where the original's drawings make none",
                    element.kind
                )
            })?;
            ensure!(
                first && diagram.kind() == element.kind,
                "element count changed"
            );
            pending.extend(diagram.values(element.value)?);
            let value = pending.remove(&place).context("diagram element missing")?;
            out.push(BodyElement { kind, value });
        } else {
            if let Some((diagram, _)) = diagrams.of(place) {
                bail!(diagram.written_apart());
            }
            out.push(element);
        }
    }
    ensure!(written.next().is_none(), "element count changed");
    Ok(out)
}
