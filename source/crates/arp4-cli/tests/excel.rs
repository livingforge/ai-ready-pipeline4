use arp4_cli::excel::Workbook;
use serde_json::json;
use std::io::{Cursor, Read, Write};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

fn fixture(extra: Option<(&str, &str)>, external: bool) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.set_comment("preserve archive comment").unwrap();
    let rel = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml" {}/></Relationships>"#,
        if external {
            "TargetMode=\"External\""
        } else {
            ""
        }
    );
    let mut parts = vec![
        (
            "xl/workbook.xml",
            r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="日本語" sheetId="1" r:id="r1"/></sheets></workbook>"#,
        ),
        ("xl/_rels/workbook.xml.rels", rel.as_str()),
        (
            "xl/worksheets/sheet1.xml",
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>text</t></is></c><c r="B1"><v>10</v></c><c r="C1" t="b"><v>1</v></c><c r="D1"><f>B1*2</f><v>20</v></c><c r="E1"><v>1</v></c><c r="F1"><v>2</v></c></row></sheetData><mergeCells><mergeCell ref="E1:F1"/></mergeCells></worksheet>"#,
        ),
        ("custom/opaque.bin", "opaque payload"),
    ];
    if let Some(part) = extra {
        parts.push(part);
    }
    for (name, content) in parts {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn scalar_writeback_preserves_parts_and_invalidates_formula_cache() {
    for (cell, value) in [
        ("A1", json!("=literal & <text>\r\n日本語")),
        ("B1", json!(1.25)),
        ("B1", json!(null)),
        ("C1", json!(false)),
    ] {
        let book = Workbook::from_bytes(fixture(None, false)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.xlsx");
        book.patch(
            &out,
            &[json!({"sheet":"日本語", "cell":cell, "after":value})],
        )
        .unwrap();
        let mut zip = ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        assert_eq!(zip.comment(), b"preserve archive comment");
        let mut opaque = String::new();
        zip.by_name("custom/opaque.bin")
            .unwrap()
            .read_to_string(&mut opaque)
            .unwrap();
        assert_eq!(opaque, "opaque payload");
        let mut sheet = String::new();
        zip.by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet)
            .unwrap();
        let xml = roxmltree::Document::parse(&sheet).unwrap();
        let formula = xml
            .descendants()
            .find(|n| n.attribute("r") == Some("D1"))
            .unwrap();
        assert_eq!(
            formula
                .children()
                .find(|n| n.tag_name().name() == "f")
                .unwrap()
                .text(),
            Some("B1*2")
        );
        assert!(!formula.children().any(|n| n.tag_name().name() == "v"));
        let reread = Workbook::open(&out).unwrap();
        let actual = reread.sheets[0]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["address"] == cell);
        assert_eq!(
            actual
                .map(|c| &c["value"])
                .unwrap_or(&serde_json::Value::Null),
            &value
        );
        assert!(
            book.patch(&out, &[]).is_err(),
            "existing output must survive"
        );
    }
}

#[test]
fn invalid_writeback_fails_before_creating_output() {
    let book = Workbook::from_bytes(fixture(None, false)).unwrap();
    for (cell, value) in [
        ("D1", json!(3)),
        ("B1", json!("wrong type")),
        ("F1", json!(3)),
        ("A2", json!(3)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("result.xlsx");
        assert!(
            book.patch(
                &out,
                &[json!({"sheet":"日本語", "cell":cell, "after":value})]
            )
            .is_err()
        );
        assert!(!out.exists());
    }
}

#[test]
fn external_relationships_and_signed_writeback_are_rejected() {
    assert!(Workbook::from_bytes(fixture(None, true)).is_err());
    let book = Workbook::from_bytes(fixture(
        Some(("_xmlsignatures/sig1.xml", "signature")),
        false,
    ))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("result.xlsx");
    assert!(
        book.patch(&out, &[json!({"sheet":"日本語", "cell":"B1", "after":12})])
            .is_err()
    );
    assert!(!out.exists());
}
