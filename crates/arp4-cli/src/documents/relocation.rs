//! Moving, renaming and deleting originals with the records kept of them.
//!
//! A document's ID is its original's path below the sources folder, and the
//! ID is written into more than the folders of its records: `document.yml`
//! and a proposal's `proposal.json` name it, the hashes the reviews of its
//! content and of its structure bind include it, and the registry names the
//! sources its records quote by hashes of it. A move changes all of them in
//! one step, keeping each review and approval of what is still the same, and
//! puts back what it changed when a step fails.
use super::import::extension;
use super::inspection::structure_format;
use super::*;
use crate::registry::{self, Moved};

/// One step a move took, which a failed move takes back.
enum Step {
    /// A folder made for a path moved into it.
    Made(PathBuf),
    /// A file or folder moved from the first path to the second.
    Moved(PathBuf, PathBuf),
    /// A folder emptied by moving what it held, then removed.
    Emptied(PathBuf),
    /// A file written over, with the bytes it held.
    Wrote(PathBuf, Vec<u8>),
}

/// The steps a move has taken, in order.
#[derive(Default)]
struct Journal(Vec<Step>);

impl Journal {
    /// Makes the folders missing above `path`, noting each.
    fn make_parents(&mut self, path: &Path) -> Result<()> {
        let mut missing = vec![];
        let mut parent = path.parent();
        while let Some(folder) = parent.filter(|folder| !folder.exists()) {
            missing.push(folder.to_owned());
            parent = folder.parent();
        }
        for folder in missing.into_iter().rev() {
            fs::create_dir(&folder)
                .with_context(|| format!("make the folder {}", folder.display()))?;
            self.0.push(Step::Made(folder));
        }
        Ok(())
    }
    /// Writes `bytes` over the file at `path`, noting what it held.
    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        let before = fs::read(path)?;
        if before != bytes {
            self.0.push(Step::Wrote(path.to_owned(), before));
            replace(path, bytes)?;
        }
        Ok(())
    }
    /// Moves the file or folder `from` to `to`. A folder moved where one is
    /// already has what it holds moved into that one, and is removed; a path
    /// that differs from `to` only in letter case is renamed through another
    /// name, as Windows would leave it as it is.
    fn move_tree(&mut self, from: &Path, to: &Path) -> Result<()> {
        match fs::symlink_metadata(to) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.make_parents(to)?;
                move_path(from, to, "move")?;
                self.0.push(Step::Moved(from.to_owned(), to.to_owned()));
            }
            Err(error) => return Err(error.into()),
            Ok(_) if same_path(from, to)? => {
                let through = renamed_through(to);
                move_path(from, &through, "move")?;
                self.0.push(Step::Moved(from.to_owned(), through.clone()));
                move_path(&through, to, "move")?;
                self.0.push(Step::Moved(through, to.to_owned()));
            }
            Ok(meta) if meta.is_dir() && from.is_dir() => {
                let mut names = vec![];
                for entry in fs::read_dir(from)? {
                    names.push(entry?.file_name());
                }
                names.sort();
                for name in names {
                    self.move_tree(&from.join(&name), &to.join(&name))?;
                }
                fs::remove_dir(from)
                    .with_context(|| format!("remove the moved folder {}", from.display()))?;
                self.0.push(Step::Emptied(from.to_owned()));
            }
            Ok(_) => bail!("both {} and {} exist", from.display(), to.display()),
        }
        Ok(())
    }
    /// Takes back every step, the last first. Returns the steps it could
    /// not take back.
    fn undo(self) -> Vec<String> {
        let mut failed = vec![];
        for step in self.0.into_iter().rev() {
            let result = match &step {
                Step::Made(folder) => fs::remove_dir(folder).map_err(anyhow::Error::from),
                Step::Moved(from, to) => move_path(to, from, "move back"),
                Step::Emptied(folder) => fs::create_dir(folder).map_err(anyhow::Error::from),
                Step::Wrote(path, bytes) => replace(path, bytes),
            };
            if let Err(error) = result {
                failed.push(format!("{error:#}"));
            }
        }
        failed
    }
}

/// Whether `a` and `b` are the same file or folder, as two names differing
/// only in letter case are on Windows.
fn same_path(a: &Path, b: &Path) -> Result<bool> {
    Ok(crate::data::canonicalize(a)? == crate::data::canonicalize(b)?)
}

/// A name beside `path` that nothing has, to rename through.
fn renamed_through(path: &Path) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut n = std::process::id();
    loop {
        let through = path.with_file_name(format!(".{name}.arp-move-{n}"));
        if !through.exists() {
            return through;
        }
        n += 1;
    }
}

/// Fails when moving the file or folder `from` to `to` as
/// [`Journal::move_tree`] does would meet a file or link already there.
fn check_tree(from: &Path, to: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(from)?;
    ensure!(!is_link(&meta), "links are not allowed: {}", from.display());
    match fs::symlink_metadata(to) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) if same_path(from, to)? => Ok(()),
        Ok(target) if target.is_dir() && meta.is_dir() && !is_link(&target) => {
            for entry in fs::read_dir(from)? {
                let name = entry?.file_name();
                check_tree(&from.join(&name), &to.join(&name))?;
            }
            Ok(())
        }
        Ok(_) => bail!(
            "both {} and {} exist; move or delete one of them first",
            from.display(),
            to.display()
        ),
    }
}

/// Fails when the document ID `new` cannot be had beside the existing
/// document `existing`: the same ID, one ID a folder of the other, or a
/// folder or name differing only in letter case, which Windows and macOS
/// take as the same.
fn check_beside(new: &str, existing: &str) -> Result<()> {
    let (mut a, mut b) = (new.split('/'), existing.split('/'));
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) => {
                ensure!(
                    x.to_lowercase() != y.to_lowercase(),
                    "{new} differs from the existing document {existing} only in letter case"
                );
                return Ok(());
            }
            (None, None) => bail!("a document is already at {new}"),
            _ => bail!("{new} and the existing document {existing} would be a file and a folder"),
        }
    }
}

/// What a move found of the records of one document or proposal before it
/// moved them: what it carries to the new place.
struct Kept {
    /// `documents` or `changes`.
    area: &'static str,
    from: String,
    to: String,
    /// The hash of the records' files at the earlier place, which they
    /// keep as they move.
    fingerprint: String,
    /// Whether the review of the content was of the content as it was.
    reviewed: bool,
    /// Whether the formation was of the content as it was.
    formed: bool,
    /// The hash the review of the structure interpretation binds.
    structure_review: Option<String>,
    /// For a proposal, whether its base was the adopted document as it was.
    based: bool,
}

/// Whether the HTML page in `dir`, whose extraction names pictures by paths
/// relative to it, reads other pictures from another folder.
fn relative_pictures(dir: &Path) -> Result<bool> {
    let extraction = crate::extraction::read(&dir.join("extraction.json"))?;
    Ok(extraction.sheets.iter().any(|sheet| {
        sheet["images"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|image| {
                let src = image["src"].as_str().unwrap_or("").trim();
                let path = src.split(['?', '#']).next().unwrap_or("");
                let scheme = path.split_once(':').is_some_and(|(name, _)| {
                    name.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                });
                !path.is_empty() && !scheme && !path.starts_with('/') && !path.starts_with('\\')
            })
    }))
}

fn parent_of(id: &str) -> &str {
    id.rsplit_once('/').map_or("", |(parent, _)| parent)
}

impl Store {
    /// Moves the document `from`, or the folder of documents, to `to`: the
    /// original (or the folder holding them, with every file in it), the
    /// records and proposals of each document, and the registry's records
    /// citing them, as one revision by `actor` for `reason`. An original
    /// already moved by hand is found at the new place. Reviews, structure
    /// reviews and approvals of what is still the same are kept.
    pub fn relocate(&self, from: &str, to: &str, actor: &str, reason: &str) -> Result<Value> {
        document_id(from)?;
        document_id(to)?;
        nonempty(actor)?;
        nonempty(reason)?;
        ensure!(from != to, "the document is already at {to}");
        ensure!(
            !to.starts_with(&format!("{from}/")) && !from.starts_with(&format!("{to}/")),
            "cannot move {from} to {to}, inside itself or into a folder holding it"
        );
        let target = |id: &str| format!("{to}{}", &id[from.len()..]);
        let documents = self.ids("documents", Some(from))?;
        let proposals = self.ids("changes", Some(from))?;
        let moving: BTreeSet<String> = documents.iter().chain(&proposals).cloned().collect();
        ensure!(!moving.is_empty(), "no document at or below {from}");
        let mut existing: BTreeSet<String> = self.ids("documents", None)?.into_iter().collect();
        existing.extend(self.ids("changes", None)?);
        for id in &moving {
            let new = target(id);
            document_id(&new)?;
            self.ensure_included(&new)?;
            for other in existing.iter().filter(|other| !moving.contains(*other)) {
                check_beside(&new, other)?;
            }
        }
        let runs = crate::workflow::ensure_no_open_run(&self.root, &moving, "move")?;

        // The originals: where they are, and that each is at one place.
        let source_from = under(&self.root, &self.source_path(from)?)?;
        let source_to = under(&self.root, &self.source_path(to)?)?;
        let move_source = fs::symlink_metadata(&source_from).is_ok();
        if move_source {
            check_tree(&source_from, &source_to)?;
        }
        let mut limits = crate::path_limits::PathLimits::new(&self.root);
        let mut originals = BTreeMap::new();
        for id in &moving {
            let new = target(id);
            let state = if self.original(&self.source_path(id)?)?.is_some() {
                "moved"
            } else if self.original(&self.source_path(&new)?)?.is_some() {
                "already_moved"
            } else {
                bail!(
                    "the original of {id} is at neither {} nor {}",
                    self.source_path(id)?,
                    self.source_path(&new)?
                );
            };
            limits.check(&under(&self.root, &self.source_path(&new)?)?)?;
            originals.insert(id.clone(), state);
        }

        // The records: what each keeps of its reviews.
        let mut kept = vec![];
        let mut adopted_fingerprints = BTreeMap::new();
        for (area, ids) in [("documents", &documents), ("changes", &proposals)] {
            for id in ids.iter() {
                let dir = under(&self.arp, &format!("{area}/{id}"))?;
                let new_dir = under(&self.arp, &format!("{area}/{}", target(id)))?;
                for name in files(&dir)?.keys() {
                    limits.check(&new_dir.join(name))?;
                }
                let fingerprint = self
                    .fingerprint(&dir)?
                    .with_context(|| format!("{area}/{id} has no document.yml"))?;
                let formed = dir.join("formation.json").is_file()
                    && self.formation(&dir)?.0["content"] == fingerprint;
                let mappings = read_mappings(&dir.join("mappings.yml"))?;
                let structure_review = mappings["interpretation"]["review"]["content_hash"]
                    .as_str()
                    .map(str::to_owned);
                let based = area == "changes"
                    && read(&dir.join("proposal.json"), Some("proposal"))?["base"]
                        .as_str()
                        .is_some_and(|base| adopted_fingerprints.get(id) == Some(&base.to_owned()));
                if area == "documents" {
                    adopted_fingerprints.insert(id.clone(), fingerprint.clone());
                }
                kept.push(Kept {
                    area,
                    from: id.clone(),
                    to: target(id),
                    reviewed: self.current_review(&dir)?.is_some(),
                    formed,
                    structure_review,
                    based,
                    fingerprint,
                });
            }
        }

        // The registry as the move leaves it, made before anything moves.
        let registry_path = under(&self.arp, "registry")?;
        let registry = if registry_path.join("registry.json").is_file() {
            let moved = moving
                .iter()
                .map(|id| {
                    Ok((
                        id.clone(),
                        Moved {
                            to: target(id),
                            path: self.source_path(&target(id))?,
                        },
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            registry::relocate(registry::load(&registry_path)?, &moved, actor, reason)?
        } else {
            None
        };

        let mut journal = Journal::default();
        let moved = (|| -> Result<Vec<Value>> {
            if move_source {
                journal.move_tree(&source_from, &source_to)?;
            }
            for area in ["documents", "changes"] {
                let records = under(&self.arp, &format!("{area}/{from}"))?;
                if records.exists() {
                    journal.move_tree(&records, &under(&self.arp, &format!("{area}/{to}"))?)?;
                }
            }
            let mut adopted = BTreeMap::new();
            let mut results = vec![];
            for kept in &kept {
                let result = self.rebind(&mut journal, kept, &adopted)?;
                if kept.area == "documents" {
                    adopted.insert(kept.to.clone(), string(&result["fingerprint"])?.to_owned());
                }
                results.push(result);
            }
            if let Some(registry) = &registry {
                registry::save(registry, &registry_path)?;
            }
            Ok(results)
        })();
        let results = match moved {
            Ok(results) => results,
            Err(error) => {
                let failed = journal.undo();
                if failed.is_empty() {
                    return Err(error);
                }
                bail!(
                    "{error:#}; putting back what the move changed also failed: {}",
                    failed.join("; ")
                );
            }
        };

        // What only the earlier IDs named: views, confirmations and the
        // folders the records left.
        for area in ["documents", "changes"] {
            let records = under(&self.arp, &format!("{area}/{from}"))?;
            if let Some(parent) = records.parent() {
                remove_empty_directories(parent, &self.arp);
            }
        }
        for id in &moving {
            self.remove_views(id, true)?;
            self.remove_confirmation(id)?;
        }
        let mut reimport = vec![];
        let mut items = vec![];
        for (kept, mut result) in kept.iter().zip(results) {
            if let Some(map) = result.as_object_mut() {
                map.remove("fingerprint");
            }
            if kept.area == "documents" {
                result["original"] = json!(originals[&kept.from]);
                let dir = under(&self.arp, &format!("documents/{}", kept.to))?;
                if ["html", "htm"].contains(&extension(Path::new(&kept.to)).as_str())
                    && parent_of(&kept.from) != parent_of(&kept.to)
                    && relative_pictures(&dir)?
                {
                    reimport.push(json!(kept.to));
                }
            } else if !documents.contains(&kept.from) {
                result["original"] = json!(originals[&kept.from]);
            }
            items.push(result);
        }
        let mut out = json!({"state":"moved","from":from,"to":to,"moved":items});
        if !reimport.is_empty() {
            out["reimport"] = json!(reimport);
            out["detail"] = json!(
                "These HTML pages read pictures by paths relative to their folder, which changed: import each again with documents import <path> --force."
            );
        }
        if let Some(registry) = &registry {
            let change = &registry.manifest.last_change;
            out["registry"] = json!({"revision":change.revision,
                "moved_documents":change.moved_documents,"repointed":change.repointed});
        }
        let completed: Vec<&Value> = runs
            .iter()
            .filter(|run| run["status"] == "complete")
            .collect();
        if !completed.is_empty() {
            out["workflows"] = json!(completed);
        }
        Ok(out)
    }

    /// Makes the records of `kept`, moved to their new place, those of the
    /// document at its new ID, and carries each review of what is the same
    /// as before. `adopted` holds the new hash of each adopted document
    /// moved before, which a proposal based on it is now based on.
    fn rebind(
        &self,
        journal: &mut Journal,
        kept: &Kept,
        adopted: &BTreeMap<String, String>,
    ) -> Result<Value> {
        let dir = under(&self.arp, &format!("{}/{}", kept.area, kept.to))?;
        ensure!(
            self.fingerprint(&dir)?.as_deref() == Some(kept.fingerprint.as_str()),
            "the records of {} changed while they moved",
            kept.from
        );
        ensure!(
            self.original(&self.source_path(&kept.to)?)?.is_some(),
            "the original of {} is not at {} after the move; a folder there may differ only in letter case",
            kept.from,
            self.source_path(&kept.to)?
        );
        let path = dir.join("document.yml");
        let mut meta = read(&path, Some("document"))?;
        meta["document_id"] = json!(kept.to);
        meta["source"]["path"] = json!(self.source_path(&kept.to)?);
        journal.write(&path, &serialized(&path, &meta)?)?;
        if kept.area == "changes" {
            let path = dir.join("proposal.json");
            let mut proposal = read(&path, Some("proposal"))?;
            proposal["document_id"] = json!(kept.to);
            if kept.based {
                proposal["base"] = json!(
                    adopted
                        .get(&kept.to)
                        .context("adopted document not moved")?
                );
            }
            journal.write(&path, &serialized(&path, &proposal)?)?;
        }
        let mut result = json!({"from":kept.from,"to":kept.to});
        if kept.area == "changes" {
            result["proposal"] = json!(true);
        }
        if let Some(earlier) = &kept.structure_review {
            result["structure_review"] =
                json!(self.rebind_structure(journal, kept, &dir, earlier)?);
        }
        let fingerprint = self.fingerprint(&dir)?.context("document.yml missing")?;
        if kept.formed {
            let path = dir.join("formation.json");
            let mut formation = read(&path, Some("formation"))?;
            formation["content"] = json!(fingerprint);
            journal.write(&path, &serialized(&path, &formation)?)?;
        }
        if kept.reviewed {
            let path = dir.join("review.json");
            let mut review = read(&path, Some("review"))?;
            review["content"] = json!(fingerprint);
            review["formation"] = json!(self.formation(&dir)?.1);
            journal.write(&path, &serialized(&path, &review)?)?;
            ensure!(
                self.current_review(&dir)?.is_some(),
                "the review of {} did not follow it to {}",
                kept.from,
                kept.to
            );
        }
        result["reviewed"] = json!(kept.reviewed);
        result["fingerprint"] = json!(fingerprint);
        Ok(result)
    }

    /// Makes the review of the structure interpretation of the moved records
    /// in `dir`, which bound the hash `earlier` of the structure at the
    /// earlier ID, bind the same structure at the new ID. Returns `kept`, or
    /// `lapsed` when the review was not of the structure as it was, or the
    /// original changed and the structure cannot be read until it is
    /// imported again.
    fn rebind_structure(
        &self,
        journal: &mut Journal,
        kept: &Kept,
        dir: &Path,
        earlier: &str,
    ) -> Result<&'static str> {
        let inspected = self.inspect(dir, false)?;
        if !inspected.source_current || !structure_format(&inspected.meta)? {
            return Ok("lapsed");
        }
        let (mut structure, _) = crate::document_structure::replay_verified_corrections(
            &self.root,
            &inspected.mappings["interpretation"],
            inspected.original_extraction(),
            string(&inspected.meta["extraction"])?,
            &inspected.axes,
        )?;
        let was = crate::document_structure::content_hash_at(
            &mut structure,
            &kept.from,
            &self.source_path(&kept.from)?,
        );
        if was != earlier {
            return Ok("lapsed");
        }
        let now = crate::document_structure::content_hash(&structure);
        let path = dir.join("mappings.yml");
        let mut mappings = read_mappings(&path)?;
        mappings["interpretation"]["review"]["content_hash"] = json!(now);
        journal.write(&path, &stored_mappings(&path, &mappings)?)?;
        Ok("kept")
    }

    fn remove_confirmation(&self, id: &str) -> Result<()> {
        let folder = under(&self.arp, &format!("cache/confirmations/{id}"))?;
        if folder.exists() {
            remove_folder(&folder, "remove the export confirmation")?;
            remove_empty_directories(folder.parent().unwrap(), &self.arp);
        }
        Ok(())
    }

    /// Documents and proposals at or below `scope` whose original no longer exists.
    pub fn missing(&self, scope: Option<&str>) -> Result<Vec<String>> {
        let mut ids: BTreeSet<String> = self.ids("documents", scope)?.into_iter().collect();
        ids.extend(self.ids("changes", scope)?);
        let mut missing = vec![];
        for id in ids {
            if self.original(&self.source_path(&id)?)?.is_none() {
                missing.push(id);
            }
        }
        Ok(missing)
    }

    /// Removes the documents and proposals at or below `scope`. Their
    /// originals must be gone already, or with `delete_originals` are deleted
    /// with them, when each holds what was imported. The registry's records
    /// citing them are listed, for them to be retired or cite another
    /// document; a moved original is moved with `relocate` instead.
    pub fn remove(&self, scope: &str, delete_originals: bool) -> Result<Value> {
        document_id(scope)?;
        let mut ids: BTreeSet<String> = self.ids("documents", Some(scope))?.into_iter().collect();
        ids.extend(self.ids("changes", Some(scope))?);
        ensure!(!ids.is_empty(), "no document at or below {scope}");
        let missing: BTreeSet<_> = self.missing(Some(scope))?.into_iter().collect();
        // An excluded original is not a document, so it stays where it is.
        let exclusion = self.exclusion()?;
        let (excluded, present): (Vec<&String>, Vec<&String>) = ids
            .iter()
            .filter(|id| !missing.contains(*id))
            .partition(|id| exclusion.pattern(id).is_some());
        ensure!(
            delete_originals || present.is_empty(),
            "originals still exist: {}; move them with documents move, or delete them too with --delete-originals",
            present
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut originals = vec![];
        for id in &present {
            let path = self
                .original(&self.source_path(id)?)?
                .context("original missing")?;
            let held = hash(&fs::read(&path)?);
            let mut imported = vec![];
            for area in ["documents", "changes"] {
                let meta = under(&self.arp, &format!("{area}/{id}/document.yml"))?;
                if meta.is_file() {
                    imported.push(read(&meta, Some("document"))?["source"]["sha256"].clone());
                }
            }
            ensure!(
                imported.contains(&json!(held)),
                "the original of {id} holds changes not imported; import it first, or delete it yourself"
            );
            originals.push(path);
        }
        crate::workflow::ensure_no_open_run(&self.root, &ids, "remove")?;
        let registry_path = under(&self.arp, "registry")?;
        let citing = if registry_path.join("registry.json").is_file() {
            registry::citing(&registry::load(&registry_path)?, &ids)?
        } else {
            BTreeMap::new()
        };
        // The originals go into a stage first, so they come back if the
        // records cannot all be removed; the stage is deleted at the end.
        let stage = Stage::new_in(&self.arp.join("work"), &self.arp)?;
        let mut journal = Journal::default();
        let removed = (|| -> Result<()> {
            for (index, path) in originals.iter().enumerate() {
                let staged = stage.path().join(index.to_string());
                move_path(path, &staged, "delete the original")?;
                journal.0.push(Step::Moved(path.clone(), staged));
            }
            for area in ["documents", "changes"] {
                for id in &ids {
                    let dir = under(&self.arp, &format!("{area}/{id}"))?;
                    if dir.exists() {
                        remove_folder(&dir, "remove the document of a missing original")?;
                        remove_empty_directories(dir.parent().unwrap(), &self.arp);
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = removed {
            let failed = journal.undo();
            if failed.is_empty() {
                return Err(error);
            }
            bail!(
                "{error:#}; putting back the originals also failed: {}",
                failed.join("; ")
            );
        }
        drop(stage);
        for id in &ids {
            self.remove_views(id, true)?;
            self.remove_confirmation(id)?;
        }
        let mut out = json!({"removed":ids});
        if !present.is_empty() {
            out["deleted_originals"] = json!(present);
        }
        if !excluded.is_empty() {
            out["kept_originals"] = json!(excluded);
        }
        if !citing.is_empty() {
            out["registry_entries"] = json!(citing);
            out["detail"] = json!(
                "These registry records quote the removed documents: retire them, or cite another document, with spec registry apply."
            );
        }
        Ok(out)
    }
}
