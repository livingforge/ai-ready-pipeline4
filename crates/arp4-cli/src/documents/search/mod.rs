//! Local, derived full-text index. It never substitutes edited YAML for source
//! evidence. Explicit refresh checks canonical bytes and reindexes changed
//! documents; search reads the last saved snapshot.
mod analyzer;
mod corpus;
mod stamp;
mod timing;
use std::time::Instant;
pub use timing::SearchTimings;
use timing::Timer;

use super::*;
use analyzer::{Query, dictionary, grams, identifiers, normalize, words};
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Default)]
pub struct SearchOptions<'a> {
    pub document: Option<&'a str>,
    pub synonyms: Option<&'a Path>,
    pub offset: usize,
    pub limit: Option<usize>,
    pub revision: Option<&'a str>,
}

pub struct BatchSearchResult {
    pub timings: SearchTimings,
    pub results: Vec<SearchResult>,
    pub revision: String,
}

pub struct SearchResult {
    pub timings: SearchTimings,
    pub items: Vec<Value>,
    pub total: usize,
    pub revision: String,
    pub failed: Vec<Value>,
    pub indexed_documents: usize,
    pub refreshed_documents: usize,
    pub indexed_at_unix: u64,
    pub query_terms: Vec<Vec<String>>,
}

pub struct SearchRefreshResult {
    pub indexed_documents: usize,
    pub refreshed_documents: usize,
    pub failed: Vec<Value>,
    pub indexed_at_unix: u64,
}

const SQL: &str = include_str!("../../../../../contracts/document-search.sql");

fn signature() -> String {
    hash(
        format!(
            "{SQL}{}{}{}{}{}",
            include_str!("mod.rs"),
            include_str!("corpus.rs"),
            include_str!("analyzer.rs"),
            include_str!("stamp.rs"),
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    )
}

fn delete_document(db: &Connection, id: &str) -> Result<()> {
    db.execute(
        "DELETE FROM terms WHERE rowid IN (SELECT id FROM passages WHERE document = ?1)",
        [id],
    )?;
    db.execute("DELETE FROM passages WHERE document = ?1", [id])?;
    db.execute("DELETE FROM documents WHERE id = ?1", [id])?;
    Ok(())
}

impl Store {
    pub fn search(&self, text: &str, options: SearchOptions<'_>) -> Result<SearchResult> {
        let batch = self.search_many(&[text], options)?;
        let mut result = batch.results.into_iter().next().unwrap();
        result.timings = batch.timings;
        Ok(result)
    }

    /// Search every query against the same saved index transaction.
    pub fn search_many(
        &self,
        texts: &[&str],
        options: SearchOptions<'_>,
    ) -> Result<BatchSearchResult> {
        self.search_many_internal(texts, options, false, false)
    }

    /// Explicitly reconcile canonical files with the disposable index.
    pub fn refresh_search_index(&self, rebuild: bool) -> Result<SearchRefreshResult> {
        let result = self.search_many_internal(
            &["index"],
            SearchOptions {
                limit: Some(0),
                ..Default::default()
            },
            true,
            rebuild,
        )?;
        let search = result.results.into_iter().next().unwrap();
        Ok(SearchRefreshResult {
            indexed_documents: search.indexed_documents,
            refreshed_documents: search.refreshed_documents,
            failed: search.failed,
            indexed_at_unix: search.indexed_at_unix,
        })
    }

    fn search_many_internal(
        &self,
        texts: &[&str],
        options: SearchOptions<'_>,
        refresh: bool,
        rebuild: bool,
    ) -> Result<BatchSearchResult> {
        ensure!(
            !texts.is_empty() && texts.len() <= 32,
            "provide between 1 and 32 queries"
        );
        let started = Instant::now();
        let mut timings = SearchTimings::default();
        let default_dictionary = under(&self.arp, "search-synonyms.yml")?;
        let synonyms = if refresh {
            None
        } else {
            options.synonyms.or_else(|| {
                default_dictionary
                    .is_file()
                    .then_some(default_dictionary.as_path())
            })
        };
        let dictionary = dictionary(synonyms)?;
        let queries = texts
            .iter()
            .map(|text| Query::new(text, &dictionary))
            .collect::<Result<Vec<_>>>()?;
        let path = under(&self.arp, "cache/search/index.sqlite")?;
        for suffix in ["-journal", "-wal", "-shm"] {
            under(&self.arp, &format!("cache/search/index.sqlite{suffix}"))?;
        }
        if refresh && rebuild && path.exists() {
            fs::remove_file(&path)?;
            remove_empty_directories(path.parent().unwrap(), &self.arp);
        }
        if refresh {
            fs::create_dir_all(path.parent().unwrap())?;
        }
        let mut db = if refresh {
            Connection::open(&path).context("open search index")?
        } else {
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .context("search index unavailable; run documents search-refresh")?
        };
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let signature = signature();
        if refresh {
            db.execute_batch(
                "PRAGMA journal_mode=DELETE; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE;",
            )?;
            let tx = db.transaction()?;
            let saved: Option<String> = tx
                .query_row("SELECT signature FROM metadata", [], |r| r.get(0))
                .optional()
                .unwrap_or(None);
            if saved.as_deref() != Some(&signature) {
                tx.execute_batch("DROP TABLE IF EXISTS terms; DROP TABLE IF EXISTS passages; DROP TABLE IF EXISTS documents; DROP TABLE IF EXISTS metadata;")?;
                tx.execute_batch(SQL)?;
                tx.execute("INSERT INTO metadata VALUES (?1,'[]',0,'',0)", [&signature])?;
            }
            tx.commit()?;
            if saved.is_some() && saved.as_deref() != Some(&signature) {
                db.execute_batch("VACUUM")?;
            }
        }
        let tx = db.transaction()?;
        timings.setup = started.elapsed();
        let integrity_started = Instant::now();
        if let Some(scope) = options.document {
            document_id(scope)?;
        }
        let selected = |id: &str| {
            options.document.is_none_or(|scope| {
                id == scope || id.strip_prefix(scope).is_some_and(|s| s.starts_with('/'))
            })
        };
        let (corpus_revision, indexed_documents, failed, refreshed, indexed_at_unix, config_hash) =
            if refresh {
                let base = under(&self.arp, "documents")?;
                let config_hash = hash(&fs::read(under(&self.arp, "config.yml")?)?);
                let ids = self.ids("documents", None)?;
                let live: BTreeSet<_> = ids.iter().collect();
                if let Some(scope) = options.document {
                    // Validate scope even when the corpus is empty.
                    under(&under(&self.arp, "documents")?, scope)?;
                }
                let cached = tx
                    .prepare("SELECT id,fingerprint,refs FROM documents")?
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                        ))
                    })?
                    .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
                for id in cached.keys() {
                    if !live.contains(id) {
                        let _timer = Timer::new(&mut timings.index_update);
                        delete_document(&tx, id)?;
                    }
                }
                let mut failed = vec![];
                let mut refreshed = 0;
                let mut stamps = BTreeMap::new();
                // BM25 uses corpus-wide frequencies. Refresh the same complete corpus for
                // every scope so ranking never depends on which folder was searched first.
                let checked = self.search_stamps(&ids, &base, &config_hash, &cached)?;
                let mut read_buffer = vec![0; stamp::READ_BUFFER_SIZE];
                for (id, checked) in ids.iter().zip(checked) {
                    let result = (|| -> Result<()> {
                        // IDs were enumerated from actual directory entries. Do not repeat
                        // the sibling-name scan used when accepting a user-supplied ID.
                        let (stamp, references) = checked?;
                        if cached.get(id).map(|(stamp, _)| stamp) != Some(&stamp) {
                            // Revalidate paths before reading a changed document for indexing.
                            let dir = under(&base, id)?;
                            let inspected = self.inspect(&dir, false)?;
                            let (formation, _) = self.formation(&dir)?;
                            ensure!(
                                formation["document_id"] == *id
                                    && formation["extraction"] == inspected.meta["extraction"],
                                "formation identity mismatch"
                            );
                            let (passages, state) = {
                                let _timer = Timer::new(&mut timings.index_update);
                                (corpus::build(self, &inspected)?, search_state(&inspected)?)
                            };
                            ensure!(
                                self.search_stamp(&dir, &config_hash, None, &mut read_buffer)?
                                    .0
                                    == stamp,
                                "document changed while indexing; retry search-refresh"
                            );
                            let _timer = Timer::new(&mut timings.index_update);
                            delete_document(&tx, id)?;
                            let mut repetition_groups = BTreeMap::<(String, String), i64>::new();
                            for passage in passages {
                                let title = normalize(string(&passage["title"])?);
                                let body = normalize(string(&passage["text"])?);
                                let raw_context = array(&passage["context"])?
                                    .iter()
                                    .map(|s| s["text"].as_str().unwrap_or(""))
                                    .collect::<Vec<_>>()
                                    .join(" | ");
                                let context = normalize(&raw_context);
                                let next_group = i64::try_from(repetition_groups.len() + 1)?;
                                let repetition_group = *repetition_groups
                                    .entry((body.clone(), context.clone()))
                                    .or_insert(next_group);
                                let identifiers = identifiers(string(&passage["text"])?);
                                tx.execute("INSERT INTO passages(document,payload,title,body,context,repetition_group,identifiers) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                            params![id, passage.to_string(), title, body, context, repetition_group, identifiers])?;
                                tx.execute("INSERT INTO terms(rowid,title,body,context,grams) VALUES (?1,?2,?3,?4,?5)",
                            params![tx.last_insert_rowid(), words(&title), words(&body), words(&context), grams(&format!("{title}\n{body}\n{context}"))])?;
                            }
                            tx.execute(
                                "INSERT INTO documents VALUES (?1,?2,?3,?4)",
                                params![
                                    id,
                                    stamp,
                                    state.to_string(),
                                    serde_json::to_string(&references)?
                                ],
                            )?;
                            refreshed += 1;
                        }
                        stamps.insert(id.clone(), stamp);
                        Ok(())
                    })();
                    if let Err(error) = result {
                        // Never return old cached hits for a now-invalid document.
                        let _timer = Timer::new(&mut timings.index_update);
                        delete_document(&tx, id)?;
                        failed.push(json!({"document_id":id,"reason":format!("{error:#}")}));
                    }
                }
                let indexed_at_unix = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();
                let corpus_revision = hash(&encoded(
                    &json!({"index":signature,"documents":stamps,"failed":failed}),
                ));
                tx.execute(
                "UPDATE metadata SET failed=?1,refreshed_unix=?2,corpus_revision=?3,indexed_documents=?4",
                params![serde_json::to_string(&failed)?, indexed_at_unix, corpus_revision, stamps.len()],
            )?;
                (
                    corpus_revision,
                    stamps.len(),
                    failed,
                    refreshed,
                    indexed_at_unix,
                    Some(config_hash),
                )
            } else {
                let (saved, failed_json, indexed_at_unix, corpus_revision, indexed_documents): (String, String, u64, String, usize) = tx
                .query_row(
                    "SELECT signature,failed,refreshed_unix,corpus_revision,indexed_documents FROM metadata",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .context("invalid search index; run documents search-refresh")?;
                ensure!(
                    saved == signature,
                    "search index version changed; run documents search-refresh"
                );
                let failed: Vec<Value> = serde_json::from_str(&failed_json)?;
                (
                    corpus_revision,
                    indexed_documents,
                    failed,
                    0,
                    indexed_at_unix,
                    None,
                )
            };
        timings.integrity = integrity_started
            .elapsed()
            .saturating_sub(timings.index_update);
        let mut results: Vec<SearchResult> = Vec::with_capacity(texts.len());
        let mut query_cache: BTreeMap<_, usize> = BTreeMap::new();
        for (text, query) in texts.iter().zip(queries) {
            let revision_started = Instant::now();
            let revision = hash(&encoded(
                &json!({"corpus":corpus_revision,"query":text,"scope":options.document,"terms":query.groups}),
            ));
            timings.integrity += revision_started.elapsed();
            let mut sql_started = Instant::now();
            let scope = options.document.unwrap_or("");
            let filter = "FROM terms JOIN passages p ON p.id=terms.rowid WHERE terms MATCH ?1 AND (?2='' OR p.document=?2 OR substr(p.document,1,length(?2)+1)=?2||'/')";
            let cache_key = (
                query.expression.clone(),
                query.identifier.clone(),
                query.normalized.clone(),
            );
            if let Some(&index) = query_cache.get(&cache_key) {
                let items = results[index].items.clone();
                let total = results[index].total;
                results.push(SearchResult {
                    timings: SearchTimings::default(),
                    items,
                    total,
                    revision,
                    failed: failed
                        .iter()
                        .filter(|failure| selected(failure["document_id"].as_str().unwrap()))
                        .cloned()
                        .collect(),
                    indexed_documents,
                    refreshed_documents: refreshed,
                    indexed_at_unix,
                    query_terms: query.groups,
                });
                continue;
            }
            // Materialize BM25 while the FTS cursor is active, before window ranking.
            // Count the same candidates for a nonempty page instead of running MATCH twice.
            // Repeated text with the same header text is deferred, never discarded:
            // different sheets/positions remain addressable through normal paging.
            let sql = format!(
                "WITH matches AS MATERIALIZED (
            SELECT p.id,p.document,p.repetition_group,bm25(terms,3.0,1.0,1.5,0.15) AS cost,
                instr(p.identifiers,?3)>0 AS exact, instr(p.body,?4)>0 AS phrase,
                instr(p.title,?4)>0 AS heading {filter}
            ), ranked AS (
                SELECT *,row_number() OVER (
                    PARTITION BY document,repetition_group
                    ORDER BY exact DESC,phrase DESC,heading DESC,cost,id
                ) AS repetition FROM matches
            ) SELECT p.payload,d.state,-r.cost,r.exact,r.phrase,r.heading,
                     (SELECT count(*) FROM matches)
            FROM ranked r JOIN passages p ON p.id=r.id JOIN documents d ON d.id=r.document
            ORDER BY r.exact DESC,r.phrase DESC,r.heading DESC,r.repetition,r.cost,r.document,r.id
            LIMIT ?5 OFFSET ?6"
            );
            let limit = options.limit.map(i64::try_from).transpose()?.unwrap_or(-1);
            let mut statement = tx.prepare(&sql)?;
            let mut rows = statement.query(params![
                query.expression,
                scope,
                query.identifier,
                query.normalized,
                limit,
                i64::try_from(options.offset)?
            ])?;
            let mut items = vec![];
            let mut total = None;
            loop {
                let row = rows.next()?;
                timings.sql += sql_started.elapsed();
                let Some(row) = row else { break };
                if total.is_none() {
                    total = Some(row.get::<_, usize>(6)?);
                }
                let response_started = Instant::now();
                let mut item: Value = serde_json::from_str(&row.get::<_, String>(0)?)?;
                item["state"] = serde_json::from_str(&row.get::<_, String>(1)?)?;
                item["score"] = json!(row.get::<_, f64>(2)?);
                item["match"] = json!({"exact_identifier":row.get::<_, bool>(3)?,"phrase_in_body":row.get::<_, bool>(4)?,"phrase_in_title":row.get::<_, bool>(5)?});
                item["extraction_path"] = json!(format!(
                    ".arp/documents/{}/extraction.json",
                    string(&item["document_id"])?
                ));
                items.push(item);
                timings.response += response_started.elapsed();
                sql_started = Instant::now();
            }
            drop(rows);
            drop(statement);
            // An empty page (including an offset beyond the end) still needs its
            // exact total. This path is uncommon and keeps normal pages to one MATCH.
            let total = if let Some(total) = total {
                total
            } else {
                let count_started = Instant::now();
                let total = tx.query_row(
                    &format!("SELECT count(*) {filter}"),
                    params![query.expression, scope],
                    |r| r.get(0),
                )?;
                timings.sql += count_started.elapsed();
                total
            };
            query_cache.insert(cache_key, results.len());
            results.push(SearchResult {
                timings: SearchTimings::default(),
                items,
                total,
                revision,
                failed: failed
                    .iter()
                    .filter(|failure| selected(failure["document_id"].as_str().unwrap()))
                    .cloned()
                    .collect(),
                indexed_documents,
                refreshed_documents: refreshed,
                indexed_at_unix,
                query_terms: query.groups,
            });
        }
        let integrity_started = Instant::now();
        let revision = if results.len() == 1 {
            results[0].revision.clone()
        } else {
            hash(&encoded(&json!(
                results.iter().map(|r| &r.revision).collect::<Vec<_>>()
            )))
        };
        if let Some(expected) = options.revision {
            ensure!(
                expected == revision,
                "search revision changed; restart at offset 0 without --revision"
            );
        }
        if let Some(config_hash) = config_hash {
            ensure!(
                hash(&fs::read(under(&self.arp, "config.yml")?)?) == config_hash,
                "project configuration changed while refreshing search index; retry"
            );
        }
        timings.integrity += integrity_started.elapsed();
        {
            let _timer = Timer::new(&mut timings.index_update);
            tx.commit()?;
        }
        Ok(BatchSearchResult {
            timings,
            results,
            revision,
        })
    }
}

fn search_state(inspected: &Inspection) -> Result<Value> {
    let mut originals = BTreeMap::new();
    for sheet in array(&inspected.extraction["sheets"])? {
        for cell in array(&sheet["cells"])? {
            originals.insert(
                (string(&sheet["name"])?, string(&cell["address"])?),
                &cell["value"],
            );
        }
    }
    let mut edited = !array(&inspected.mappings["operations"])?.is_empty();
    for entry in array(&inspected.mappings["entries"])? {
        if entry["writeback"] == "cell"
            && let Some(MappingTarget::Cell { sheet, cell }) = mapping_target(&entry["target"])?
            && let Some(value) = inspected.values.get(&key(entry)?)
        {
            edited |= originals
                .get(&(sheet, cell))
                .is_some_and(|old| **old != *value);
        }
    }
    Ok(
        json!({"source_sha256":inspected.meta["source"]["sha256"],"source_current":inspected.source_current,
        "source_missing":inspected.source_missing,"reviewed":inspected.reviewed,"content_differs_from_extraction":edited,
        "content_sha256":inspected.fingerprint}),
    )
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_batch_revision_hashes() {
        let stamps: BTreeMap<_, _> = (0..5000)
            .map(|i| (format!("doc/{i:05}"), format!("{i:064x}")))
            .collect();
        let failed: Vec<Value> = vec![];
        let mut old = Vec::new();
        let mut new = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for i in 0..32 {
                black_box(hash(&encoded(
                    &json!({"index":"signature","documents":stamps,"failed":failed,
                    "query":format!("term{i}"),"scope":null,"terms":[[format!("term{i}")]]}),
                )));
            }
            old.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let corpus = hash(&encoded(
                &json!({"index":"signature","documents":stamps,"failed":failed}),
            ));
            for i in 0..32 {
                black_box(hash(&encoded(
                    &json!({"corpus":corpus,"query":format!("term{i}"),
                    "scope":null,"terms":[[format!("term{i}")]]}),
                )));
            }
            new.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        eprintln!(
            "revision_hashes_old_ms={:.2} shared_ms={:.2}",
            old[2], new[2]
        );
    }
}
