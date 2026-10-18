//! Rendering a page of an exported candidate to a PNG image, so that its
//! layout is looked at before `confirm-export`: an Excel sheet or range by
//! Excel, a Word or PowerPoint document as Office saves it as PDF, and a PDF
//! as Windows renders it.
use super::*;

/// The part of a candidate to render: an Excel `sheet` and `range`, or the
/// `page` of another document, from 1.
pub struct RenderPlace<'a> {
    pub sheet: Option<&'a str>,
    pub range: Option<&'a str>,
    pub page: u32,
}

impl Store {
    /// Renders the place of `candidate`, a file export wrote for the document
    /// `id` with its report beside it, to the new PNG file `out`.
    pub fn render_candidate(
        &self,
        id: &str,
        candidate: &Path,
        out: &Path,
        place: RenderPlace<'_>,
        timeout: u64,
    ) -> Result<Value> {
        self.document(id)?;
        let candidate = if candidate.is_absolute() {
            candidate.to_owned()
        } else {
            std::env::current_dir()?.join(candidate)
        };
        let relative = candidate
            .strip_prefix(&self.root)
            .context("candidate must be inside project .arp/cache/export")?
            .to_string_lossy()
            .replace('\\', "/");
        let candidate = under(&self.root, &relative)?;
        ensure!(
            candidate.starts_with(self.arp.join("cache/export")),
            "candidate must be an export inside .arp/cache/export"
        );
        let extension = candidate
            .extension()
            .context("missing candidate extension")?
            .to_string_lossy()
            .to_lowercase();
        let report_path = candidate.with_extension(format!(
            "{}.report.json",
            candidate.extension().unwrap().to_string_lossy()
        ));
        let report: Value = serde_json::from_slice(
            &fs::read(&report_path).context("the candidate has no export report beside it")?,
        )?;
        let bytes = fs::read(&candidate)?;
        let output_sha256 = hash(&bytes);
        ensure!(
            report["document_id"] == id && report["output_sha256"] == output_sha256.as_str(),
            "the candidate is not the file its export report describes for {id}"
        );
        ensure!(
            !out.exists(),
            "render output already exists: {}",
            out.display()
        );
        ensure!(
            out.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("png")),
            "render output must be a .png file"
        );
        ensure!(
            (1..=600).contains(&timeout),
            "timeout must be 1..600 seconds"
        );
        let mut result =
            json!({"document_id":id,"candidate":relative,"output_sha256":output_sha256});
        let png = if crate::document_source::EXCEL_FORMATS.contains(&extension.as_str()) {
            let sheet = place
                .sheet
                .context("--sheet names the worksheet to render")?;
            // An export asks Excel to calculate the formulas whose results it
            // left out, as Excel does opening the workbook.
            let recalculate = report["requires_excel_recalculation"] == true;
            let (png, rendered) =
                excel::render(&candidate, sheet, place.range, timeout, recalculate)?;
            result["recalculated"] = json!(recalculate);
            result["sheet"] = json!(sheet);
            result["range"] = rendered["range"].clone();
            png
        } else {
            ensure!(
                place.sheet.is_none() && place.range.is_none(),
                "--sheet and --range render Excel workbooks; give --page for other documents"
            );
            let pdf = if extension == "pdf" {
                bytes
            } else {
                let stage = Stage::new_in(&under(&self.arp, "work")?, &self.arp)?;
                crate::office_pdf::convert(&candidate, stage.path(), timeout)?
            };
            let rendered = crate::pdf_render::render(&pdf, place.page, timeout)?;
            result["page"] = json!(place.page);
            result["pages"] = json!(rendered.pages);
            rendered.png
        };
        ensure!(
            png.len() >= 24 && png.starts_with(b"\x89PNG\r\n\x1a\n") && &png[12..16] == b"IHDR",
            "the renderer gave no PNG image"
        );
        result["width"] = json!(u32::from_be_bytes(png[16..20].try_into()?));
        result["height"] = json!(u32::from_be_bytes(png[20..24].try_into()?));
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        immutable(out, &png)?;
        result["image"] = json!(out);
        result["state"] = json!("rendered");
        Ok(result)
    }
}
