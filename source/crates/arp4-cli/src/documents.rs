use crate::{
    data::*,
    excel::{self, Workbook},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const RECORDS: [&str; 4] = [
    "document.yml",
    "mappings.yml",
    "formation.json",
    "review.json",
];
pub struct Store {
    pub root: PathBuf,
    pub arp: PathBuf,
    pub directory: PathBuf,
}
pub struct Inspection {
    pub meta: Value,
    pub extraction: Value,
    pub mappings: Value,
    pub values: BTreeMap<(String, String, String), Value>,
    pub fingerprint: String,
    pub reviewed: bool,
    pub source_current: bool,
}
fn key(entry: &Value) -> Result<(String, String, String)> {
    Ok((
        string(&entry["page"])?.into(),
        string(&entry["block"])?.into(),
        entry["field"].as_str().unwrap_or("").into(),
    ))
}
fn target(value: &Value) -> Result<(String, String)> {
    Ok((
        string(&value["sheet"])?.into(),
        string(&value["cell"])?.into(),
    ))
}
fn nonempty(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "nonempty actor/model/reviewer required"
    );
    Ok(())
}

impl Store {
    pub fn init(root: &Path, directory: &str) -> Result<Self> {
        fs::create_dir_all(root)?;
        let root = dunce::canonicalize(root)?;
        let dest = under(&root, directory)?;
        ensure!(
            ![".arp", ".git", "src", "tests"].contains(&directory.split('/').next().unwrap_or("")),
            "choose dedicated document directory"
        );
        let config = under(&root, ".arp/config.yml")?;
        if config.exists() {
            let store = Self::open(&root)?;
            ensure!(
                store.directory == dest,
                "document directory already configured"
            );
            return Ok(store);
        }
        ensure!(
            !dest.exists() || fs::read_dir(&dest)?.next().is_none(),
            "refusing nonempty document directory"
        );
        fs::create_dir_all(dest.join("documents"))?;
        write(
            &config,
            &json!({"schema_version":"1","documents":{"directory":directory}}),
        )?;
        let store = Self::open(&root)?;
        immutable(
            &under(&store.arp, "originals/.gitattributes")?,
            b"** -text\n",
        )?;
        immutable(
            &under(&store.arp, "evidence/.gitattributes")?,
            b"sources/** -text\nassets/** -text\nprompts/** -text\n*.json text eol=lf\n",
        )?;
        immutable(&dest.join("README.md"),"# 文書正本\n\n本文は documents/**/content/*.yml。編集後に arp4 documents check と diff を実行し、review 後に export します。\n".as_bytes())?;
        immutable(&dest.join("AGENTS.md"),"# 文書編集\n\n本文YAMLの既存の行・列の値を編集できます。型とIDを維持してください。原本・抽出証跡は変更しないでください。\nRust版では構造変更・edit-plan・仕様生成は未対応です。成形を実施した後にrecordで実際のactor/model/promptを記録し、権限のある担当者がadopt/reviewします。\n".as_bytes())?;
        Ok(store)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let root = dunce::canonicalize(root)?;
        let arp = under(&root, ".arp")?;
        let config = read(&under(&root, ".arp/config.yml")?, Some("project-config"))?;
        let relative = string(&config["documents"]["directory"])?;
        ensure!(
            ![".arp", ".git", "src", "tests"].contains(&relative.split('/').next().unwrap_or("")),
            "invalid document directory"
        );
        let directory = under(&root, relative)?;
        Ok(Self {
            root,
            arp,
            directory,
        })
    }
    fn index(&self) -> Result<Value> {
        let p = under(&self.arp, "document-paths.json")?;
        if p.exists() {
            read(&p, Some("document-paths"))
        } else {
            Ok(json!({}))
        }
    }
    pub fn document(&self, id: &str) -> Result<PathBuf> {
        identifier(id)?;
        let index = self.index()?;
        under(
            &self.directory.join("documents"),
            index[id].as_str().unwrap_or(id),
        )
    }
    pub fn proposal(&self, id: &str) -> Result<PathBuf> {
        identifier(id)?;
        let mut matches = vec![];
        for (name, path) in files(&under(&self.arp, "proposals")?)? {
            if name.ends_with("/proposal.json")
                && path
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|s| s == id)
            {
                matches.push(path.parent().unwrap().to_path_buf());
            }
        }
        ensure!(matches.len() == 1, "missing or ambiguous proposal: {id}");
        Ok(matches.remove(0))
    }
    pub fn management(&self, dir: &Path) -> Result<PathBuf> {
        if dir.join("document.yml").is_file() {
            return Ok(dir.into());
        }
        if let Ok(rel) = dir.strip_prefix(self.directory.join("documents")) {
            under(
                &self.arp.join("documents"),
                &rel.to_string_lossy().replace('\\', "/"),
            )
        } else {
            Ok(dir.into())
        }
    }
    fn logical(&self, dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
        let mut out = files(dir)?;
        let metadata = self.management(dir)?;
        if metadata != dir {
            for (k, v) in files(&metadata)? {
                ensure!(
                    out.insert(k, v).is_none(),
                    "duplicate logical document file"
                );
            }
        }
        Ok(out)
    }
    pub fn fingerprint(&self, dir: &Path) -> Result<Option<String>> {
        if !self.management(dir)?.join("document.yml").exists() {
            return Ok(None);
        }
        let mut map = serde_json::Map::new();
        for (name, path) in self.logical(dir)? {
            if name.starts_with("original/")
                || ["review.json", "formation.json", "proposal.json"].contains(&name.as_str())
            {
                continue;
            }
            let mut raw = fs::read(&path)?;
            if ["md", "yml", "yaml", "json"]
                .contains(&path.extension().and_then(|s| s.to_str()).unwrap_or(""))
            {
                raw = String::from_utf8(raw)?.replace("\r\n", "\n").into_bytes()
            };
            map.insert(name, json!(hash(&raw)));
        }
        Ok(Some(hash(&encoded(&Value::Object(map)))))
    }
    fn snapshot(&self, dir: &Path, group: &str, key: &str) -> Result<()> {
        let dest = under(&self.arp, &format!("evidence/{group}/{key}"))?;
        if dest.exists() {
            ensure!(
                self.fingerprint(&dest)?.as_deref() == Some(key),
                "snapshot integrity failure"
            );
            return Ok(());
        }
        for (name, path) in self.logical(dir)? {
            if name.starts_with("original/")
                || ["review.json", "formation.json", "proposal.json"].contains(&name.as_str())
            {
                continue;
            }
            immutable(&under(&dest, &name)?, &fs::read(path)?)?;
        }
        ensure!(
            self.fingerprint(&dest)?.as_deref() == Some(key),
            "source changed during snapshot"
        );
        Ok(())
    }
    pub fn import(&self, source: &Path, id: &str) -> Result<Value> {
        identifier(id)?;
        let source = dunce::canonicalize(if source.is_absolute() {
            source.to_owned()
        } else {
            self.root.join(source)
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
        ensure!(
            ["xlsx", "xlsm"].contains(&ext.as_str()),
            "Rust import supports .xlsx/.xlsm only"
        );
        let current = self.document(id)?;
        let index = self.index()?;
        let source_path = if source.starts_with(&self.arp) {
            let meta = read(
                &self.management(&current)?.join("document.yml"),
                Some("document"),
            )?;
            ensure!(
                source == under(&self.root, string(&meta["source"]["path"])?)?,
                "import original, not ARP evidence"
            );
            string(&meta["source"]["source_path"])?.to_owned()
        } else {
            ensure!(
                !source.starts_with(&self.directory),
                "cannot import ARP document data"
            );
            relative
        };
        if let Some(existing) = index[id].as_str() {
            ensure!(
                existing == source_path,
                "source relocation is not yet supported by Rust"
            );
        }
        let dest = under(&self.directory.join("documents"), &source_path)?;
        for (other, rel) in index.as_object().unwrap() {
            let p = under(&self.directory.join("documents"), string(rel)?)?;
            ensure!(
                other == id || !(p.starts_with(&dest) || dest.starts_with(&p)),
                "document path collision"
            );
        }
        if dest.exists() {
            ensure!(
                self.management(&dest)?.join("document.yml").exists(),
                "unmanaged document destination"
            );
        }
        for (name, path) in files(&under(&self.arp, "proposals")?)? {
            if name.ends_with("/document.yml") {
                let m = read(&path, Some("document"))?;
                if m["document_id"] != id {
                    let p = under(
                        &self.directory.join("documents"),
                        string(&m["source"]["source_path"])?,
                    )?;
                    ensure!(
                        !(p.starts_with(&dest) || dest.starts_with(&p)),
                        "proposal path collision"
                    );
                }
            }
        }
        let book = Workbook::open(&source)?;
        let sha = hash(&book.raw);
        let snapshot_rel = format!(".arp/evidence/sources/{sha}.{ext}");
        let managed_rel = format!(
            ".arp/originals/{source_path}/original/{}",
            source.file_name().unwrap().to_string_lossy()
        );
        let managed = under(&self.root, &managed_rel)?;
        if managed.exists() {
            ensure!(
                fs::read(&managed)? == book.raw,
                "managed original differs; update managed original and reimport"
            );
        }
        let info = json!({"path":managed_rel,"sha256":sha,"snapshot":snapshot_rel,"source_path":source_path});
        let note = "Rust版はセル値・数式原文・結合範囲を抽出します。図形・画像・コメント・印刷情報・OCRの内容は未抽出です。原本の表示を確認してください。";
        let extraction = json!({"schema_version":"1","document_id":id,"source":info,"parser":format!("arp4-rust/{};cells/1;ocr=false",env!("CARGO_PKG_VERSION")),"pages":[],"sheets":book.sheets,"findings":[{"level":"warning","code":"R001","message":note}],"assets":[]});
        validate("extraction", &extraction)?;
        let extraction_hash = hash(&encoded(&extraction));
        let proposal_id = format!("{id}-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let parent = under(&self.arp, &format!("proposals/{source_path}"))?;
        fs::create_dir_all(&parent)?;
        let stage = tempfile::tempdir_in(&parent)?;
        let base = self.fingerprint(&current)?;
        if let Some(key) = &base {
            self.snapshot(&current, "authorities", key)?
        }
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
        for (i, sheet) in book.sheets.iter().enumerate() {
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
                entries.push(json!({"page":page,"block":"table-1","field":c["id"],"origins":[],"reason":"原本から転記","target":target,"writeback":if c["type"]=="formula"||c["type"]=="error"{"excluded"}else{"cell"},"position":{"row":row_id,"column":column,"type":kind(&c["value"])}}));
                if c["type"] == "formula" {
                    let f = string(&c["id"])?;
                    formulas[f]["formula"] = json!(format!("={}", string(&c["formula"])?));
                    entries.push(json!({"page":page,"block":"formulas","field":format!("{f}-formula"),"origins":[],"reason":"数式原文。Rust版では変更不可","target":target,"writeback":"excluded","position":{"row":f,"column":"formula","type":"string"}}));
                }
            }
            let mut blocks = json!({"extraction-notes":{"title":"未抽出・注意事項","text":note}});
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
            &json!({"schema_version":"2","entries":entries,"tables":tables,"omissions":[],"operations":[]}),
        )?;
        ensure!(
            fs::read(&source)? == book.raw,
            "source changed during import"
        );
        immutable(&under(&self.root, &snapshot_rel)?, &book.raw)?;
        immutable(&managed, &book.raw)?;
        immutable(
            &under(
                &self.arp,
                &format!("evidence/extractions/{extraction_hash}.json"),
            )?,
            &encoded(&extraction),
        )?;
        self.inspect(stage.path(), false)?;
        let proposal = parent.join(&proposal_id);
        fs::rename(stage.path(), &proposal)?;
        let mut index = index;
        index[id] = json!(source_path);
        write(&under(&self.arp, "document-paths.json")?, &index)?;
        Ok(
            json!({"proposal_id":proposal_id,"proposal":proposal,"document_id":id,"findings":extraction["findings"]}),
        )
    }

    pub fn inspect(&self, dir: &Path, require_reviewed: bool) -> Result<Inspection> {
        let fp = self.fingerprint(dir)?.context("document not found")?;
        let metadata = self.management(dir)?;
        let meta = read(&metadata.join("document.yml"), Some("document"))?;
        let id = string(&meta["document_id"])?;
        identifier(id)?;
        if dir.starts_with(self.directory.join("documents")) {
            ensure!(self.document(id)? == dir, "document path identity mismatch");
        }
        let ext_key = string(&meta["extraction"])?;
        let extraction = read(
            &under(&self.arp, &format!("evidence/extractions/{ext_key}.json"))?,
            Some("extraction"),
        )?;
        ensure!(
            hash(&encoded(&extraction)) == ext_key,
            "extraction integrity failure"
        );
        ensure!(
            meta["source"] == extraction["source"]
                && meta["document_id"] == extraction["document_id"],
            "document/extraction mismatch"
        );
        let snapshot = under(&self.root, string(&meta["source"]["snapshot"])?)?;
        ensure!(
            snapshot.starts_with(self.arp.join("evidence/sources"))
                && hash(&fs::read(&snapshot)?) == meta["source"]["sha256"],
            "snapshot integrity failure"
        );
        let mappings = read(&metadata.join("mappings.yml"), Some("mappings"))?;
        ensure!(
            array(&mappings["operations"])?.is_empty(),
            "structural Excel operations are not yet supported by Rust"
        );
        let entries = array(&mappings["entries"])?;
        let tables = mappings["tables"].as_array().cloned().unwrap_or_default();
        let mut table_keys = BTreeSet::new();
        for t in &tables {
            ensure!(
                table_keys.insert((
                    string(&t["page"])?.to_owned(),
                    string(&t["block"])?.to_owned()
                )),
                "duplicate table definition"
            );
        }
        let mut values = BTreeMap::new();
        let mut expected = BTreeSet::new();
        let mut page_ids = BTreeSet::new();
        let mut references = vec![];
        for (name, path) in self.logical(dir)? {
            if name.starts_with("original/")
                || name.starts_with("assets/")
                || RECORDS.contains(&name.as_str())
                || name == "proposal.json"
            {
                continue;
            }
            ensure!(
                name.starts_with("content/") && (name.ends_with(".yml") || name.ends_with(".yaml")),
                "unmanaged document file: {name}"
            );
            let page = read(&path, Some("content"))?;
            ensure!(
                page["document_id"] == meta["document_id"]
                    && page["source_path"] == meta["source"]["source_path"],
                "page identity mismatch"
            );
            let page_id = string(&page["page_id"])?.to_owned();
            ensure!(page_ids.insert(page_id.clone()), "duplicate page ID");
            for (block, body) in page["blocks"].as_object().unwrap() {
                let base = (page_id.clone(), block.clone());
                expected.insert((base.0.clone(), base.1.clone(), String::new()));
                // Local links require full link/asset semantics; reject unsupported links instead of silently accepting them.
                if let Some(links) = body["links"].as_array() {
                    ensure!(
                        links
                            .iter()
                            .all(|v| v.as_str().is_some_and(|s| s.starts_with("https://")
                                || s.starts_with("http://")
                                || s.starts_with("mailto:"))),
                        "local document links are not yet supported by Rust"
                    );
                }
                if let Some(fields) = body["fields"].as_object() {
                    for (field, value) in fields {
                        values.insert(
                            (base.0.clone(), base.1.clone(), field.clone()),
                            value.clone(),
                        );
                    }
                }
                if let Some(rows) = body["rows"].as_object() {
                    let table = tables
                        .iter()
                        .find(|t| t["page"] == page_id && t["block"] == *block)
                        .context("rows require table definition")?;
                    ensure!(body.get("fields").is_none(), "table cannot contain fields");
                    let mut consumed = BTreeSet::new();
                    for e in entries.iter().filter(|e| {
                        e["page"] == page_id && e["block"] == *block && e.get("position").is_some()
                    }) {
                        let pos = &e["position"];
                        let row = string(&pos["row"])?;
                        let col = string(&pos["column"])?;
                        let value = rows
                            .get(row)
                            .and_then(|r| r.get(col))
                            .context("missing table position")?;
                        ensure!(
                            consumed.insert((row.to_owned(), col.to_owned())),
                            "duplicate table position"
                        );
                        ensure!(
                            kind(value) != "object"
                                && (value.is_null() || kind(value) == pos["type"]),
                            "table type/value mismatch"
                        );
                        ensure!(
                            array(&table["columns"])?.contains(&json!(col)),
                            "unknown column"
                        );
                        ensure!(!e["field"].is_null(), "table field required");
                        ensure!(
                            values.insert(key(e)?, value.clone()).is_none(),
                            "duplicate table field"
                        );
                    }
                    for r in array(&table["references"])? {
                        let row = string(&r["row"])?;
                        let col = string(&r["column"])?;
                        ensure!(
                            consumed.insert((row.into(), col.into()))
                                && rows.get(row).and_then(|v| v.get(col))
                                    == Some(
                                        &json!({"ref":{"block":r["block"],"field":r["field"]}})
                                    ),
                            "invalid table reference"
                        );
                        ensure!(
                            array(&table["columns"])?.contains(&json!(col)),
                            "unknown reference column"
                        );
                        references.push((
                            page_id.clone(),
                            string(&r["block"])?.into(),
                            string(&r["field"])?.into(),
                        ));
                    }
                    let mut count = 0;
                    for row in rows.values() {
                        let obj = row.as_object().context("invalid table row")?;
                        ensure!(!obj.is_empty(), "empty table row");
                        count += obj.len();
                    }
                    ensure!(count == consumed.len(), "unmapped table value");
                } else {
                    ensure!(!table_keys.contains(&base), "mapped table has no rows");
                }
            }
        }
        ensure!(!page_ids.is_empty(), "content is empty");
        for r in references {
            ensure!(values.contains_key(&r), "missing table reference target");
        }
        for (p, b) in table_keys {
            ensure!(
                expected.contains(&(p, b, String::new())),
                "orphaned table definition"
            );
        }
        expected.extend(values.keys().cloned());
        let mut cells = BTreeMap::new();
        for sheet in array(&extraction["sheets"])? {
            for c in array(&sheet["cells"])? {
                ensure!(
                    cells
                        .insert(
                            (
                                string(&sheet["name"])?.to_owned(),
                                string(&c["address"])?.to_owned()
                            ),
                            c
                        )
                        .is_none(),
                    "duplicate extraction cell"
                );
            }
        }
        let mut origins = BTreeSet::new();
        for p in array(&extraction["pages"])? {
            for c in array(&p["chunks"])? {
                ensure!(
                    origins.insert((string(&p["id"])?.to_owned(), string(&c["id"])?.to_owned())),
                    "duplicate extraction origin"
                );
            }
        }
        let mut actual = BTreeSet::new();
        let mut covered_cells = BTreeSet::new();
        let mut covered_origins = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for e in entries {
            let k = key(e)?;
            ensure!(actual.insert(k.clone()), "duplicate mapping");
            ensure!(
                !array(&e["origins"])?.is_empty() || !string(&e["reason"])?.trim().is_empty(),
                "new text requires reason"
            );
            for origin in array(&e["origins"])? {
                let pair = (
                    string(&origin["page"])?.to_owned(),
                    string(&origin["block"])?.to_owned(),
                );
                ensure!(origins.contains(&pair), "missing origin");
                covered_origins.insert(pair);
            }
            if !e["target"].is_null() {
                let pair = target(&e["target"])?;
                ensure!(cells.contains_key(&pair), "missing Excel target");
                covered_cells.insert(pair);
            }
            match string(&e["writeback"])? {
                "excluded" => ensure!(
                    !string(&e["reason"])?.trim().is_empty(),
                    "exclusion requires reason"
                ),
                "pending" => {}
                "cell" => {
                    let pair = target(&e["target"])?;
                    ensure!(
                        destinations.insert(pair.clone()),
                        "duplicate writeback target"
                    );
                    let source = cells.get(&pair).context("missing target")?;
                    ensure!(
                        source["type"] != "formula" && source["type"] != "error",
                        "formula/error cell cannot be overwritten"
                    );
                    let value = values.get(&k).context("writeback requires field")?;
                    ensure!(
                        value.is_null()
                            || source["type"] == kind(value)
                            || source["type"] == "null",
                        "Excel target type mismatch"
                    );
                    let (col, row) = excel::coordinate(&pair.1)?;
                    for sheet in array(&extraction["sheets"])?
                        .iter()
                        .filter(|s| s["name"] == pair.0)
                    {
                        for merge in array(&sheet["merges"])? {
                            let (a, b) = string(merge)?.split_once(':').context("invalid merge")?;
                            let (c1, r1) = excel::coordinate(a)?;
                            let (c2, r2) = excel::coordinate(b)?;
                            ensure!(
                                !(c1 <= col && col <= c2 && r1 <= row && row <= r2)
                                    || (col, row) == (c1, r1),
                                "merged cell is not top-left"
                            );
                        }
                    }
                }
                _ => bail!("formula/operation writeback is not yet supported by Rust"),
            }
        }
        ensure!(expected == actual, "mapping coverage mismatch");
        for o in array(&mappings["omissions"])? {
            ensure!(
                !string(&o["reason"])?.trim().is_empty()
                    && (!o["origin"].is_null() || !o["target"].is_null()),
                "invalid omission"
            );
            if !o["origin"].is_null() {
                let pair = (
                    string(&o["origin"]["page"])?.to_owned(),
                    string(&o["origin"]["block"])?.to_owned(),
                );
                ensure!(
                    origins.contains(&pair) && covered_origins.insert(pair),
                    "invalid/duplicate omission origin"
                );
            }
            if !o["target"].is_null() {
                let pair = target(&o["target"])?;
                ensure!(
                    cells.contains_key(&pair) && covered_cells.insert(pair),
                    "invalid/duplicate omission target"
                );
            }
        }
        ensure!(
            covered_cells == cells.keys().cloned().collect() && covered_origins == origins,
            "source coverage incomplete"
        );
        for asset in array(&extraction["assets"])? {
            ensure!(
                hash(&fs::read(under(
                    &self.arp,
                    &format!("evidence/assets/{}", string(&asset["path"])?)
                )?)?)
                    == asset["sha256"],
                "asset evidence changed"
            );
        }
        let mut reviewed = false;
        let review = metadata.join("review.json");
        if review.exists() {
            let r = read(&review, Some("review"))?;
            let (f, key) = self.formation(dir)?;
            ensure!(
                r["formation"] == key
                    && f["document_id"] == meta["document_id"]
                    && f["extraction"] == meta["extraction"],
                "review/formation identity mismatch"
            );
            reviewed = r["content"] == fp;
        }
        ensure!(
            !require_reviewed || reviewed,
            "content changed or not reviewed; run documents review"
        );
        let original = under(&self.root, string(&meta["source"]["path"])?)?;
        let source_current =
            original.is_file() && hash(&fs::read(original)?) == meta["source"]["sha256"];
        Ok(Inspection {
            meta,
            extraction,
            mappings,
            values,
            fingerprint: fp,
            reviewed,
            source_current,
        })
    }

    fn formation(&self, dir: &Path) -> Result<(Value, String)> {
        let f = read(
            &self.management(dir)?.join("formation.json"),
            Some("formation"),
        )?;
        let key = hash(&encoded(&f));
        ensure!(
            read(
                &under(&self.arp, &format!("evidence/formations/{key}.json"))?,
                Some("formation")
            )? == f,
            "formation evidence mismatch"
        );
        ensure!(
            hash(&fs::read(under(
                &self.arp,
                &format!("evidence/prompts/{}.txt", string(&f["prompt_sha256"])?)
            )?)?)
                == f["prompt_sha256"],
            "formation prompt changed"
        );
        ensure!(
            self.fingerprint(&under(
                &self.arp,
                &format!("evidence/formed/{}", string(&f["content"])?)
            )?)?
            .as_deref()
                == f["content"].as_str(),
            "formation baseline changed"
        );
        Ok((f, key))
    }
    pub fn record(&self, id: &str, model: &str, actor: &str, prompt: &Path) -> Result<Value> {
        nonempty(model)?;
        nonempty(actor)?;
        let dir = self.proposal(id)?;
        let result = self.inspect(&dir, false)?;
        let raw = fs::read(prompt)?;
        let prompt_hash = hash(&raw);
        let f = json!({"schema_version":"1","document_id":result.meta["document_id"],"extraction":result.meta["extraction"],"content":result.fingerprint,"actor":actor,"model":model,"prompt_sha256":prompt_hash});
        let key = hash(&encoded(&f));
        self.snapshot(&dir, "formed", &result.fingerprint)?;
        immutable(
            &under(&self.arp, &format!("evidence/prompts/{prompt_hash}.txt"))?,
            &raw,
        )?;
        immutable(
            &under(&self.arp, &format!("evidence/formations/{key}.json"))?,
            &encoded(&f),
        )?;
        write(&dir.join("formation.json"), &f)?;
        Ok(f)
    }
    pub fn review(&self, id: &str, reviewer: &str) -> Result<Value> {
        nonempty(reviewer)?;
        let dir = self.document(id)?;
        let result = self.inspect(&dir, false)?;
        let (f, key) = self.formation(&dir)?;
        ensure!(
            f["document_id"] == id && f["extraction"] == result.meta["extraction"],
            "formation identity mismatch"
        );
        let r = json!({"schema_version":"1","content":result.fingerprint,"reviewer":reviewer,"formation":key});
        self.snapshot(&dir, "authorities", &result.fingerprint)?;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&result.fingerprint),
            "document changed during review"
        );
        write(&self.management(&dir)?.join("review.json"), &r)?;
        Ok(r)
    }
    pub fn adopt(&self, id: &str, reviewer: &str) -> Result<Value> {
        nonempty(reviewer)?;
        let proposal = self.proposal(id)?;
        let plan = read(&proposal.join("proposal.json"), Some("proposal"))?;
        let result = self.inspect(&proposal, false)?;
        let (f, key) = self.formation(&proposal)?;
        ensure!(
            f["document_id"] == plan["document_id"]
                && plan["document_id"] == result.meta["document_id"]
                && f["extraction"] == result.meta["extraction"]
                && f["content"] == result.fingerprint,
            "proposal changed after record; record again"
        );
        let doc_id = string(&plan["document_id"])?;
        let current = self.document(doc_id)?;
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned),
            "authority changed since import"
        );
        let relative = string(&result.meta["source"]["source_path"])?;
        let dest = under(&self.directory.join("documents"), relative)?;
        ensure!(current == dest, "relocation unsupported");
        let metadata = under(&self.arp.join("documents"), relative)?;
        ensure!(
            !dest.exists() || self.management(&dest)? == metadata,
            "legacy layout requires Python migration"
        );
        self.snapshot(&proposal, "authorities", &result.fingerprint)?;
        let stage = tempfile::tempdir_in(&self.arp)?;
        let content = stage.path().join("content-dir");
        let records = stage.path().join("records");
        fs::create_dir(&content)?;
        fs::create_dir(&records)?;
        for (name, path) in self.logical(&proposal)? {
            if ["proposal.json", "review.json"].contains(&name.as_str()) {
                continue;
            }
            let parent = if RECORDS.contains(&name.as_str()) {
                &records
            } else {
                &content
            };
            immutable(&under(parent, &name)?, &fs::read(path)?)?;
        }
        write(
            &records.join("review.json"),
            &json!({"schema_version":"1","content":result.fingerprint,"reviewer":reviewer,"formation":key}),
        )?;
        ensure!(
            self.fingerprint(&current)? == plan["base"].as_str().map(str::to_owned)
                && self.fingerprint(&proposal)?.as_deref() == Some(&result.fingerprint),
            "document changed during adoption"
        );
        let backup = under(
            &self.arp,
            &format!("proposals/replaced-{}", uuid::Uuid::new_v4().simple()),
        )?;
        fs::create_dir_all(&backup)?;
        let mut backups = vec![];
        let mut installed = vec![];
        let outcome = (|| -> Result<()> {
            for (n, p) in [("content", &dest), ("metadata", &metadata)] {
                fs::create_dir_all(p.parent().unwrap())?;
                if p.exists() {
                    let b = backup.join(n);
                    fs::rename(p, &b)?;
                    backups.push((p.clone(), b));
                }
            }
            for (from, to) in [(&content, &dest), (&records, &metadata)] {
                fs::rename(from, to)?;
                installed.push((to.clone(), from.clone()));
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            for (to, from) in installed.into_iter().rev() {
                fs::rename(to, from)?
            }
            for (to, from) in backups.into_iter().rev() {
                fs::rename(from, to)?
            }
            return Err(e);
        }
        Ok(json!({"document_id":doc_id,"document":dest}))
    }

    fn diff_values(&self, dir: Option<&Path>) -> Result<Value> {
        let Some(dir) = dir else { return Ok(json!({})) };
        if self.fingerprint(dir)?.is_none() {
            return Ok(json!({}));
        }
        let mut out = json!({});
        for (name, path) in self.logical(dir)? {
            if name.starts_with("content/") || name == "mappings.yml" || name == "document.yml" {
                out[&name] = read(&path, None)?
            } else if name.starts_with("assets/") {
                out[&name] = json!(hash(&fs::read(path)?));
            }
        }
        Ok(out)
    }
    pub fn diff(&self, proposal: Option<&str>, document: Option<&str>) -> Result<Value> {
        fn compare(before: &Value, after: &Value, path: &str, out: &mut Vec<Value>) {
            if before == after {
                return;
            }
            if let (Some(a), Some(b)) = (before.as_object(), after.as_object()) {
                let keys: BTreeSet<_> = a.keys().chain(b.keys()).collect();
                for k in keys {
                    let p = format!("{path}/{k}");
                    match(a.get(k),b.get(k)){
                    (Some(x),Some(y))=>compare(x,y,&p,out),
                    (x,y)=>out.push(json!({"path":p,"before":x,"after":y,"kind":if x.is_none(){"added"}else{"removed"}}))
                }
                }
            } else {
                out.push(json!({"path":path,"before":before,"after":after,"kind":"changed"}));
            }
        }
        let mut comparisons = vec![];
        let mut changed = false;
        let pairs = if let Some(id) = proposal {
            let p = self.proposal(id)?;
            self.inspect(&p, false)?;
            let plan = read(&p.join("proposal.json"), Some("proposal"))?;
            let current = self.document(string(&plan["document_id"])?)?;
            let base = plan["base"]
                .as_str()
                .map(|h| under(&self.arp, &format!("evidence/authorities/{h}")))
                .transpose()?;
            if let Some(b) = &base {
                ensure!(
                    self.fingerprint(b)?.as_deref() == plan["base"].as_str(),
                    "diff baseline integrity failure"
                );
            }
            changed = self.fingerprint(&current)? != plan["base"].as_str().map(str::to_owned);
            vec![
                ("base_to_current", base, Some(current.clone())),
                ("current_to_proposal", Some(current), Some(p)),
            ]
        } else {
            let current = self.document(document.context("document or proposal required")?)?;
            self.inspect(&current, false)?;
            let review = self.management(&current)?.join("review.json");
            let (group, key) = if review.exists() {
                (
                    "authorities",
                    read(&review, Some("review"))?["content"].clone(),
                )
            } else {
                ("formed", self.formation(&current)?.0["content"].clone())
            };
            let baseline = under(&self.arp, &format!("evidence/{group}/{}", string(&key)?))?;
            ensure!(
                self.fingerprint(&baseline)?.as_deref() == key.as_str(),
                "diff baseline integrity failure"
            );
            vec![("review_to_current", Some(baseline), Some(current))]
        };
        for (label, left, right) in pairs {
            let mut changes = vec![];
            compare(
                &self.diff_values(left.as_deref())?,
                &self.diff_values(right.as_deref())?,
                "",
                &mut changes,
            );
            comparisons.push(json!({"kind":label,"changes":changes}));
        }
        Ok(
            json!({"schema_version":"1","implementation":"rust","comparisons":comparisons,"authority_changed":changed}),
        )
    }
    pub fn export(&self, id: &str, output: Option<&Path>, engine: &str) -> Result<Value> {
        ensure!(
            engine == "auto" || engine == "xml",
            "Rust supports XML cell writeback only"
        );
        let dir = self.document(id)?;
        let result = self.inspect(&dir, true)?;
        ensure!(
            result.source_current,
            "source changed/missing; re-import before export"
        );
        let source = under(&self.root, string(&result.meta["source"]["snapshot"])?)?;
        let book = Workbook::open(&source)?;
        let mut changes = vec![];
        let mut excluded = vec![];
        let mut pending = vec![];
        for e in array(&result.mappings["entries"])? {
            match string(&e["writeback"])? {
                "excluded" => excluded.push(e.clone()),
                "pending" => pending.push(e.clone()),
                "cell" => {
                    let pair = target(&e["target"])?;
                    let old = array(&result.extraction["sheets"])?
                        .iter()
                        .find(|s| s["name"] == pair.0)
                        .and_then(|s| {
                            s["cells"]
                                .as_array()?
                                .iter()
                                .find(|c| c["address"] == pair.1)
                        })
                        .context("missing cell")?;
                    let new = &result.values[&key(e)?];
                    if old["value"] != *new {
                        changes.push(json!({"sheet":pair.0,"cell":pair.1,"before":old["value"],"after":new,"field":e["field"]}));
                    }
                }
                _ => bail!("unsupported writeback"),
            }
        }
        let mut report = json!({"schema_version":"1","document_id":id,"content":result.fingerprint,"source_sha256":result.meta["source"]["sha256"],"changes":changes,"unreflected":pending,"excluded":excluded,"omissions":result.mappings["omissions"],"operations":[],"deleted_cells":[],"engine":"xml","complete":pending.is_empty(),"written":false});
        let Some(output) = output else {
            return Ok(report);
        };
        ensure!(pending.is_empty(), "unresolved writeback mappings");
        let output = if output.is_absolute() {
            output.to_owned()
        } else {
            std::env::current_dir()?.join(output)
        };
        let relative = output
            .strip_prefix(&self.root)
            .context("output must be inside project .arp/out")?
            .to_string_lossy()
            .replace('\\', "/");
        let output = under(&self.root, &relative)?;
        ensure!(
            output.starts_with(self.arp.join("out")) && output.extension() == source.extension(),
            "output must be a new matching Excel file inside .arp/out"
        );
        let report_path = output.with_extension(format!(
            "{}.report.json",
            output.extension().unwrap().to_string_lossy()
        ));
        ensure!(
            !output.exists() && !report_path.exists(),
            "output/report already exists"
        );
        fs::create_dir_all(output.parent().unwrap())?;
        let stage = tempfile::tempdir_in(output.parent().unwrap())?;
        let staged = stage.path().join("result.xlsx");
        let info = book.patch(&staged, &changes)?;
        for (k, v) in info.as_object().unwrap() {
            report[k] = v.clone()
        }
        report["written"] = json!(true);
        report["output_sha256"] = json!(hash(&fs::read(&staged)?));
        let staged_report = stage.path().join("report.json");
        write(&staged_report, &report)?;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(&result.fingerprint)
                && hash(&fs::read(under(
                    &self.root,
                    string(&result.meta["source"]["path"])?
                )?)?)
                    == result.meta["source"]["sha256"],
            "source/document changed during export"
        );
        fs::hard_link(&staged, &output)?;
        if let Err(error) = fs::hard_link(&staged_report, &report_path) {
            if fs::read(&output).ok() == fs::read(&staged).ok() {
                fs::remove_file(&output)?
            }
            return Err(error.into());
        }
        Ok(report)
    }
    pub fn status(
        &self,
        id: Option<&str>,
        proposal: Option<&str>,
        require_reviewed: bool,
    ) -> Result<Value> {
        let mut dirs = vec![];
        if let Some(p) = proposal {
            dirs.push(self.proposal(p)?)
        } else if let Some(id) = id {
            dirs.push(self.document(id)?)
        } else {
            for id in self.index()?.as_object().unwrap().keys() {
                let d = self.document(id)?;
                if self.management(&d)?.join("document.yml").exists() {
                    dirs.push(d)
                }
            }
            for (name, path) in files(&under(&self.arp, "proposals")?)? {
                if name.ends_with("/proposal.json") {
                    dirs.push(path.parent().unwrap().into());
                }
            }
        }
        let mut out = vec![];
        for dir in dirs {
            match self.inspect(&dir,require_reviewed){Ok(r)=>out.push(json!({"document_id":r.meta["document_id"],"directory":dir,"reviewed":r.reviewed,"source_current":r.source_current,"content":r.fingerprint,"pending":array(&r.mappings["entries"] )?.iter().filter(|e|e["writeback"]=="pending").count(),"state":if r.reviewed{"reviewed"}else{"needs_record_or_review"}})),Err(e)=>out.push(json!({"directory":dir,"state":"invalid","error":format!("{e:#}")}))}
        }
        Ok(json!(out))
    }
}
