//! Local, derived full-text index. It never substitutes edited YAML for source
//! evidence. Explicit refresh checks canonical bytes and reindexes changed
//! documents; search reads the last saved snapshot.
mod analyzer;
mod corpus;
mod format;
mod ranking;
mod stamp;
mod timing;
pub use format::FormatFilter;
pub(super) use stamp::READ_BUFFER_SIZE;
use std::io::{Read, Write};
use std::time::Instant;
pub use timing::SearchTimings;
use timing::Timer;

use super::*;
use analyzer::{Query, dictionary, grams, identifiers, normalize, words};
use ranking::Ranked;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Default)]
pub struct SearchOptions<'a> {
    pub document: Option<&'a str>,
    pub synonyms: Option<&'a Path>,
    pub offset: usize,
    pub limit: Option<usize>,
    pub revision: Option<&'a str>,
    /// Only passages with text in this format; with it, an empty query
    /// searches by format alone.
    pub format: FormatFilter,
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

/// The formats of the indexed text, as [`Store::search_formats`] lists them.
pub struct FormatInventory {
    pub items: Vec<Value>,
    pub failed: Vec<Value>,
    pub indexed_documents: usize,
    pub indexed_at_unix: u64,
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
            "{SQL}{}{}{}{}{}{}{}{}",
            include_str!("mod.rs"),
            include_str!("corpus.rs"),
            include_str!("analyzer.rs"),
            include_str!("format.rs"),
            include_str!("ranking.rs"),
            include_str!("stamp.rs"),
            include_str!("../../excel/number_format.rs"),
            env!("CARGO_PKG_VERSION")
        )
        .as_bytes(),
    )
}

/// A passage as stored: JSON, deflate-compressed. A table row repeats its
/// keys, sheet and headers for every cell, which compresses several times over.
/// The JSON is compressed whole: written straight into the encoder, it went
/// through the compressor a few bytes at a time.
fn pack(passage: &Value) -> Result<Vec<u8>> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(&serde_json::to_vec(passage)?)?;
    Ok(encoder.finish()?)
}

fn unpack(stored: &[u8]) -> Result<Value> {
    let mut json = vec![];
    flate2::read::DeflateDecoder::new(stored).read_to_end(&mut json)?;
    Ok(serde_json::from_slice(&json)?)
}

/// Whether the document `id` is in `scope`, a document or folder ID; without
/// a scope every document is.
fn within(id: &str, scope: Option<&str>) -> bool {
    scope.is_none_or(|scope| {
        id == scope || id.strip_prefix(scope).is_some_and(|s| s.starts_with('/'))
    })
}

/// SQL that holds when passage `p` is in the scope bound to `param`, as
/// [`within`] decides it; an empty scope holds every passage.
fn in_scope(param: &str) -> String {
    format!(
        "({param}='' OR p.document={param} OR substr(p.document,1,length({param})+1)={param}||'/')"
    )
}

/// The metadata of a saved index this version made: its corpus revision,
/// failed documents, last refresh and number of documents.
fn snapshot(db: &Connection, signature: &str) -> Result<(String, Vec<Value>, u64, usize)> {
    let (saved, failed, indexed_at_unix, corpus_revision, indexed_documents): (
        String,
        String,
        u64,
        String,
        usize,
    ) = db
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
    Ok((
        corpus_revision,
        serde_json::from_str(&failed)?,
        indexed_at_unix,
        indexed_documents,
    ))
}

/// Moves the format of each source of `passage` into the `formats` table,
/// leaving its ID in its place, and gives the cells of the passage in each
/// format. A source continuing a value from an earlier fragment is counted
/// with that fragment.
fn store_formats(
    db: &Connection,
    known: &mut HashMap<String, i64>,
    passage: &mut Value,
) -> Result<BTreeMap<i64, i64>> {
    let mut cells = BTreeMap::new();
    for source in passage["sources"].as_array_mut().into_iter().flatten() {
        let Some(format) = source.as_object_mut().and_then(|s| s.remove("format")) else {
            continue;
        };
        let key = format.to_string();
        let id = match known.get(&key) {
            Some(id) => *id,
            None => {
                db.execute("INSERT OR IGNORE INTO formats(format) VALUES (?1)", [&key])?;
                let id = db.query_row("SELECT id FROM formats WHERE format=?1", [&key], |r| {
                    r.get(0)
                })?;
                known.insert(key, id);
                id
            }
        };
        source["format"] = json!(id);
        *cells.entry(id).or_default() += i64::from(source["start"] == 0);
    }
    Ok(cells)
}

/// Takes the format IDs out of a hit's sources and, when the search filters
/// by format, lists the sources in a `wanted` format as `formatted`: a cell
/// by its address, a shape's text by its pointer.
fn mark_formats(item: &mut Value, wanted: Option<&BTreeSet<i64>>) {
    let mut formatted: Vec<Value> = vec![];
    for source in item["sources"].as_array_mut().into_iter().flatten() {
        let Some(id) = source.as_object_mut().and_then(|s| s.remove("format")) else {
            continue;
        };
        if wanted.is_some_and(|wanted| id.as_i64().is_some_and(|id| wanted.contains(&id))) {
            let place = match source["address"].as_str() {
                Some(address) if !address.is_empty() => json!(address),
                _ => source["pointer"].clone(),
            };
            if !formatted.contains(&place) {
                formatted.push(place);
            }
        }
    }
    if wanted.is_some() {
        item["formatted"] = json!(formatted);
    }
}

/// The SQL ranking the hits of a search with a query (`query`) or by format
/// alone, and the SQL counting them. Each ranked row starts with the columns
/// [`Ranked::read`] reads, then the number of hits; a `paged` ranking adds
/// the passage and its document's state and stops at the limit.
///
/// Parameters: with a query, the FTS expression (1), the scope (2), the
/// wanted format IDs as JSON or null (3), the query as an identifier (4) and
/// normalized (5) and its terms as JSON (6); by format alone, the format IDs
/// (1) and the scope (2). A paged ranking takes the limit last. The count
/// takes the first three with a query, the first two without.
fn ranking_sql(query: bool, paged: bool) -> (String, String) {
    let (page, limit) = if paged {
        (
            ",p.payload,d.state",
            if query { " LIMIT ?7" } else { " LIMIT ?3" },
        )
    } else {
        ("", "")
    };
    if query {
        let filter = format!(
            "FROM terms JOIN passages p ON p.id=terms.rowid WHERE terms MATCH ?1 AND {} AND (?3 IS NULL OR EXISTS (SELECT 1 FROM passage_formats f WHERE f.passage=p.id AND f.format IN (SELECT value FROM json_each(?3))))",
            in_scope("?2")
        );
        let joined = if paged {
            " JOIN passages p ON p.id=r.id JOIN documents d ON d.id=r.document"
        } else {
            ""
        };
        // Materialize BM25 while the FTS cursor is active, before window ranking.
        // Count the same candidates for a nonempty page instead of running MATCH twice.
        // Repeated text with the same header text is deferred, never discarded:
        // different sheets/positions remain addressable through normal paging.
        // Hits whose body holds no query term matched only their headings;
        // they follow the body hits, one per heading group before the rest.
        (
            format!(
                "WITH matches AS MATERIALIZED (
            SELECT p.id,p.document,p.repetition_group,p.heading_group,
                bm25(terms,3.0,1.0,1.5,0.15) AS cost,
                instr(p.identifiers,?4)>0 AS exact, instr(p.body,?5)>0 AS phrase,
                instr(p.title,?5)>0 AS heading,
                NOT EXISTS (SELECT 1 FROM json_each(?6) WHERE instr(p.body,value)>0) AS headed
                {filter}
            ), ranked AS (
                SELECT *,row_number() OVER (
                    PARTITION BY document,repetition_group
                    ORDER BY exact DESC,phrase DESC,heading DESC,cost,id
                ) AS repetition,
                row_number() OVER (
                    PARTITION BY headed,document,heading_group
                    ORDER BY exact DESC,phrase DESC,heading DESC,cost,id
                ) AS deferral,
                count(*) OVER (PARTITION BY headed,document,heading_group) AS group_hits
                FROM matches
            ) SELECT r.id,-r.cost,r.exact,r.phrase,r.heading,r.headed,r.group_hits,
                     (SELECT count(*) FROM matches){page}
            FROM ranked r{joined}
            ORDER BY r.headed,CASE WHEN r.headed THEN r.deferral ELSE 1 END,
                r.exact DESC,r.phrase DESC,r.heading DESC,r.repetition,r.cost,r.document,r.id{limit}"
            ),
            format!("SELECT count(*) {filter}"),
        )
    } else {
        // Without a query nothing ranks the hits: they keep the order of the
        // documents and of each document's passages.
        let filter = format!(
            "FROM passage_formats f JOIN passages p ON p.id=f.passage WHERE f.format IN (SELECT value FROM json_each(?1)) AND {}",
            in_scope("?2")
        );
        let joined = if paged {
            " JOIN passages p ON p.id=m.id JOIN documents d ON d.id=m.document"
        } else {
            ""
        };
        (
            format!(
                "WITH matches AS MATERIALIZED (SELECT DISTINCT p.id,p.document {filter})
            SELECT m.id,NULL,0,0,0,0,0,(SELECT count(*) FROM matches){page}
            FROM matches m{joined} ORDER BY m.document,m.id{limit}"
            ),
            format!("SELECT count(*) FROM (SELECT DISTINCT p.id {filter})"),
        )
    }
}

fn delete_document(db: &Connection, id: &str) -> Result<()> {
    db.execute(
        "DELETE FROM passage_formats WHERE passage IN (SELECT id FROM passages WHERE document = ?1)",
        [id],
    )?;
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

    /// The hash of every file the document in `dir` is made from: its own,
    /// its original and region images, and the configuration (`config_hash`).
    pub(super) fn document_stamp(
        &self,
        dir: &Path,
        config_hash: &str,
        buffer: &mut [u8],
    ) -> Result<String> {
        Ok(self.search_stamp(dir, config_hash, None, buffer)?.0)
    }

    /// Each font color, theme color, font name, size and emphasis of the text
    /// indexed in `scope` (every document for none), with the hits a search
    /// filtering by it alone returns and the cells in it.
    pub fn search_formats(&self, scope: Option<&str>) -> Result<FormatInventory> {
        if let Some(scope) = scope {
            document_id(scope)?;
        }
        let path = under(&self.arp, "cache/search/index.sqlite")?;
        let mut db = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("search index unavailable; run documents search-refresh")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let tx = db.transaction()?;
        let (_, failed, indexed_at_unix, indexed_documents) = snapshot(&tx, &signature())?;
        let formats = tx
            .prepare("SELECT id,format FROM formats")?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .map(|row| {
                let (id, format) = row?;
                Ok((id, serde_json::from_str::<Value>(&format)?))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let rows = tx
            .prepare(&format!(
                "SELECT f.passage,f.format,f.cells FROM passage_formats f JOIN passages p ON p.id=f.passage WHERE {}",
                in_scope("?1")
            ))?
            .query_map([scope.unwrap_or("")], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })?
            .map(|row| {
                let (passage, format, cells) = row?;
                let format = formats
                    .get(&format)
                    .with_context(|| format!("search index lacks format {format}"))?;
                Ok((passage, format, cells))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(FormatInventory {
            items: format::inventory(rows),
            failed: failed
                .into_iter()
                .filter(|failure| within(failure["document_id"].as_str().unwrap_or(""), scope))
                .collect(),
            indexed_documents,
            indexed_at_unix,
        })
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
            .map(|text| {
                if !options.format.is_empty() && normalize(text).is_empty() {
                    Ok(None)
                } else {
                    Query::new(text, &dictionary).map(Some)
                }
            })
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
            // A table row of a few KiB leaves most of a 4 KiB page empty; 16 KiB
            // pages hold several. It applies to a new index file, and to an older
            // one when the VACUUM below rebuilds it for a new signature.
            db.execute_batch(
                "PRAGMA page_size=16384; PRAGMA journal_mode=DELETE; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE;",
            )?;
            let tx = db.transaction()?;
            let saved: Option<String> = tx
                .query_row("SELECT signature FROM metadata", [], |r| r.get(0))
                .optional()
                .unwrap_or(None);
            if saved.as_deref() != Some(&signature) {
                tx.execute_batch("DROP TABLE IF EXISTS terms; DROP TABLE IF EXISTS passage_formats; DROP TABLE IF EXISTS formats; DROP TABLE IF EXISTS passages; DROP TABLE IF EXISTS documents; DROP TABLE IF EXISTS metadata;")?;
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
        let selected = |id: &str| within(id, options.document);
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
                let mut known_formats = HashMap::new();
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
                            let (inspected, _) = self.inspect_records(&dir, false, false)?;
                            self.formation(&dir)?;
                            let _timer = Timer::new(&mut timings.index_update);
                            // Each passage is stored as it is made. A build or
                            // check failing below leaves the rows written so far,
                            // which the failure removes with the rest.
                            delete_document(&tx, id)?;
                            let mut repetition_groups = BTreeMap::<(String, String), i64>::new();
                            let mut heading_groups = BTreeMap::<(String, String), i64>::new();
                            corpus::build(self, &inspected, |mut passage| {
                                let formats = store_formats(&tx, &mut known_formats, &mut passage)?;
                                let title = normalize(string(&passage["title"])?);
                                let body = normalize(string(&passage["text"])?);
                                let context = normalize(&corpus::headings(&passage));
                                let next_group = i64::try_from(repetition_groups.len() + 1)?;
                                let repetition_group = *repetition_groups
                                    .entry((body.clone(), context.clone()))
                                    .or_insert(next_group);
                                let next_group = i64::try_from(heading_groups.len() + 1)?;
                                let heading_group = *heading_groups
                                    .entry((title.clone(), context.clone()))
                                    .or_insert(next_group);
                                let identifiers = identifiers(string(&passage["text"])?);
                                tx.execute("INSERT INTO passages(document,payload,title,body,repetition_group,heading_group,identifiers) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                            params![id, pack(&passage)?, title, body, repetition_group, heading_group, identifiers])?;
                                let passage_id = tx.last_insert_rowid();
                                tx.execute("INSERT INTO terms(rowid,title,body,context,grams) VALUES (?1,?2,?3,?4,?5)",
                            params![passage_id, words(&title), words(&body), words(&context), grams(&format!("{title}\n{body}\n{context}"))])?;
                                for (format, cells) in formats {
                                    tx.execute(
                                        "INSERT INTO passage_formats(passage,format,cells) VALUES (?1,?2,?3)",
                                        params![passage_id, format, cells],
                                    )?;
                                }
                                Ok(())
                            })?;
                            let state = search_state(&inspected)?;
                            ensure!(
                                self.search_stamp(&dir, &config_hash, None, &mut read_buffer)?
                                    .0
                                    == stamp,
                                "document changed while indexing; retry search-refresh"
                            );
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
                {
                    // Formats no passage is in any more go, so that the table
                    // keeps the formats of the documents, not of their history.
                    let _timer = Timer::new(&mut timings.index_update);
                    tx.execute(
                        "DELETE FROM formats WHERE id NOT IN (SELECT format FROM passage_formats)",
                        [],
                    )?;
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
                let (corpus_revision, failed, indexed_at_unix, indexed_documents) =
                    snapshot(&tx, &signature)?;
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
        // The formats the filter keeps; the formats table holds few rows.
        let formats = if options.format.is_empty() {
            None
        } else {
            let started = Instant::now();
            let mut wanted = BTreeSet::new();
            let mut statement = tx.prepare("SELECT id,format FROM formats")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                if options
                    .format
                    .matches(&serde_json::from_str(&row.get::<_, String>(1)?)?)
                {
                    wanted.insert(row.get::<_, i64>(0)?);
                }
            }
            timings.sql += started.elapsed();
            Some(wanted)
        };
        let format_ids = formats.as_ref().map(serde_json::to_string).transpose()?;
        let scope = options.document.unwrap_or("");
        let rankings = under(&self.arp, "cache/search/rankings")?;
        let mut results: Vec<SearchResult> = Vec::with_capacity(texts.len());
        let mut query_cache: BTreeMap<_, usize> = BTreeMap::new();
        for (text, query) in texts.iter().zip(queries) {
            let revision_started = Instant::now();
            let terms = query
                .as_ref()
                .map(|query| query.groups.clone())
                .unwrap_or_default();
            let revision = hash(&encoded(
                &json!({"corpus":corpus_revision,"query":text,"scope":options.document,"terms":terms,
                    "format":options.format.describe()}),
            ));
            timings.integrity += revision_started.elapsed();
            let cache_key = query.as_ref().map(|query| {
                (
                    query.expression.clone(),
                    query.identifier.clone(),
                    query.normalized.clone(),
                )
            });
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
                    query_terms: terms,
                });
                continue;
            }
            let sql_started = Instant::now();
            let mut values: Vec<rusqlite::types::Value> = match &query {
                Some(query) => vec![
                    query.expression.clone().into(),
                    scope.to_owned().into(),
                    format_ids.clone().into(),
                    query.identifier.clone().into(),
                    query.normalized.clone().into(),
                    serde_json::to_string(&query.groups.concat())?.into(),
                ],
                None => vec![format_ids.clone().into(), scope.to_owned().into()],
            };
            let counted = if query.is_some() { 3 } else { 2 };
            let mut hits: Vec<(Ranked, Vec<u8>, String)> = vec![];
            let total = if options.offset == 0 {
                // The first page ranks with its passages, keeping only the
                // hits it shows.
                let (sql, count) = ranking_sql(query.is_some(), true);
                values.push(options.limit.map_or(-1, |limit| limit as i64).into());
                let mut statement = tx.prepare(&sql)?;
                let mut rows = statement.query(rusqlite::params_from_iter(&values))?;
                let mut total = None;
                while let Some(row) = rows.next()? {
                    total.get_or_insert(row.get::<_, usize>(7)?);
                    hits.push((Ranked::read(row)?, row.get(8)?, row.get(9)?));
                }
                drop(rows);
                drop(statement);
                // An empty page (including a limit of 0) still needs its exact
                // total. This path is uncommon and keeps normal pages to one MATCH.
                match total {
                    Some(total) => total,
                    None => tx.query_row(
                        &count,
                        rusqlite::params_from_iter(&values[..counted]),
                        |r| r.get(0),
                    )?,
                }
            } else {
                // A later page reads its hits from the ranking saved for this
                // revision, ranking every hit and saving them only when none is.
                let key = ranking::key(&revision, indexed_at_unix);
                let (total, page) =
                    match ranking::load(&rankings, &key, options.offset, options.limit) {
                        Some(saved) => saved,
                        None => {
                            let (sql, _) = ranking_sql(query.is_some(), false);
                            let ranked = tx
                                .prepare(&sql)?
                                .query_map(rusqlite::params_from_iter(&values), Ranked::read)?
                                .collect::<rusqlite::Result<Vec<_>>>()?;
                            ranking::save(&rankings, &key, &ranked);
                            let start = options.offset.min(ranked.len());
                            let end = options.limit.map_or(ranked.len(), |limit| {
                                start.saturating_add(limit).min(ranked.len())
                            });
                            (ranked.len(), ranked[start..end].to_vec())
                        }
                    };
                let ids: Vec<i64> = page.iter().map(|hit| hit.id).collect();
                let mut passages = tx
                    .prepare("SELECT p.id,p.payload,d.state FROM passages p JOIN documents d ON d.id=p.document WHERE p.id IN (SELECT value FROM json_each(?1))")?
                    .query_map([serde_json::to_string(&ids)?], |r| {
                        Ok((r.get::<_, i64>(0)?, (r.get::<_, Vec<u8>>(1)?, r.get::<_, String>(2)?)))
                    })?
                    .collect::<rusqlite::Result<HashMap<_, _>>>()?;
                for hit in page {
                    let (payload, state) = passages.remove(&hit.id).context(
                        "saved search ranking names a passage the index lacks; search again",
                    )?;
                    hits.push((hit, payload, state));
                }
                total
            };
            timings.sql += sql_started.elapsed();
            let response_started = Instant::now();
            let mut items = Vec::with_capacity(hits.len());
            for (hit, payload, state) in hits {
                let mut item = unpack(&payload)?;
                item["state"] = serde_json::from_str(&state)?;
                if let Some(score) = hit.score {
                    item["score"] = json!(score);
                    item["match"] = json!({"exact_identifier":hit.exact,"phrase_in_body":hit.phrase,"phrase_in_title":hit.heading});
                    if let Some(group_hits) = hit.group_hits {
                        item["heading_only"] = json!({"group_hits":group_hits});
                    }
                }
                mark_formats(&mut item, formats.as_ref());
                item["extraction_path"] = json!(format!(
                    ".arp/documents/{}/extraction.json",
                    string(&item["document_id"])?
                ));
                items.push(item);
            }
            timings.response += response_started.elapsed();
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
                query_terms: terms,
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
            if refresh {
                ranking::clear(&rankings, &self.arp)?;
            }
        }
        Ok(BatchSearchResult {
            timings,
            results,
            revision,
        })
    }
}

fn search_state(inspected: &Inspection) -> Result<Value> {
    let edited = inspected.content_edited;
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
