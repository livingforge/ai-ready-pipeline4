use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) type Stamp = (String, References);
pub(super) const READ_BUFFER_SIZE: usize = 64 * 1024;

fn hash_file(path: &Path, buffer: &mut [u8]) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    loop {
        let n = match file.read(buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            other => other?,
        };
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_hash_matches_whole_file_across_buffer_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.bin");
        let mut buffer = vec![0; READ_BUFFER_SIZE];
        for length in [
            0,
            1,
            READ_BUFFER_SIZE - 1,
            READ_BUFFER_SIZE,
            READ_BUFFER_SIZE * 3 + 7,
        ] {
            let raw: Vec<_> = (0..length).map(|i| (i % 251) as u8).collect();
            fs::write(&path, &raw).unwrap();
            assert_eq!(hash_file(&path, &mut buffer).unwrap(), hash(&raw));
        }
    }
}

/// Only parsed references are cached. Every referenced file is still read and
/// hashed on every explicit index refresh, including changes that preserve size and mtime.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct References {
    descriptor: String,
    source: String,
    images: Vec<String>,
}

impl Store {
    /// Hash independent documents concurrently, with bounded read buffers and
    /// no SQLite access from workers. Results stay in the original ID order.
    pub(super) fn search_stamps(
        &self,
        ids: &[String],
        base: &Path,
        config_hash: &str,
        cached: &BTreeMap<String, (String, String)>,
    ) -> Result<Vec<Result<Stamp>>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let workers = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4)
            .min(ids.len());
        std::thread::scope(|scope| {
            let handles: Vec<_> = ids
                .chunks(ids.len().div_ceil(workers))
                .map(|chunk| {
                    scope.spawn(move || {
                        let mut buffer = vec![0; READ_BUFFER_SIZE];
                        chunk
                            .iter()
                            .map(|id| {
                                document_id(id)?;
                                let dir = under(base, id)?;
                                let refs = cached
                                    .get(id)
                                    .map(|(_, refs)| serde_json::from_str(refs))
                                    .transpose()?;
                                self.search_stamp(&dir, config_hash, refs, &mut buffer)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut results = Vec::with_capacity(ids.len());
            for handle in handles {
                results.extend(
                    handle
                        .join()
                        .map_err(|_| anyhow::anyhow!("search integrity worker panicked"))?,
                );
            }
            Ok(results)
        })
    }

    pub(super) fn search_stamp(
        &self,
        dir: &Path,
        config_hash: &str,
        previous: Option<References>,
        buffer: &mut [u8],
    ) -> Result<Stamp> {
        let mut hashes = BTreeMap::new();
        for (name, path) in self.logical(dir)? {
            hashes.insert(name, hash_file(&path, buffer)?);
        }
        let descriptor = hash(&encoded(&json!([
            hashes.get("document.yml"),
            hashes.get("mappings.yml")
        ])));
        let references = if let Some(previous) = previous.filter(|r| r.descriptor == descriptor) {
            previous
        } else {
            let meta = read(&dir.join("document.yml"), Some("document"))?;
            let mappings = read(&dir.join("mappings.yml"), None)?;
            References {
                descriptor,
                source: string(&meta["source"]["path"])?.into(),
                images: mappings["interpretation"]["regions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|region| Ok(string(&region["image"])?.to_owned()))
                    .collect::<Result<_>>()?,
            }
        };
        hashes.insert(
            "@original".into(),
            self.original(&references.source)?
                .map(|p| hash_file(&p, buffer))
                .transpose()?
                .unwrap_or_default(),
        );
        hashes.insert("@config".into(), config_hash.to_owned());
        for image in &references.images {
            hashes.insert(
                format!("@region/{image}"),
                hash_file(&under(&self.root, image)?, buffer)?,
            );
        }
        Ok((hash(&encoded(&json!(hashes))), references))
    }
}
