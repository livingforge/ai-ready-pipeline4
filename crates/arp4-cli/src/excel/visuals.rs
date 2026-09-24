//! Lossless DrawingML facts. Coordinates are zero-based cells / EMU, not pixels.
use super::*;

fn target(parts: &BTreeMap<String, Vec<u8>>, base: &str, id: &str, kind: &str) -> Result<String> {
    let key = relationships_part(base)?;
    let doc = xml(parts.get(&key).context("missing object relationships")?)?;
    let rel = doc
        .descendants()
        .find(|n| n.has_tag_name((PKG_REL, "Relationship")) && n.attribute("Id") == Some(id))
        .context("missing object relationship")?;
    ensure!(
        rel.attribute("TargetMode") != Some("External"),
        "external object relationship is unsupported"
    );
    ensure!(
        rel.attribute("Type").is_some_and(|v| v.ends_with(kind)),
        "unexpected object relationship type"
    );
    let value = rel.attribute("Target").context("missing object target")?;
    ensure!(!value.contains(['\\', ':', '%']), "invalid object target");
    let resolved = relationship_target(base, value)?;
    ensure!(parts.contains_key(&resolved), "missing object part");
    Ok(resolved)
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

pub(super) fn extract_visuals(
    parts: &BTreeMap<String, Vec<u8>>,
    sheet_part: &str,
    sheet: Node<'_, '_>,
) -> Result<Vec<Value>> {
    let mut objects = vec![];
    for drawing in sheet.children().filter(|n| n.has_tag_name((NS, "drawing"))) {
        let part = target(
            parts,
            sheet_part,
            drawing
                .attribute((REL, "id"))
                .context("missing drawing ID")?,
            "/drawing",
        )?;
        let doc = xml(&parts[&part])?;
        for anchor in doc.root_element().children().filter(Node::is_element) {
            for node in anchor.descendants().filter(|n| {
                n.tag_name().namespace() == Some(XDR)
                    && matches!(
                        n.tag_name().name(),
                        "sp" | "pic" | "cxnSp" | "grpSp" | "graphicFrame"
                    )
            }) {
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
                        let media = target(parts, &part, id, "/image")?;
                        let sha = hash(&parts[&media]);
                        image = json!({"part":media,"sha256":sha});
                    }
                    linked_image = json!(blip.attribute((REL, "link")));
                }
                objects.push(json!({"id":format!("{part}#{raw_id}"),"part":part,"kind":kind,
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
    for item in sheet
        .descendants()
        .filter(|n| n.has_tag_name((NS, "tablePart")))
    {
        let part = target(
            parts,
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
