//! The results of formulas, calculated by desktop Excel and written into a
//! candidate whose writeback left them out or stale. Excel calculates a copy;
//! only the results (`<v>` and the cell's type `t`) of formula cells come
//! from it, and every other byte stays as ARP wrote it.
use super::*;

/// How long Excel may take to open, calculate and save a copy.
const TIMEOUT_SECONDS: u64 = 300;

/// The result of each formula cell of a workbook by sheet name and address:
/// the cell's type (`t`, none for a number) and its cached value.
type Results = BTreeMap<(String, String), (Option<String>, String)>;

/// Calculates the formulas of `candidate`, which the writeback of the
/// workbook `source` wrote, with desktop Excel and writes each result that
/// differs from the one the cell holds. When Excel gave every result, the
/// full calculation on opening that the writeback asked for is withdrawn,
/// since the workbook then holds what Excel calculates.
pub fn calculate(candidate: &Path, source: &Path) -> Result<Value> {
    ensure!(
        cfg!(windows),
        "calculating formulas requires Windows and desktop Microsoft Excel"
    );
    super::render::validate_source(candidate)?;
    let stage = tempfile::tempdir()?;
    let extension = candidate
        .extension()
        .context("candidate without an extension")?;
    let calculated = stage.path().join("calculated").with_extension(extension);
    let request_path = stage.path().join("request.json");
    let result_path = stage.path().join("result.json");
    fs::write(
        &request_path,
        encoded(&json!({"source":candidate,"output":calculated,"result":result_path})),
    )?;
    let log = stage.path().join("stderr.txt");
    let Some(status) = crate::office_worker::run(
        "--internal-excel-calculate",
        &request_path,
        &log,
        TIMEOUT_SECONDS,
    )?
    else {
        bail!("Excel calculation timed out; no results were written");
    };
    ensure!(
        status.success(),
        "Excel calculation failed: {}",
        String::from_utf8_lossy(&fs::read(log)?)
    );
    let result = read(&result_path, None)?;
    ensure!(
        result["engine"] == "excel-com",
        "unexpected calculation result"
    );
    let book = Workbook::open(candidate)?;
    let (mut patched, written, missing) =
        fill_results(&book, &formula_results(&Workbook::open(&calculated)?)?)?;
    let original = Workbook::open(source)?;
    if missing == 0 {
        let workbook = withdraw_full_calculation(
            std::str::from_utf8(&book.parts["xl/workbook.xml"])?,
            std::str::from_utf8(&original.parts["xl/workbook.xml"])?,
        )?;
        patched.insert("xl/workbook.xml".into(), workbook.into_bytes());
    }
    let filled = candidate.with_extension(format!("calculated.{}", extension.to_string_lossy()));
    write_archive_without(&book.raw, &filled, &patched, &BTreeSet::new())?;
    fs::rename(&filled, candidate)?;
    // A part the results leave as the original had it, such as a workbook
    // part whose calculation setting is restored, is no longer changed.
    let written_book = Workbook::open(candidate)?;
    let changed: Vec<_> = written_book
        .parts
        .iter()
        .filter(|(part, bytes)| original.parts.get(*part) != Some(*bytes))
        .map(|(part, _)| part)
        .collect();
    Ok(
        json!({"state":"calculated","engine":"excel-com","version":result["version"],"results":written,"missing":missing,"changed_parts":changed}),
    )
}

/// The result Excel saved for each formula cell of `book`.
fn formula_results(book: &Workbook) -> Result<Results> {
    let mut results = BTreeMap::new();
    for sheet in &book.sheets {
        let name = string(&sheet["name"])?;
        let doc = xml(&book.parts[string(&sheet["part"])?])?;
        for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
            if child(cell, "f").is_none() {
                continue;
            }
            let (Some(address), Some(value)) = (cell.attribute("r"), child(cell, "v")) else {
                continue;
            };
            results.insert(
                (name.to_owned(), address.to_owned()),
                (
                    cell.attribute("t").map(str::to_owned),
                    value.text().unwrap_or_default().to_owned(),
                ),
            );
        }
    }
    Ok(results)
}

/// The worksheet parts of `book` with `results` written into the formula
/// cells holding another result or none, with how many were written and how
/// many formula cells `results` lacked.
fn fill_results(
    book: &Workbook,
    results: &Results,
) -> Result<(BTreeMap<String, Vec<u8>>, usize, usize)> {
    let mut patched = BTreeMap::new();
    let (mut written, mut missing) = (0, 0);
    for sheet in &book.sheets {
        let name = string(&sheet["name"])?;
        let part = string(&sheet["part"])?;
        let text = std::str::from_utf8(&book.parts[part])?;
        let doc = xml(text.as_bytes())?;
        let mut edits = vec![];
        for cell in doc.descendants().filter(|n| n.has_tag_name((NS, "c"))) {
            let Some(formula) = child(cell, "f") else {
                continue;
            };
            let address = cell.attribute("r").context("cell without an address")?;
            let Some((kind, value)) = results.get(&(name.to_owned(), address.to_owned())) else {
                missing += 1;
                continue;
            };
            let held = child(cell, "v");
            if cell.attribute("t") == kind.as_deref()
                && held.is_some_and(|held| held.text().unwrap_or_default() == value)
            {
                continue;
            }
            let opening = opening_range(text, cell)?;
            let prefix = element_tag(&text[opening.clone()])?
                .strip_suffix('c')
                .context("invalid cell tag")?
                .to_owned();
            let mut replacement = remove_xml_attribute(&text[opening.clone()], "t")?;
            if let Some(kind) = kind {
                replacement = set_xml_attribute(&replacement, "t", kind)?;
            }
            edits.push((opening, replacement));
            let result = format!("<{prefix}v>{}</{prefix}v>", xml_attr(value));
            let end = formula.range().end;
            edits.push((held.map_or(end..end, |held| held.range()), result));
            written += 1;
        }
        if !edits.is_empty() {
            patched.insert(part.to_owned(), splice(text, edits, "result")?.into_bytes());
        }
    }
    Ok((patched, written, missing))
}

/// The `workbook` part with its full calculation on opening as the
/// `original` part has it: the setting restored, or the calcPr the
/// writeback added removed.
fn withdraw_full_calculation(workbook: &str, original: &str) -> Result<String> {
    let original = xml(original.as_bytes())?;
    let before = child(original.root_element(), "calcPr");
    let doc = xml(workbook.as_bytes())?;
    let calculation = child(doc.root_element(), "calcPr").context("workbook without calcPr")?;
    let range = calculation.range();
    let replacement = match before {
        Some(before) => {
            let (opening, rest) = xml_opening(&workbook[range.clone()])?;
            let opening = match before.attribute("fullCalcOnLoad") {
                Some(value) => set_xml_attribute(opening, "fullCalcOnLoad", value)?,
                None => remove_xml_attribute(opening, "fullCalcOnLoad")?,
            };
            format!("{opening}{rest}")
        }
        None => String::new(),
    };
    splice(workbook, vec![(range, replacement)], "calculation")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workbook(sheet: &str) -> Workbook {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in [
            (
                "xl/workbook.xml",
                format!(
                    r#"<workbook xmlns="{NS}" xmlns:r="{REL}"><sheets><sheet name="S" sheetId="1" r:id="r1"/></sheets><calcPr fullCalcOnLoad="1"/></workbook>"#
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PKG_REL}"><Relationship Id="r1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
                ),
            ),
            ("xl/worksheets/sheet1.xml", sheet.to_owned()),
            (
                "[Content_Types].xml",
                r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/></Types>"#.to_owned(),
            ),
        ] {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        Workbook::from_bytes(zip.finish().unwrap().into_inner()).unwrap()
    }

    /// Results go into the formula cells that hold none, an empty one or a
    /// stale one; a cell already holding Excel's result stays as it is, and
    /// a formula Excel gave no result keeps none.
    #[test]
    fn results_replace_what_formula_cells_hold() {
        let written = workbook(&format!(
            r#"<x:worksheet xmlns:x="{NS}"><x:sheetData><x:row r="1"><x:c r="A1"><x:v>2</x:v></x:c><x:c r="B1" t="str"><x:f>A1*2</x:f></x:c><x:c r="C1"><x:f>A1&amp;"x"</x:f><x:v /></x:c><x:c r="D1"><x:f t="shared" ref="D1" si="0">A1&gt;1</x:f></x:c><x:c r="E1"><x:f>A1</x:f><x:v>9</x:v></x:c><x:c r="F1"><x:f>NA()</x:f></x:c><x:c r="G1"><x:f>A1</x:f><x:v>2</x:v></x:c></x:row></x:sheetData></x:worksheet>"#
        ));
        let calculated = workbook(&format!(
            r#"<worksheet xmlns="{NS}"><sheetData><row r="1"><c r="A1"><v>2</v></c><c r="B1"><f>A1*2</f><v>4</v></c><c r="C1" t="str"><f>A1&amp;"x"</f><v>2&lt;x</v></c><c r="D1" t="b"><f t="shared" ref="D1" si="0">A1&gt;1</f><v>1</v></c><c r="E1"><f>A1</f><v>2</v></c><c r="G1"><f>A1</f><v>2</v></c></row></sheetData></worksheet>"#
        ));
        let (patched, written_count, missing) =
            fill_results(&written, &formula_results(&calculated).unwrap()).unwrap();
        assert_eq!((written_count, missing), (4, 1));
        assert_eq!(
            std::str::from_utf8(&patched["xl/worksheets/sheet1.xml"]).unwrap(),
            format!(
                r#"<x:worksheet xmlns:x="{NS}"><x:sheetData><x:row r="1"><x:c r="A1"><x:v>2</x:v></x:c><x:c r="B1"><x:f>A1*2</x:f><x:v>4</x:v></x:c><x:c r="C1" t="str"><x:f>A1&amp;"x"</x:f><x:v>2&lt;x</x:v></x:c><x:c r="D1" t="b"><x:f t="shared" ref="D1" si="0">A1&gt;1</x:f><x:v>1</x:v></x:c><x:c r="E1"><x:f>A1</x:f><x:v>2</x:v></x:c><x:c r="F1"><x:f>NA()</x:f></x:c><x:c r="G1"><x:f>A1</x:f><x:v>2</x:v></x:c></x:row></x:sheetData></x:worksheet>"#
            )
        );
    }

    #[test]
    fn the_full_calculation_on_opening_returns_to_the_originals() {
        let original = |calculation: &str| {
            format!(r#"<workbook xmlns="{NS}"><sheets/>{calculation}</workbook>"#)
        };
        let written = original(r#"<calcPr calcId="191029" fullCalcOnLoad="1"/>"#);
        for (before, after) in [
            ("", ""),
            (
                r#"<calcPr calcId="191029"/>"#,
                r#"<calcPr calcId="191029"/>"#,
            ),
            (
                r#"<calcPr calcId="191029" fullCalcOnLoad="0"/>"#,
                r#"<calcPr calcId="191029" fullCalcOnLoad="0"/>"#,
            ),
        ] {
            assert_eq!(
                withdraw_full_calculation(&written, &original(before)).unwrap(),
                original(after)
            );
        }
    }
}
