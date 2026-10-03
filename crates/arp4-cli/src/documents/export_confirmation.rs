use super::*;

impl Store {
    fn confirmation_path(&self, id: &str) -> Result<PathBuf> {
        document_id(id)?;
        under(&self.arp, &format!("cache/confirmations/{id}/layout.json"))
    }

    pub fn confirm_export(
        &self,
        id: &str,
        candidate: &Path,
        output_sha256: &str,
        reviewer: &str,
        reason: &str,
        layout_reviewed: bool,
    ) -> Result<Value> {
        ensure!(
            layout_reviewed,
            "layout is unconfirmed; open the exported candidate, verify changed cells and surrounding layout, then use --layout-reviewed with its exact output hash"
        );
        nonempty(reviewer)?;
        nonempty(reason)?;
        let inspected = self.inspect(&self.document(id)?, true)?;
        ensure!(
            inspected.source_current,
            "source changed; re-import before confirming export"
        );
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
            "candidate must be inside .arp/cache/export"
        );
        let report_path = candidate.with_extension(format!(
            "{}.report.json",
            candidate
                .extension()
                .context("missing candidate extension")?
                .to_string_lossy()
        ));
        let report_bytes = fs::read(&report_path)?;
        let report: Value = serde_json::from_slice(&report_bytes)?;
        ensure!(
            report["written"] == true
                && report["complete"] == true
                && report["document_id"] == id
                && report["content"] == inspected.fingerprint
                && report["source_sha256"] == inspected.meta["source"]["sha256"],
            "export report is stale or incomplete"
        );
        ensure!(
            hash(&fs::read(&candidate)?) == output_sha256
                && report["output_sha256"] == output_sha256,
            "export candidate hash differs from the reviewed output"
        );
        let plan = self.export(id, None, "auto")?;
        for field in [
            "changes",
            "formula_changes",
            "operations",
            "unreflected",
            "excluded",
            "omissions",
            "deleted_cells",
        ] {
            ensure!(
                report[field] == plan[field],
                "export report differs from current writeback plan: {field}"
            );
        }
        ensure!(
            report["layout_review_required"] == true,
            "export must be regenerated with layout review metadata"
        );
        if inspected.extraction["parser"]
            .as_str()
            .is_some_and(|parser| parser.contains(";cells/"))
            && array(&plan["operations"])?.is_empty()
        {
            // Scalar writers provide this proof; table-label/structural paths
            // have their own explicit operations and still require visual review.
            let labels = array(&plan["changes"])?.iter().any(|change| {
                let sheet = array(&inspected.extraction["sheets"])
                    .ok()
                    .and_then(|sheets| {
                        sheets.iter().find(|sheet| sheet["name"] == change["sheet"])
                    });
                let cell = change["cell"]
                    .as_str()
                    .and_then(|cell| excel::coordinate(cell).ok());
                sheet.zip(cell).is_some_and(|(sheet, (column, row))| {
                    excel::table_label(sheet, column, row).is_ok_and(|label| label.is_some())
                })
            });
            if !labels {
                ensure!(
                    report["non_edit_preservation_verified"] == true,
                    "export lacks non-edit preservation verification; regenerate it"
                );
            }
        }
        let confirmation = json!({"schema_version":"1","document_id":id,"content":inspected.fingerprint,"source_sha256":inspected.meta["source"]["sha256"],"candidate":relative,"output_sha256":output_sha256,"report_sha256":hash(&report_bytes),"reviewer":reviewer,"reason":reason,"layout_status":"human_confirmed"});
        validate("export-confirmation", &confirmation)?;
        write(&self.confirmation_path(id)?, &confirmation)?;
        Ok(confirmation)
    }

    pub(super) fn confirmed_export(
        &self,
        id: &str,
        inspected: &Inspection,
    ) -> Result<(Vec<u8>, Value)> {
        let confirmation = read(&self.confirmation_path(id)?,Some("export-confirmation")).context("export layout is unconfirmed; export and confirm-export the exact candidate before apply")?;
        ensure!(
            confirmation["document_id"] == id
                && confirmation["content"] == inspected.fingerprint
                && confirmation["source_sha256"] == inspected.meta["source"]["sha256"],
            "export confirmation is stale"
        );
        let candidate = under(&self.root, string(&confirmation["candidate"])?)?;
        ensure!(
            candidate.starts_with(self.arp.join("cache/export")),
            "confirmed candidate must be inside .arp/cache/export"
        );
        let report_path = candidate.with_extension(format!(
            "{}.report.json",
            candidate
                .extension()
                .context("missing candidate extension")?
                .to_string_lossy()
        ));
        let bytes = fs::read(&candidate)?;
        let report_bytes = fs::read(report_path)?;
        ensure!(
            hash(&bytes) == confirmation["output_sha256"]
                && hash(&report_bytes) == confirmation["report_sha256"],
            "confirmed export candidate or report changed"
        );
        let source = under(&self.root, string(&inspected.meta["source"]["path"])?)?;
        ensure!(
            candidate.extension() == source.extension(),
            "candidate format differs from source"
        );
        Ok((bytes, serde_json::from_slice(&report_bytes)?))
    }
}
