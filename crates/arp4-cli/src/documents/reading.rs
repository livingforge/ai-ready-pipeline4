//! What a review confirms of a document, apart from how this version of ARP
//! stores it. The content hash a review binds is of the stored bytes, which
//! another version stores otherwise; the reading is of what the document
//! reads as, so a proposal that reads as the reviewed content keeps its
//! review when it is adopted, as a proposal import makes again of an
//! unchanged original after another version changed how documents are stored.
use super::*;

/// The version of what [`Store::reading`] reads of a document. A reading made
/// otherwise is another, and the reviews bound to readings made before are not
/// kept.
const READING_VERSION: &str = "1";

/// The version of review.json this program writes and reads. A review another
/// version recorded lapses rather than being read.
pub(super) const REVIEW_VERSION: &str = "2";

/// The members of the reading form of an extraction that say how and by
/// which program it is stored, or that document.yml records, not what the
/// original holds.
const EXTRACTION_STORAGE: [&str; 4] = ["schema_version", "parser", "document_id", "source"];

/// The members of a structure interpretation that name the stored extraction
/// it is of, or record its own review.
const STRUCTURE_STORAGE: [&str; 5] = [
    "schema_version",
    "document",
    "source",
    "extraction_hash",
    "review",
];

/// The members of mappings.yml that are not decisions: its version, the
/// correction journal, whose replay the reading holds, and the entries, whose
/// complete list it holds.
const MAPPINGS_STORAGE: [&str; 3] = ["schema_version", "interpretation", "entries"];

impl Store {
    /// The hash of what the document or proposal in `dir` reads as: its
    /// original, the reading of its extraction, its structure interpretation,
    /// its element metadata, each mapping entry with its value and font, the
    /// decisions mappings.yml records and the images it holds. Without the
    /// versions, keys and stored forms of its records, which another version
    /// of ARP writes otherwise.
    pub(super) fn reading(&self, dir: &Path) -> Result<String> {
        let keep = inspection::Keep {
            layout: true,
            pages: &[],
        };
        let (inspected, kept) = self.inspect_layout(dir, false, &Planned::new(), &keep)?;
        let layout = kept.layout.context("layout kept")?;
        let (structure, _) = crate::document_structure::replay_verified_corrections(
            &self.root,
            &inspected.mappings["interpretation"],
            inspected.original_extraction(),
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?;
        let interpretation = elements::interpret(&layout, &structure.elements)?;
        let extraction =
            crate::extraction::reading_hash(inspected.original_extraction(), &EXTRACTION_STORAGE)?;
        let mut members = Lazy::members(&structure.fields, &STRUCTURE_STORAGE);
        members.push((
            "elements",
            Lazy::Items(Box::new(structure.elements.iter().map(|e| Ok(e.lazy())))),
        ));
        let structure = hash_encoded_lazy(Lazy::Object(members))?;
        let mut pages = vec![("pages", elements_pages(&layout, &interpretation)?)];
        // The titles and notes of the tables of each page, which the body
        // writes beside them.
        pages.push((
            "tables",
            Lazy::Object(
                layout
                    .sheets
                    .iter()
                    .map(|(page, sheet)| {
                        (
                            page.as_str(),
                            Lazy::Object(vec![
                                ("block_titles", Lazy::Borrowed(&sheet["block_titles"])),
                                ("notes", Lazy::Borrowed(&sheet["notes"])),
                            ]),
                        )
                    })
                    .collect(),
            ),
        ));
        let elements = hash_encoded_lazy(Lazy::Object(pages))?;
        let content = hash_encoded_lazy(Lazy::Items(Box::new(
            inspected.entries().enumerate().map(|(index, entry)| {
                let mut item = json!({"entry":entry.json()});
                if let Some(field) = inspected.content.get(index) {
                    item["value"] = field.value.clone();
                    if let Some(font) = &field.font {
                        item["font"] = (**font).clone();
                    }
                }
                Ok(Lazy::Value(item))
            }),
        )))?;
        let mappings = hash_encoded_lazy(Lazy::Object(Lazy::members(
            &inspected.mappings,
            &MAPPINGS_STORAGE,
        )))?;
        let mut assets = serde_json::Map::new();
        for (name, path) in self.logical(dir)? {
            if name.starts_with("assets/") {
                assets.insert(name, json!(hash(&fs::read(path)?)));
            }
        }
        Ok(hash_encoded(&json!({
            "reading":READING_VERSION,
            "source":inspected.meta["source"]["sha256"],
            "extraction":extraction,
            "structure":structure,
            "elements":elements,
            "content":content,
            "mappings":mappings,
            "assets":assets,
        })))
    }

    /// The review of the document in `dir` when it is of its content as it is:
    /// recorded by this version, of its formation and of its files. None when
    /// it has none, or a review that has lapsed.
    pub(super) fn current_review(&self, dir: &Path) -> Result<Option<Value>> {
        let Some(review) = recorded_review(&self.management(dir)?.join("review.json"))? else {
            return Ok(None);
        };
        let (_, formation) = self.formation(dir)?;
        let content = self.fingerprint(dir)?;
        Ok((review["formation"] == formation.as_str()
            && review["content"].as_str() == content.as_deref())
        .then_some(review))
    }

    /// The review the proposal in `proposal`, whose formation is the one
    /// `formation` names, keeps when it is adopted in place of the document in
    /// `current`: the document's review, when it is of the document as it is
    /// and the proposal reads as the content it confirmed.
    pub(super) fn kept_review(
        &self,
        current: &Path,
        proposal: &Path,
        formation: &str,
    ) -> Result<Option<Value>> {
        let Some(review) = self.current_review(current)? else {
            return Ok(None);
        };
        if review["reading"] != self.reading(proposal)? {
            return Ok(None);
        }
        let content = self.fingerprint(proposal)?.context("proposal not found")?;
        Ok(Some(
            json!({"schema_version":REVIEW_VERSION,"content":content,
            "reading":review["reading"],"reviewer":review["reviewer"],"formation":formation}),
        ))
    }
}

/// The review recorded at `path`, unless there is none or another version of
/// ARP recorded it: that review lapses, and is not read past its version.
pub(super) fn recorded_review(path: &Path) -> Result<Option<Value>> {
    if !path.is_file() {
        return Ok(None);
    }
    let review = read(path, None)?;
    if review["schema_version"] != REVIEW_VERSION {
        return Ok(None);
    }
    validate("review", &review)?;
    Ok(Some(review))
}
