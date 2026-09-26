//! Lossless DrawingML facts. Coordinates are zero-based cells / EMU, not pixels.
use super::*;

const THREADED: &str = "http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments";

/// A relationship as written: its type, target and whether it is external.
struct Relationship {
    kind: String,
    target: Option<String>,
    external: bool,
}

/// Relationships of parts by ID, each `.rels` part parsed once: a sheet with a
/// picture or form control per row would otherwise re-parse it for each one.
struct Relationships<'a> {
    parts: &'a BTreeMap<String, Vec<u8>>,
    parsed: BTreeMap<String, BTreeMap<String, Relationship>>,
}

impl<'a> Relationships<'a> {
    fn new(parts: &'a BTreeMap<String, Vec<u8>>) -> Self {
        Self {
            parts,
            parsed: BTreeMap::new(),
        }
    }

    /// Resolves a relationship of `base` to its (type, part).
    fn related(&mut self, base: &str, id: &str) -> Result<(String, String)> {
        let key = relationships_part(base)?;
        if !self.parsed.contains_key(&key) {
            let doc = xml(self
                .parts
                .get(&key)
                .context("missing object relationships")?)?;
            let mut by_id = BTreeMap::new();
            for rel in doc
                .descendants()
                .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
            {
                if let Some(id) = rel.attribute("Id") {
                    by_id.entry(id.to_owned()).or_insert_with(|| Relationship {
                        kind: rel.attribute("Type").unwrap_or("").to_owned(),
                        target: rel.attribute("Target").map(str::to_owned),
                        external: rel.attribute("TargetMode") == Some("External"),
                    });
                }
            }
            self.parsed.insert(key.clone(), by_id);
        }
        let rel = self.parsed[&key]
            .get(id)
            .context("missing object relationship")?;
        ensure!(!rel.external, "external object relationship is unsupported");
        let value = rel.target.as_deref().context("missing object target")?;
        ensure!(!value.contains(['\\', ':', '%']), "invalid object target");
        let resolved = relationship_target(base, value)?;
        ensure!(self.parts.contains_key(&resolved), "missing object part");
        Ok((rel.kind.clone(), resolved))
    }

    fn target(&mut self, base: &str, id: &str, kind: &str) -> Result<String> {
        let (relationship, part) = self.related(base, id)?;
        ensure!(
            relationship.ends_with(kind),
            "unexpected object relationship type"
        );
        Ok(part)
    }
}

/// Controls on a sheet by shape ID, as (settings, assigned macro). Excel keeps
/// these outside DrawingML: the macro on `controlPr`, and the object type,
/// linked cell and list range of a form control in its `ctrlProp` part.
/// ActiveX settings live in binary parts and are not read.
fn controls(
    parts: &BTreeMap<String, Vec<u8>>,
    relationships: &mut Relationships<'_>,
    sheet_part: &str,
    sheet: Node<'_, '_>,
) -> Result<BTreeMap<String, (Value, Option<String>)>> {
    let mut output = BTreeMap::new();
    for control in sheet
        .descendants()
        .filter(|n| n.has_tag_name((NS, "control")))
    {
        let Some(shape) = control.attribute("shapeId") else {
            continue;
        };
        let property = child(control, "controlPr");
        // The mc:Fallback copy repeats the control without its properties.
        if property.is_none() && output.contains_key(shape) {
            continue;
        }
        let (relationship, part) = relationships.related(
            sheet_part,
            control
                .attribute((REL, "id"))
                .context("missing control relationship")?,
        )?;
        let settings = if relationship.ends_with("/ctrlProp") {
            let doc = xml(&parts[&part])?;
            let root = doc.root_element();
            json!({"kind":root.attribute("objectType").unwrap_or(""),
                "linked_cell":root.attribute("fmlaLink"),"list_range":root.attribute("fmlaRange")})
        } else {
            json!({"kind":"ActiveX","linked_cell":null,"list_range":null})
        };
        let assigned = property
            .and_then(|p| p.attribute("macro"))
            .filter(|m| !m.is_empty())
            .map(str::to_owned);
        output.insert(shape.to_owned(), (settings, assigned));
    }
    Ok(output)
}

/// Display names of threaded comment authors by person ID.
pub(super) fn persons(parts: &BTreeMap<String, Vec<u8>>) -> Result<BTreeMap<String, String>> {
    let mut output = BTreeMap::new();
    let Some(rels) = parts.get("xl/_rels/workbook.xml.rels") else {
        return Ok(output);
    };
    let doc = xml(rels)?;
    let mut relationships = Relationships::new(parts);
    for rel in doc.descendants().filter(|n| {
        n.has_tag_name((PKG_REL, "Relationship"))
            && n.attribute("Type").is_some_and(|t| t.ends_with("/person"))
    }) {
        let part = relationships.target(
            "xl/workbook.xml",
            rel.attribute("Id").context("missing relationship ID")?,
            "/person",
        )?;
        let people = xml(&parts[&part])?;
        for person in people
            .descendants()
            .filter(|n| n.has_tag_name((THREADED, "person")))
        {
            if let (Some(id), Some(name)) =
                (person.attribute("id"), person.attribute("displayName"))
            {
                output.insert(id.to_owned(), name.to_owned());
            }
        }
    }
    Ok(output)
}

/// Notes and threaded comments on a sheet. Excel also saves each threaded
/// comment as a placeholder note for older versions; those notes are left out.
pub(super) fn extract_comments(
    parts: &BTreeMap<String, Vec<u8>>,
    persons: &BTreeMap<String, String>,
    sheet_part: &str,
) -> Result<Vec<Value>> {
    let Some(rels) = parts.get(&relationships_part(sheet_part)?) else {
        return Ok(vec![]);
    };
    let rels = xml(rels)?;
    let mut relationships = Relationships::new(parts);
    let mut notes = vec![];
    let mut threads = vec![];
    for rel in rels
        .descendants()
        .filter(|n| n.has_tag_name((PKG_REL, "Relationship")))
    {
        let kind = rel.attribute("Type").unwrap_or("");
        let threaded = kind.ends_with("/threadedComment");
        if kind != format!("{REL}/comments") && !threaded {
            continue;
        }
        let part = relationships.target(
            sheet_part,
            rel.attribute("Id").context("missing relationship ID")?,
            if threaded {
                "/threadedComment"
            } else {
                "/comments"
            },
        )?;
        let doc = xml(&parts[&part])?;
        if threaded {
            for comment in doc
                .descendants()
                .filter(|n| n.has_tag_name((THREADED, "threadedComment")))
            {
                threads.push(json!({
                    "address":comment.attribute("ref").context("missing comment reference")?,
                    "kind":if comment.attribute("parentId").is_some() {"reply"} else {"thread"},
                    "author":comment.attribute("personId").and_then(|p| persons.get(p)),
                    "text":comment.children().find(|n| n.has_tag_name((THREADED, "text"))).and_then(|n| n.text()).unwrap_or("")}));
            }
        } else {
            let authors: Vec<_> = doc
                .descendants()
                .filter(|n| n.has_tag_name((NS, "author")))
                .map(|n| n.text().unwrap_or(""))
                .collect();
            for comment in doc
                .descendants()
                .filter(|n| n.has_tag_name((NS, "comment")))
            {
                notes.push(json!({
                    "address":comment.attribute("ref").context("missing comment reference")?,
                    "kind":"note",
                    "author":comment.attribute("authorId").and_then(|i| i.parse::<usize>().ok()).and_then(|i| authors.get(i)),
                    "text":child(comment, "text").map(texts).unwrap_or_default()}));
            }
        }
    }
    let threaded: BTreeSet<_> = threads
        .iter()
        .filter_map(|c| c["address"].as_str().map(str::to_owned))
        .collect();
    notes.retain(|n| !n["address"].as_str().is_some_and(|a| threaded.contains(a)));
    notes.extend(threads);
    Ok(notes)
}

fn attributes(node: Node<'_, '_>) -> Value {
    Value::Object(
        node.attributes()
            .map(|a| (a.name().to_owned(), json!(a.value())))
            .collect(),
    )
}

fn placement(node: Node<'_, '_>) -> Value {
    let mut result = json!({"kind":node.tag_name().name()});
    for c in node.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "from" | "to" => {
                let mut point = json!({});
                for v in c.children().filter(Node::is_element) {
                    point[v.tag_name().name()] =
                        json!(v.text().and_then(|v| v.parse::<i64>().ok()));
                }
                result[c.tag_name().name()] = point;
            }
            "pos" | "ext" => result[c.tag_name().name()] = attributes(c),
            _ => {}
        }
    }
    result
}

/// Resolves `mc:AlternateContent` to the anchors of one branch so that the same
/// object is not extracted twice. Excel wraps form controls this way with the
/// object in the first `mc:Choice` and an empty or VML `mc:Fallback`.
fn alternate_content_branch<'a, 'input>(node: Node<'a, 'input>) -> Vec<Node<'a, 'input>> {
    if !node.has_tag_name((MARKUP_COMPATIBILITY, "AlternateContent")) {
        return vec![node];
    }
    let anchors = |branch: Node<'a, 'input>| -> Vec<Node<'a, 'input>> {
        branch
            .children()
            .filter(|n| n.is_element() && n.tag_name().namespace() == Some(XDR))
            .collect()
    };
    node.children()
        .filter(|n| n.has_tag_name((MARKUP_COMPATIBILITY, "Choice")))
        .chain(
            node.children()
                .filter(|n| n.has_tag_name((MARKUP_COMPATIBILITY, "Fallback"))),
        )
        .map(anchors)
        .find(|anchors| !anchors.is_empty())
        .unwrap_or_default()
}

fn drawing_object(node: Node<'_, '_>) -> bool {
    node.tag_name().namespace() == Some(XDR)
        && matches!(
            node.tag_name().name(),
            "sp" | "pic" | "cxnSp" | "grpSp" | "graphicFrame"
        )
}

/// Whether `node` sits in an `mc:AlternateContent` branch within `anchor` that
/// readers do not show. Excel saves a slicer, timeline or newer chart type as a
/// graphic frame in `mc:Choice` and a shape explaining the object in
/// `mc:Fallback`; readers show the first branch holding a drawing object.
fn in_unshown_branch(node: Node<'_, '_>, anchor: Node<'_, '_>) -> bool {
    node.ancestors()
        .take_while(|ancestor| *ancestor != anchor)
        .any(|branch| {
            let Some(alternate) = branch
                .parent()
                .filter(|p| p.has_tag_name((MARKUP_COMPATIBILITY, "AlternateContent")))
            else {
                return false;
            };
            let shown = alternate.children().find(|b| {
                (b.has_tag_name((MARKUP_COMPATIBILITY, "Choice"))
                    || b.has_tag_name((MARKUP_COMPATIBILITY, "Fallback")))
                    && b.descendants().any(drawing_object)
            });
            shown != Some(branch)
        })
}

pub(super) fn extract_visuals(
    parts: &BTreeMap<String, Vec<u8>>,
    sheet_part: &str,
    sheet: Node<'_, '_>,
) -> Result<Vec<Value>> {
    let mut objects = vec![];
    let mut relationships = Relationships::new(parts);
    // Pictures often share one image (a logo on every page).
    let mut hashes = BTreeMap::new();
    let controls = controls(parts, &mut relationships, sheet_part, sheet)?;
    for drawing in sheet.children().filter(|n| n.has_tag_name((NS, "drawing"))) {
        let part = relationships.target(
            sheet_part,
            drawing
                .attribute((REL, "id"))
                .context("missing drawing ID")?,
            "/drawing",
        )?;
        let doc = xml(&parts[&part])?;
        for anchor in doc
            .root_element()
            .children()
            .filter(Node::is_element)
            .flat_map(alternate_content_branch)
        {
            for node in anchor
                .descendants()
                .filter(|n| drawing_object(*n) && !in_unshown_branch(*n, anchor))
            {
                let property = node
                    .children()
                    .find(|n| n.tag_name().name().starts_with("nv"))
                    .and_then(|n| n.children().find(|c| c.has_tag_name((XDR, "cNvPr"))));
                let raw_id = property.and_then(|n| n.attribute("id")).unwrap_or("0");
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
                    .find(|n| n.has_tag_name((XDR, "grpSp")))
                    .and_then(|n| n.children().find(|n| n.has_tag_name((XDR, "nvGrpSpPr"))))
                    .and_then(|n| n.children().find(|n| n.has_tag_name((XDR, "cNvPr"))))
                    .and_then(|n| n.attribute("id"));
                let shape = node
                    .children()
                    .find(|n| matches!(n.tag_name().name(), "spPr" | "grpSpPr"));
                let transform = shape
                    .and_then(|n| n.children().find(|n| n.has_tag_name((DRAWING, "xfrm"))))
                    .or_else(|| node.children().find(|n| n.has_tag_name((XDR, "xfrm"))));
                let transform = transform.map(|n| {
                    let mut v = attributes(n);
                    for child in n.children().filter(Node::is_element) {
                        v[child.tag_name().name()] = attributes(child);
                    }
                    v
                });
                let text = node
                    .children()
                    .find(|n| n.has_tag_name((XDR, "txBody")))
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
                        let media = relationships.target(&part, id, "/image")?;
                        let sha = hashes
                            .entry(media.clone())
                            .or_insert_with(|| hash(&parts[&media]))
                            .clone();
                        image = json!({"part":media,"sha256":sha});
                    }
                    linked_image = json!(blip.attribute((REL, "link")));
                }
                let control = controls.get(raw_id);
                let assigned = node
                    .attribute("macro")
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned)
                    .or_else(|| control.and_then(|(_, m)| m.clone()));
                objects.push(json!({"id":format!("{part}#{raw_id}"),"part":part,"kind":kind,
                    "macro":assigned,"control":control.map(|(settings, _)| settings),
                    "name":property.and_then(|n|n.attribute("name")).unwrap_or(""),"description":property.and_then(|n|n.attribute("descr")).unwrap_or(""),
                    "text":text,"anchor":placement(anchor),"group":group.map(|v|format!("{part}#{v}")),"transform":transform,
                    "geometry":shape.and_then(|n|n.children().find(|n|n.has_tag_name((DRAWING,"prstGeom")))).and_then(|n|n.attribute("prst")),
                    "connections":connections,"image":image,"linked_image":linked_image}));
            }
        }
    }
    Ok(objects)
}

pub(super) fn extract_tables(
    parts: &BTreeMap<String, Vec<u8>>,
    sheet_part: &str,
    sheet: Node<'_, '_>,
) -> Result<Vec<Value>> {
    let mut tables = vec![];
    let mut relationships = Relationships::new(parts);
    for item in sheet
        .descendants()
        .filter(|n| n.has_tag_name((NS, "tablePart")))
    {
        let part = relationships.target(
            sheet_part,
            item.attribute((REL, "id")).context("missing table ID")?,
            "/table",
        )?;
        let doc = xml(&parts[&part])?;
        let table = doc.root_element();
        tables.push(json!({"name":table.attribute("displayName").unwrap_or(""),"range":table.attribute("ref").context("missing table range")?,
            "header_rows":table.attribute("headerRowCount").unwrap_or("1").parse::<u32>()?,
            "totals_rows":table.attribute("totalsRowCount").unwrap_or("0").parse::<u32>()?}));
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parts(body: &str) -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="d" Type="{REL}/drawing" Target="../drawings/drawing1.xml"/></Relationships>"#).into_bytes()),
            ("xl/drawings/drawing1.xml".into(),format!(r#"<xdr:wsDr xmlns:xdr="{XDR}" xmlns:a="{DRAWING}" xmlns:r="{REL}">{body}</xdr:wsDr>"#).into_bytes())
        ])
    }
    fn extract(parts: &BTreeMap<String, Vec<u8>>) -> Result<Vec<Value>> {
        let sheet =
            format!(r#"<worksheet xmlns="{NS}" xmlns:r="{REL}"><drawing r:id="d"/></worksheet>"#);
        let doc = xml(sheet.as_bytes())?;
        extract_visuals(parts, "xl/worksheets/sheet1.xml", doc.root_element())
    }
    #[test]
    fn groups_preserve_local_transforms_paragraphs_and_explicit_connections() {
        let p = parts(
            r#"<xdr:oneCellAnchor><xdr:from><xdr:col>2</xdr:col><xdr:row>3</xdr:row></xdr:from><xdr:ext cx="900" cy="600"/>
            <xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id="1" name="group"/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x="50" y="60"/><a:chOff x="0" y="0"/></a:xfrm></xdr:grpSpPr>
            <xdr:sp><xdr:nvSpPr><xdr:cNvPr id="2" name="step" descr="description"/></xdr:nvSpPr><xdr:spPr><a:xfrm rot="5400000"><a:off x="5" y="6"/></a:xfrm><a:prstGeom prst="diamond"/></xdr:spPr><xdr:txBody><a:p><a:r><a:t>first</a:t></a:r><a:br/><a:r><a:t>line</a:t></a:r></a:p><a:p><a:r><a:t>next</a:t></a:r></a:p></xdr:txBody></xdr:sp>
            <xdr:cxnSp><xdr:nvCxnSpPr><xdr:cNvPr id="3" name="connection"/><xdr:cNvCxnSpPr><a:stCxn id="2" idx="1"/><a:endCxn id="2" idx="3"/></xdr:cNvCxnSpPr></xdr:nvCxnSpPr></xdr:cxnSp></xdr:grpSp></xdr:oneCellAnchor>"#,
        );
        let v = extract(&p).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[1]["text"], "first\nline\nnext");
        assert_eq!(v[1]["group"], v[0]["id"]);
        assert_eq!(v[1]["transform"]["rot"], "5400000");
        assert_eq!(v[1]["geometry"], "diamond");
        assert_eq!(v[1]["anchor"]["from"]["row"], 3);
        assert_eq!(v[2]["connections"][0]["target"], v[1]["id"]);
        assert_eq!(v[2]["connections"][0]["basis"], "explicit");
    }
    #[test]
    fn alternate_content_uses_one_branch_and_falls_back_when_choice_is_empty() {
        let shape = |name: &str| {
            format!(
                r#"<xdr:absoluteAnchor><xdr:pos x="1" y="2"/><xdr:ext cx="3" cy="4"/><xdr:sp><xdr:nvSpPr><xdr:cNvPr id="1" name="{name}"/></xdr:nvSpPr></xdr:sp></xdr:absoluteAnchor>"#
            )
        };
        let wrapped = |choice: &str, fallback: &str| {
            parts(&format!(
                r#"<mc:AlternateContent xmlns:mc="{MARKUP_COMPATIBILITY}"><mc:Choice Requires="a14">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>"#
            ))
        };
        let both = extract(&wrapped(&shape("choice"), &shape("fallback"))).unwrap();
        assert_eq!(both.len(), 1);
        assert_eq!(both[0]["name"], "choice");
        assert_eq!(both[0]["anchor"]["kind"], "absoluteAnchor");
        let fallback = extract(&wrapped("", &shape("fallback"))).unwrap();
        assert_eq!(fallback.len(), 1);
        assert_eq!(fallback[0]["name"], "fallback");
        // Excel puts a slicer's branches inside the anchor; the fallback shape
        // only explains the slicer to older readers.
        let slicer = extract(&parts(&format!(
            r#"<xdr:twoCellAnchor editAs="absolute"><xdr:from><xdr:col>3</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>6</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><mc:AlternateContent xmlns:mc="{MARKUP_COMPATIBILITY}"><mc:Choice Requires="sle15"><xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="2" name="ItemSlicer"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="0" name=""/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:txBody><a:p><a:r><a:t>この図形はテーブル スライサーを表しています。</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent><xdr:clientData/></xdr:twoCellAnchor>"#
        )))
        .unwrap();
        assert_eq!(slicer.len(), 1);
        assert_eq!(slicer[0]["name"], "ItemSlicer");
    }
    #[test]
    fn absolute_linked_picture_is_identified_without_loading_external_content() {
        let p = parts(
            r#"<xdr:absoluteAnchor><xdr:pos x="120" y="240"/><xdr:ext cx="300" cy="400"/><xdr:pic><xdr:nvPicPr><xdr:cNvPr id="1" name="linked"/></xdr:nvPicPr><xdr:blipFill><a:blip r:link="external"/></xdr:blipFill></xdr:pic></xdr:absoluteAnchor>"#,
        );
        let v = extract(&p).unwrap();
        assert_eq!(v[0]["anchor"]["pos"]["x"], "120");
        assert_eq!(v[0]["linked_image"], "external");
        assert!(v[0]["image"].is_null());
    }
    #[test]
    fn external_drawing_and_package_escape_are_rejected() {
        for rel in [
            r#"Target="https://example.invalid/drawing" TargetMode="External""#,
            r#"Target="../../../outside.xml""#,
        ] {
            let mut p = parts("");
            p.insert("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="d" Type="{REL}/drawing" {rel}/></Relationships>"#).into_bytes());
            assert!(extract(&p).is_err());
        }
    }
    #[test]
    fn explicit_table_metadata_uses_package_relationships() {
        let mut p = BTreeMap::new();
        p.insert("xl/worksheets/_rels/sheet1.xml.rels".into(),format!(r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="t" Type="{REL}/table" Target="../tables/table1.xml"/></Relationships>"#).into_bytes());
        p.insert("xl/tables/table1.xml".into(),format!(r#"<table xmlns="{NS}" displayName="Measurements" ref="B2:E8" headerRowCount="1" totalsRowCount="1"/>"#).into_bytes());
        let sheet = format!(
            r#"<worksheet xmlns="{NS}" xmlns:r="{REL}"><tableParts><tablePart r:id="t"/></tableParts></worksheet>"#
        );
        let doc = xml(sheet.as_bytes()).unwrap();
        let tables = extract_tables(&p, "xl/worksheets/sheet1.xml", doc.root_element()).unwrap();
        assert_eq!(
            tables[0],
            json!({"name":"Measurements","range":"B2:E8","header_rows":1,"totals_rows":1})
        );
    }
}
