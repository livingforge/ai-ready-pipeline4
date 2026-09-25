use super::*;

impl Store {
    pub fn import(&self, source: &Path, id: &str) -> Result<Value> {
        identifier(id)?;
        let source = dunce::canonicalize(if source.is_absolute() {
            source.to_owned()
        } else {
            let direct = self.root.join(source);
            if direct.exists() {
                direct
            } else {
                let config = read(
                    &under(&self.root, ".arp/config.yml")?,
                    Some("project-config"),
                )?;
                under(&self.root, string(&config["sources"])?)?.join(source)
            }
        })?;
        let relative = source
            .strip_prefix(&self.root)
            .context("place source inside project")?
            .to_string_lossy()
            .replace('\\', "/");
        under(&self.root, &relative)?;
        let ext = source
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if let Some(replacement) = crate::document_source::binary_office_replacement(&ext) {
            bail!(
                ".{ext} is a binary Office format; save it in Office as {replacement} and import that copy"
            );
        }
        let formats = crate::document_source::input_formats();
        ensure!(
            formats.contains(&ext.as_str()),
            "import supports {}",
            formats
                .iter()
                .map(|f| format!(".{f}"))
                .collect::<Vec<_>>()
                .join("/")
        );
        let current = self.document(id)?;
        let index = self.index()?;
        ensure!(
            !source.starts_with(&self.arp),
            "import an original outside .arp"
        );
        let source_path = relative;
        for (other, path) in index.as_object().context("invalid source index")? {
            ensure!(
                other == id || path != &json!(source_path),
                "source already assigned to another document"
            );
        }
        let book = Source::open(&source)?;
        let sha = hash(book.raw());
        let info = json!({"path":source_path,"sha256":sha,"source_path":source_path});
        let note = book.note();
        let mut assets = BTreeMap::new();
        if let Source::Excel(workbook) = &book {
            for sheet in &workbook.sheets {
                for drawing in array(&sheet["drawings"])? {
                    if let Some(part) = drawing["image"]["part"].as_str() {
                        assets.insert(
                            string(&drawing["image"]["asset"])?.to_owned(),
                            workbook.parts[part].clone(),
                        );
                    }
                }
            }
        }
        let asset_info: Vec<_> = assets
            .iter()
            .map(|(name, bytes)| json!({"path":name,"sha256":hash(bytes),"ocr":crate::ocr::recognize(name, bytes)}))
            .collect();
        let extraction = json!({"schema_version":"1","document_id":id,"source":info,"parser":format!("arp4-rust/{};{};ocr=auto-images",env!("CARGO_PKG_VERSION"),book.parser()),"pages":[],"sheets":book.sheets(),"findings":[{"level":"warning","code":"R001","message":&note}],"assets":asset_info});
        validate("extraction", &extraction)?;
        let extraction_hash = hash(&encoded(&extraction));
        let interpretation = if current.join("mappings.yml").exists() {
            let previous = read(&current.join("mappings.yml"), Some("mappings"))?;
            let journal = previous["interpretation"].clone();
            crate::document_structure::validate_corrections(&journal)?;
            if journal["document"] == id && journal["source_path"] == source_path {
                journal
            } else {
                crate::document_structure::empty_corrections(&extraction)
            }
        } else {
            crate::document_structure::empty_corrections(&extraction)
        };
        let proposal_id = format!("{id}-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let parent = under(&self.arp, &format!("changes/{id}"))?;
        fs::create_dir_all(&parent)?;
        let stage = tempfile::tempdir_in(&parent)?;
        if !assets.is_empty() {
            fs::create_dir(stage.path().join("assets"))?;
            for (name, bytes) in &assets {
                fs::write(stage.path().join("assets").join(name), bytes)?;
            }
        }
        let base = self.fingerprint(&current)?;
        write(
            &stage.path().join("document.yml"),
            &json!({"schema_version":"1","document_id":id,"source":info,"extraction":extraction_hash}),
        )?;
        write(
            &stage.path().join("proposal.json"),
            &json!({"schema_version":"1","document_id":id,"base":base}),
        )?;
        let mut entries = vec![];
        let mut tables = vec![];
        for (i, sheet) in book.sheets().iter().enumerate() {
            let page = format!("sheet-{}", i + 1);
            let mut rows = json!({});
            let mut formulas = json!({});
            let mut columns = BTreeSet::new();
            for c in array(&sheet["cells"])? {
                let address = string(&c["address"])?;
                let (_, row) = excel::coordinate(address)?;
                let column = address.trim_end_matches(|c: char| c.is_ascii_digit());
                columns.insert(column.to_owned());
                let row_id = format!("r{row}");
                rows[&row_id][column] = c["value"].clone();
                let target = json!({"sheet":sheet["name"],"cell":address});
                entries.push(json!({"page":page,"block":"table-1","field":c["id"],"origins":[],"reason":"原本から転記","target":target,"writeback":if book.parser()=="native-text/1"||c["type"]=="formula"||c["type"]=="error"{"excluded"}else{"cell"},"position":{"row":row_id,"column":column,"type":kind(&c["value"])}}));
                if c["type"] == "formula" {
                    let f = string(&c["id"])?;
                    formulas[f]["formula"] = json!(format!("={}", string(&c["formula"])?));
                    entries.push(json!({"page":page,"block":"formulas","field":format!("{f}-formula"),"origins":[],"reason":"数式原文。Rust版では変更不可","target":target,"writeback":"excluded","position":{"row":f,"column":"formula","type":"string"}}));
                }
            }
            let mut blocks = json!({"extraction-notes":{"title":"未抽出・注意事項","text":&note}});
            entries.push(json!({"page":page,"block":"extraction-notes","field":null,"origins":[],"reason":"抽出範囲の申告","target":null,"writeback":"excluded"}));
            for (block, body, cols) in [
                ("table-1", rows, columns.into_iter().collect::<Vec<_>>()),
                ("formulas", formulas, vec!["formula".into()]),
            ] {
                if body.as_object().is_some_and(|o| !o.is_empty()) {
                    blocks[block] =
                        json!({"title":if block=="table-1"{"本文"}else{"数式原文"},"rows":body});
                    tables.push(json!({"page":page,"block":block,"columns":cols,"references":[]}));
                    entries.push(json!({"page":page,"block":block,"field":null,"origins":[],"reason":"表の構造","target":null,"writeback":"excluded"}));
                }
            }
            write(
                &stage
                    .path()
                    .join("content")
                    .join(excel::filename(string(&sheet["name"])?)),
                &json!({"schema_version":"1","document_id":id,"page_id":page,"source_path":source_path,"title":sheet["name"],"blocks":blocks}),
            )?;
        }
        write(
            &stage.path().join("mappings.yml"),
            &json!({"schema_version":"1","entries":entries,"tables":tables,"omissions":[],"operations":[],"interpretation":interpretation}),
        )?;
        ensure!(
            fs::read(&source)? == book.raw(),
            "source changed during import"
        );
        write(&stage.path().join("extraction.json"), &extraction)?;
        self.inspect(stage.path(), false)?;
        let proposal = parent.join(&proposal_id);
        fs::rename(stage.path(), &proposal)?;
        Ok(
            json!({"proposal_id":proposal_id,"proposal":proposal,"document_id":id,"findings":extraction["findings"]}),
        )
    }
}
