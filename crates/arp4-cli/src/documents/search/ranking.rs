//! Rankings saved for later pages. A page after the first reads its hits from
//! the ranking an earlier page beyond the first saved, instead of ranking every
//! hit again; reading all pages one by one then takes time in proportion to the
//! hits rather than to their square. A ranking is a cache: it is keyed by the
//! search revision and the refresh that built the index, and every refresh
//! removes them all, as it may give passages other IDs.
use crate::data::{self, hash, remove_empty_directories};
use anyhow::Result;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const MAGIC: &[u8; 8] = b"ARPRANK1";
/// The magic, the key (64 hex digits) and the number of hits.
const HEADER: usize = 8 + 64 + 8;
/// A hit: its passage ID, score, group hits and flags, padded.
const RECORD: usize = 24;
/// The rankings kept; saving another removes the oldest.
const KEPT: usize = 32;

/// A hit as ranked, without its passage: what the response tells of its
/// rank. `score` is none for a search by format alone; `group_hits` is the
/// size of the heading group of a hit that matched its headings only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ranked {
    pub id: i64,
    pub score: Option<f64>,
    pub exact: bool,
    pub phrase: bool,
    pub heading: bool,
    pub group_hits: Option<u32>,
}

impl Ranked {
    /// A hit from the first columns of a ranking query: passage ID, score,
    /// exact, phrase, heading, headed and group hits.
    pub fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            score: row.get(1)?,
            exact: row.get(2)?,
            phrase: row.get(3)?,
            heading: row.get(4)?,
            group_hits: if row.get::<_, bool>(5)? {
                Some(row.get(6)?)
            } else {
                None
            },
        })
    }

    fn encode(&self, bytes: &mut Vec<u8>) {
        bytes.extend(self.id.to_le_bytes());
        bytes.extend(self.score.unwrap_or(0.0).to_le_bytes());
        bytes.extend(self.group_hits.unwrap_or(0).to_le_bytes());
        let flags = u8::from(self.score.is_some())
            | u8::from(self.exact) << 1
            | u8::from(self.phrase) << 2
            | u8::from(self.heading) << 3
            | u8::from(self.group_hits.is_some()) << 4;
        bytes.extend([flags, 0, 0, 0]);
    }

    fn decode(record: &[u8]) -> Self {
        let flag = |bit: u8| record[20] & (1 << bit) != 0;
        Self {
            id: i64::from_le_bytes(record[..8].try_into().unwrap()),
            score: flag(0).then(|| f64::from_le_bytes(record[8..16].try_into().unwrap())),
            exact: flag(1),
            phrase: flag(2),
            heading: flag(3),
            group_hits: flag(4).then(|| u32::from_le_bytes(record[16..20].try_into().unwrap())),
        }
    }
}

/// The key a ranking is saved under: its search `revision` and the refresh
/// of the index it was ranked in.
pub(super) fn key(revision: &str, indexed_at_unix: u64) -> String {
    hash(format!("{revision}\n{indexed_at_unix}").as_bytes())
}

/// The hits from `offset`, at most `limit` of them (all for none), of the
/// ranking saved under `key` in `directory`, with the number of its hits;
/// none when no ranking is saved under it or it does not read back whole.
pub(super) fn load(
    directory: &Path,
    key: &str,
    offset: usize,
    limit: Option<usize>,
) -> Option<(usize, Vec<Ranked>)> {
    let mut file = fs::File::open(directory.join(key)).ok()?;
    let mut header = [0; HEADER];
    file.read_exact(&mut header).ok()?;
    if &header[..8] != MAGIC || &header[8..72] != key.as_bytes() {
        return None;
    }
    let total = usize::try_from(u64::from_le_bytes(header[72..].try_into().ok()?)).ok()?;
    let length = total.checked_mul(RECORD)?.checked_add(HEADER)?;
    if file.metadata().ok()?.len() != u64::try_from(length).ok()? {
        return None;
    }
    let start = offset.min(total);
    let end = limit.map_or(total, |limit| start.saturating_add(limit).min(total));
    file.seek(SeekFrom::Start(
        u64::try_from(HEADER + start * RECORD).ok()?,
    ))
    .ok()?;
    let mut records = vec![0; (end - start) * RECORD];
    file.read_exact(&mut records).ok()?;
    Some((
        total,
        records
            .as_chunks::<RECORD>()
            .0
            .iter()
            .map(|record| Ranked::decode(record))
            .collect(),
    ))
}

/// Saves `ranking` under `key` in `directory`, keeping only the newest
/// rankings. A search that cannot save its ranking still answers; its next
/// page ranks again.
pub(super) fn save(directory: &Path, key: &str, ranking: &[Ranked]) {
    let mut bytes = Vec::with_capacity(HEADER + ranking.len() * RECORD);
    bytes.extend(MAGIC);
    bytes.extend(key.as_bytes());
    bytes.extend((ranking.len() as u64).to_le_bytes());
    for hit in ranking {
        hit.encode(&mut bytes);
    }
    if data::replace_cache(&directory.join(key), &bytes).is_ok() {
        data::keep_newest(directory, KEPT);
    }
}

/// Removes every ranking saved in `directory`, and the folders this leaves
/// empty below `boundary`.
pub(super) fn clear(directory: &Path, boundary: &Path) -> Result<()> {
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    if let Some(parent) = directory.parent() {
        remove_empty_directories(parent, boundary);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_ranking_reads_back_from_any_offset() {
        let directory = tempfile::tempdir().unwrap();
        let ranking: Vec<_> = (1..=50)
            .map(|id| Ranked {
                id,
                score: (id % 2 == 0).then_some(-1.0 / id as f64),
                exact: id % 3 == 0,
                phrase: id % 5 == 0,
                heading: id % 7 == 0,
                group_hits: (id % 11 == 0).then_some(id as u32),
            })
            .collect();
        let key = key("revision", 1);
        assert_eq!(load(directory.path(), &key, 0, None), None);
        save(directory.path(), &key, &ranking);
        assert_eq!(
            load(directory.path(), &key, 0, None),
            Some((50, ranking.clone()))
        );
        assert_eq!(
            load(directory.path(), &key, 45, Some(10)),
            Some((50, ranking[45..].to_vec()))
        );
        assert_eq!(
            load(directory.path(), &key, 80, Some(10)),
            Some((50, vec![]))
        );
        // Another key, or a ranking cut short, reads as none.
        assert_eq!(
            load(directory.path(), &super::key("revision", 2), 0, None),
            None
        );
        let path = directory.path().join(&key);
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(load(directory.path(), &key, 0, None), None);
    }

    #[test]
    fn saving_keeps_the_newest_rankings() {
        let directory = tempfile::tempdir().unwrap();
        for i in 0..KEPT + 3 {
            save(directory.path(), &key("revision", i as u64), &[]);
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), KEPT);
    }
}
