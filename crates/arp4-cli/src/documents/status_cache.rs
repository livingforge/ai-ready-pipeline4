//! Status rows kept between calls. A row is what inspecting a document or
//! proposal in full gives; it is reused while every byte it was made from is
//! the same: the files of the document, its original and region images, the
//! adopted document a proposal is based on, the configuration and the
//! program. Asking again, or paging through the status of many documents,
//! then hashes their files instead of inspecting each again. Like the search
//! index, it compares whole files, never sizes or modification times alone.
use super::*;

/// The form of the saved rows; another form is not read.
const VERSION: u64 = 1;

/// What every row's stamp depends on besides its own files.
struct Context {
    program: String,
    config: String,
    require_reviewed: bool,
}

impl Store {
    /// The status rows of `dirs`, each a document or proposal with its ID,
    /// from the saved rows where their stamps match and by [`Store::state`]
    /// where not.
    pub(super) fn states(
        &self,
        dirs: &[(String, PathBuf)],
        require_reviewed: bool,
    ) -> Result<Vec<Value>> {
        let path = under(&self.arp, "cache/status/rows.json")?;
        let mut saved = saved_rows(&path);
        let context = self.status_context(require_reviewed)?;
        let stamps = self.status_stamps(dirs, &context);
        let mut buffer = vec![0; search::READ_BUFFER_SIZE];
        let mut changed = false;
        let mut rows = Vec::with_capacity(dirs.len());
        for ((id, dir), stamp) in dirs.iter().zip(stamps) {
            let key = dir.to_string_lossy().into_owned();
            if let Some(stamp) = &stamp
                && let Some(entry) = saved.get(&key)
                && entry["stamp"] == *stamp
            {
                rows.push(entry["row"].clone());
                continue;
            }
            let row = self.state(id, dir, require_reviewed);
            // Kept only when nothing changed while inspecting.
            if let Some(stamp) = stamp
                && self
                    .status_stamp(id, dir, &context, &mut buffer)
                    .is_ok_and(|after| after == stamp)
            {
                saved.insert(key, json!({"stamp":stamp,"row":row}));
                changed = true;
            }
            rows.push(row);
        }
        // Rows of documents and proposals that are gone go with them.
        let kept = saved.len();
        saved.retain(|key, _| Path::new(key).is_dir());
        changed |= saved.len() != kept;
        if changed {
            // A status that cannot save its rows still answers; the next
            // inspects again.
            save_rows(&path, &saved);
        }
        Ok(rows)
    }

    /// Saves `row`, the status of the document or proposal `id` in `dir` that
    /// an edit made from the inspection of the content it wrote, so that the
    /// status reported after the edit does not inspect it again. Best effort:
    /// a row not saved is made by inspecting.
    pub(super) fn remember_state(&self, id: &str, dir: &Path, row: &Value) {
        let remembered = (|| -> Result<()> {
            let context = self.status_context(false)?;
            let mut buffer = vec![0; search::READ_BUFFER_SIZE];
            let stamp = self.status_stamp(id, dir, &context, &mut buffer)?;
            let path = under(&self.arp, "cache/status/rows.json")?;
            let mut saved = saved_rows(&path);
            saved.insert(
                dir.to_string_lossy().into_owned(),
                json!({"stamp":stamp,"row":row}),
            );
            save_rows(&path, &saved);
            Ok(())
        })();
        drop(remembered);
    }

    fn status_context(&self, require_reviewed: bool) -> Result<Context> {
        Ok(Context {
            program: crate::data::program()?,
            config: hash(&fs::read(under(&self.arp, "config.yml")?)?),
            require_reviewed,
        })
    }

    /// The stamps of `dirs`, hashed on up to four threads; none for a
    /// document whose files cannot be read, which is inspected every time.
    fn status_stamps(&self, dirs: &[(String, PathBuf)], context: &Context) -> Vec<Option<String>> {
        if dirs.is_empty() {
            return vec![];
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4)
            .min(dirs.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = dirs
                .chunks(dirs.len().div_ceil(workers))
                .map(|chunk| {
                    scope.spawn(move || {
                        let mut buffer = vec![0; search::READ_BUFFER_SIZE];
                        chunk
                            .iter()
                            .map(|(id, dir)| self.status_stamp(id, dir, context, &mut buffer).ok())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap_or_default())
                .collect()
        })
    }

    /// The hash of everything the status row of the document or proposal
    /// `id` in `dir` is made from.
    fn status_stamp(
        &self,
        id: &str,
        dir: &Path,
        context: &Context,
        buffer: &mut [u8],
    ) -> Result<String> {
        let own = self.document_stamp(dir, &context.config, buffer)?;
        // A proposal is checked against the adopted document it is based on.
        let adopted = if dir.join("proposal.json").is_file() {
            self.document(id)
                .ok()
                .filter(|adopted| adopted.is_dir())
                .map(|adopted| self.document_stamp(&adopted, &context.config, buffer))
                .transpose()?
        } else {
            None
        };
        Ok(hash(&encoded(&json!({"program":context.program,
            "require_reviewed":context.require_reviewed,"directory":dir,
            "own":own,"adopted":adopted}))))
    }
}

/// The rows saved at `path`; none when there are none of this form.
fn saved_rows(path: &Path) -> BTreeMap<String, Value> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|cache| cache["version"] == VERSION)
        .and_then(|mut cache| serde_json::from_value(cache["rows"].take()).ok())
        .unwrap_or_default()
}

fn save_rows(path: &Path, rows: &BTreeMap<String, Value>) {
    if let Ok(bytes) = serde_json::to_vec(&json!({"version":VERSION,"rows":rows})) {
        let _ = crate::data::replace_cache(path, &bytes);
    }
}
