//! Opening an Office package's ZIP container the way Office reads it.
//!
//! Office ignores several ZIP details that the zip crate validates, so packages
//! that Office opens without complaint would otherwise fail to import. The
//! central directory is rewritten in memory before the zip crate sees it; the
//! entry data and the original file stay untouched.

use anyhow::{Result, bail, ensure};
use std::{
    borrow::Cow,
    io::{Cursor, Read, Seek},
};
use zip::{ZipArchive, read::ZipFile};

/// Extra fields that Office does not read and whose content the zip crate
/// rejects when it is malformed: extended timestamp, NTFS times, and the
/// Info-ZIP Unicode path and comment.
const IGNORED_EXTRA_FIELDS: [u16; 4] = [0x5455, 0x000A, 0x7075, 0x6375];
/// The AES extra field only describes entries stored with the AES method; on
/// other entries the zip crate would treat them as encrypted.
const AES_EXTRA_FIELD: u16 = 0x9901;
const AES_METHOD: u16 = 99;
const UTF8_NAME_FLAG: u16 = 0x0800;

/// Office writes encrypted packages (password, IRM, sensitivity labels) and the
/// binary formats as OLE compound files rather than ZIP packages.
fn ensure_zip_package(raw: &[u8]) -> Result<()> {
    const OLE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if !raw.starts_with(&OLE) {
        return Ok(());
    }
    let encrypted: Vec<u8> = "EncryptedPackage"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    if raw.windows(encrypted.len()).any(|w| w == encrypted) {
        bail!(
            "the file is encrypted (password, IRM or sensitivity label); remove the protection in Office, save it again and import that copy"
        );
    }
    bail!(
        "the file is a binary Office document with an Open XML extension; save it in Office as an Open XML file and import that copy"
    )
}

/// The package bytes with the central directory as Office reads it: extra
/// fields Office ignores are left out and `\` separators in part names become
/// `/`. Writers that copy entries from this package therefore write the
/// normalized directory.
pub(crate) fn office_package(raw: &[u8]) -> Result<Cow<'_, [u8]>> {
    ensure_zip_package(raw)?;
    Ok(normalized_directory(raw).map_or(Cow::Borrowed(raw), Cow::Owned))
}

pub(crate) fn office_archive(raw: &[u8]) -> Result<ZipArchive<Cursor<Cow<'_, [u8]>>>> {
    Ok(ZipArchive::new(Cursor::new(office_package(raw)?))?)
}

/// The declared uncompressed size of all entries. The zip crate's own total
/// gives up on entries written with a data descriptor, although the central
/// directory records their sizes too.
pub(crate) fn uncompressed_size<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<u128> {
    let mut total = 0u128;
    for index in 0..archive.len() {
        total += u128::from(archive.by_index_raw(index)?.size());
    }
    Ok(total)
}

/// Reads an entry, failing when it inflates past its declared size, so budgets
/// checked against declared sizes also bound memory.
pub(crate) fn read_entry<R: Read>(entry: &mut ZipFile<'_, R>) -> Result<Vec<u8>> {
    let size = entry.size();
    let mut bytes = vec![];
    (&mut *entry)
        .take(size.saturating_add(1))
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= size,
        "ZIP entry {} inflates past its declared size",
        entry.name()
    );
    Ok(bytes)
}

fn u16_at(raw: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(raw.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(raw: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(raw.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(raw: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(raw.get(at..at + 8)?.try_into().ok()?))
}

/// The package with a rewritten central directory, or `None` when nothing
/// changes or the directory does not have the usual layout (directory, then
/// the ZIP64 end records, then the end record); the zip crate then reads the
/// original bytes and reports its own error.
fn normalized_directory(raw: &[u8]) -> Option<Vec<u8>> {
    const EOCD: usize = 22;
    const EOCD64: usize = 56;
    const LOCATOR: usize = 20;
    let lowest = raw.len().saturating_sub(EOCD + usize::from(u16::MAX));
    let eocd = (lowest..=raw.len().checked_sub(EOCD)?).rev().find(|&at| {
        raw[at..].starts_with(b"PK\x05\x06")
            && u16_at(raw, at + 20)
                .is_some_and(|comment| at + EOCD + usize::from(comment) <= raw.len())
    })?;
    let zip64 = eocd.checked_sub(LOCATOR + EOCD64).filter(|&record| {
        raw[eocd - LOCATOR..].starts_with(b"PK\x06\x07")
            && raw[record..].starts_with(b"PK\x06\x06")
            && u64_at(raw, record + 4) == Some((EOCD64 - 12) as u64)
    });
    let (end, entries, size) = match zip64 {
        Some(record) => (record, u64_at(raw, record + 32)?, u64_at(raw, record + 40)?),
        None => (
            eocd,
            u64::from(u16_at(raw, eocd + 10)?),
            u64::from(u32_at(raw, eocd + 12)?),
        ),
    };
    let start = end.checked_sub(usize::try_from(size).ok()?)?;

    let mut directory = Vec::with_capacity(end - start);
    let mut changed = false;
    let mut at = start;
    for _ in 0..entries {
        let header = raw.get(at..at + 46)?;
        if !header.starts_with(b"PK\x01\x02") {
            return None;
        }
        let flags = u16_at(header, 8)?;
        let method = u16_at(header, 10)?;
        let name_end = at + 46 + usize::from(u16_at(header, 28)?);
        let extra_end = name_end + usize::from(u16_at(header, 30)?);
        let comment_end = extra_end + usize::from(u16_at(header, 32)?);
        let name = raw.get(at + 46..name_end)?;
        let extra = kept_extra_fields(raw.get(name_end..extra_end)?, method)?;
        let comment = raw.get(extra_end..comment_end)?;
        // `\` is only a separator when no multi-byte encoding can contain the byte.
        let separators = (flags & UTF8_NAME_FLAG != 0 || name.is_ascii()) && name.contains(&b'\\');
        changed |= separators || matches!(extra, Cow::Owned(_));
        directory.extend_from_slice(&header[..30]);
        directory.extend_from_slice(&u16::try_from(extra.len()).ok()?.to_le_bytes());
        directory.extend_from_slice(&header[32..]);
        if separators {
            directory.extend(name.iter().map(|&b| if b == b'\\' { b'/' } else { b }));
        } else {
            directory.extend_from_slice(name);
        }
        directory.extend_from_slice(&extra);
        directory.extend_from_slice(comment);
        at = comment_end;
    }
    if !changed || at != end {
        return None;
    }

    // Leaving fields out only shrinks the directory, so its start, the
    // relative offsets before it and the 32-bit size field stay valid.
    let shrink = end - start - directory.len();
    let mut output = Vec::with_capacity(raw.len() - shrink);
    output.extend_from_slice(&raw[..start]);
    output.extend_from_slice(&directory);
    let tail = output.len();
    output.extend_from_slice(&raw[end..]);
    let new_size = directory.len() as u64;
    if zip64.is_some() {
        output[tail + 40..tail + 48].copy_from_slice(&new_size.to_le_bytes());
        let locator = tail + EOCD64 + 8;
        let record = u64_at(&output, locator)?.checked_sub(shrink as u64)?;
        output[locator..locator + 8].copy_from_slice(&record.to_le_bytes());
    }
    let eocd = tail + (eocd - end);
    if u32_at(&output, eocd + 12)? != u32::MAX {
        output[eocd + 12..eocd + 16].copy_from_slice(&u32::try_from(new_size).ok()?.to_le_bytes());
    }
    Some(output)
}

/// The extra fields Office reads, or `None` when a field overruns the block;
/// the zip crate then reports the malformed directory.
fn kept_extra_fields(extra: &[u8], method: u16) -> Option<Cow<'_, [u8]>> {
    let mut kept = Vec::with_capacity(extra.len());
    let mut dropped = false;
    let mut rest = extra;
    while rest.len() >= 4 {
        let id = u16_at(rest, 0)?;
        let field = rest.get(..4 + usize::from(u16_at(rest, 2)?))?;
        if IGNORED_EXTRA_FIELDS.contains(&id) || (id == AES_EXTRA_FIELD && method != AES_METHOD) {
            dropped = true;
        } else {
            kept.extend_from_slice(field);
        }
        rest = &rest[field.len()..];
    }
    kept.extend_from_slice(rest);
    Some(if dropped {
        Cow::Owned(kept)
    } else {
        Cow::Borrowed(extra)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    }

    struct Entry {
        name: &'static [u8],
        data: &'static [u8],
        flags: u16,
        extra: Vec<u8>,
        declared: Option<u32>,
    }

    fn entry(name: &'static [u8], data: &'static [u8]) -> Entry {
        Entry {
            name,
            data,
            flags: 0,
            extra: vec![],
            declared: None,
        }
    }

    fn field(id: u16, body: &[u8]) -> Vec<u8> {
        [
            &id.to_le_bytes()[..],
            &(body.len() as u16).to_le_bytes(),
            body,
        ]
        .concat()
    }

    /// A stored package written byte by byte, so the central directory can carry
    /// what other ZIP writers produce.
    fn package(entries: &[Entry], zip64: bool) -> Vec<u8> {
        let mut out = vec![];
        let mut directory = vec![];
        for e in entries {
            let offset = out.len() as u32;
            let crc = crc32(e.data);
            let size = e.data.len() as u32;
            let descriptor = e.flags & 0x08 != 0;
            let (local_crc, local_size) = if descriptor { (0, 0) } else { (crc, size) };
            out.extend(0x0403_4B50u32.to_le_bytes());
            out.extend([20, 0]);
            out.extend(e.flags.to_le_bytes());
            out.extend([0u8; 6]); // stored, time, date
            out.extend(local_crc.to_le_bytes());
            out.extend(local_size.to_le_bytes());
            out.extend(local_size.to_le_bytes());
            out.extend((e.name.len() as u16).to_le_bytes());
            out.extend([0, 0]);
            out.extend(e.name);
            out.extend(e.data);
            if descriptor {
                out.extend(0x0807_4B50u32.to_le_bytes());
                out.extend(crc.to_le_bytes());
                out.extend(size.to_le_bytes());
                out.extend(size.to_le_bytes());
            }
            let declared = e.declared.unwrap_or(size);
            let mut extra = e.extra.clone();
            let (compressed, uncompressed, offset) = if zip64 {
                let sizes = [u64::from(declared), u64::from(size), u64::from(offset)];
                extra.extend(field(1, &sizes.map(u64::to_le_bytes).concat()));
                (u32::MAX, u32::MAX, u32::MAX)
            } else {
                (size, declared, offset)
            };
            directory.extend(0x0201_4B50u32.to_le_bytes());
            directory.extend([20, 0, 20, 0]);
            directory.extend(e.flags.to_le_bytes());
            directory.extend([0u8; 6]);
            directory.extend(crc.to_le_bytes());
            directory.extend(compressed.to_le_bytes());
            directory.extend(uncompressed.to_le_bytes());
            directory.extend((e.name.len() as u16).to_le_bytes());
            directory.extend((extra.len() as u16).to_le_bytes());
            directory.extend([0u8; 10]); // comment length, disk, attributes
            directory.extend(offset.to_le_bytes());
            directory.extend(e.name);
            directory.extend(extra);
        }
        let start = out.len() as u64;
        out.extend(&directory);
        let count = entries.len() as u64;
        if zip64 {
            let record = out.len() as u64;
            out.extend(0x0606_4B50u32.to_le_bytes());
            out.extend(44u64.to_le_bytes());
            out.extend([45, 0, 45, 0]);
            out.extend([0u8; 8]);
            for value in [count, count, directory.len() as u64, start] {
                out.extend(value.to_le_bytes());
            }
            out.extend(0x0706_4B50u32.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(record.to_le_bytes());
            out.extend(1u32.to_le_bytes());
        }
        out.extend(0x0605_4B50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((count as u16).to_le_bytes());
        out.extend((count as u16).to_le_bytes());
        let (size, offset) = if zip64 {
            (u32::MAX, u32::MAX)
        } else {
            (directory.len() as u32, start as u32)
        };
        out.extend(size.to_le_bytes());
        out.extend(offset.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    fn entries_read(raw: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut archive = office_archive(raw)?;
        let mut parts = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            parts.insert(file.name().to_owned(), read_entry(&mut file)?);
        }
        Ok(parts)
    }

    fn plain_read(raw: &[u8]) -> zip::result::ZipResult<()> {
        let mut archive = ZipArchive::new(Cursor::new(raw))?;
        for index in 0..archive.len() {
            std::io::copy(&mut archive.by_index(index)?, &mut std::io::sink())?;
        }
        Ok(())
    }

    /// Extra fields that Office opens without complaint but the zip crate rejects.
    fn unread_extra_fields() -> Vec<Entry> {
        let name = b"ppt/slides/slide1.xml";
        let mut wrong_crc = vec![1];
        wrong_crc.extend((crc32(name) ^ 1).to_le_bytes());
        wrong_crc.extend(name);
        let mut ntfs = vec![0u8; 4];
        ntfs.extend([1, 0, 24, 0]);
        ntfs.extend([0u8; 28]);
        let mut aes = 2u16.to_le_bytes().to_vec();
        aes.extend(b"AE\x03");
        aes.extend(8u16.to_le_bytes());
        let mut parts = vec![
            entry(b"[Content_Types].xml", b"<Types/>"),
            entry(name, b"<sld/>"),
            entry(b"ppt/slides/slide2.xml", b"<sld>2</sld>"),
            entry(b"ppt/slides/slide3.xml", b"<sld>3</sld>"),
            entry(b"ppt/presentation.xml", b"<presentation/>"),
        ];
        parts[0].extra = [field(0x5455, b""), field(0xCAFE, b"kept")].concat();
        parts[1].extra = field(0x7075, &wrong_crc);
        parts[2].extra = field(0x000A, &ntfs);
        parts[3].extra = field(0x9901, &aes);
        parts[4].extra = field(0x6375, b"\x01\0\0\0\0note");
        parts
    }

    #[test]
    fn extra_fields_office_does_not_read_are_left_out() {
        for zip64 in [false, true] {
            let raw = package(&unread_extra_fields(), zip64);
            assert!(plain_read(&raw).is_err());
            let parts = entries_read(&raw).unwrap();
            assert_eq!(parts.len(), 5);
            assert_eq!(parts["ppt/slides/slide1.xml"], b"<sld/>");
            assert_eq!(parts["ppt/slides/slide3.xml"], b"<sld>3</sld>");
            let mut archive = office_archive(&raw).unwrap();
            let types = archive.by_name("[Content_Types].xml").unwrap();
            assert_eq!(types.extra_data(), Some(&field(0xCAFE, b"kept")[..]));
        }
    }

    #[test]
    fn malformed_extra_field_blocks_are_left_to_the_zip_crate() {
        let mut parts = unread_extra_fields();
        parts[1].extra.extend([0xFE, 0xCA, 16, 0, 0]);
        let raw = package(&parts, false);
        assert!(normalized_directory(&raw).is_none());
        assert!(entries_read(&raw).is_err());
    }

    #[test]
    fn backslash_separators_become_slashes_unless_the_name_may_be_multibyte() {
        let raw = package(
            &[
                entry(b"ppt\\slides\\slide1.xml", b"<sld/>"),
                // Shift_JIS for 表 ends in 0x5C, which is not a separator there.
                entry(b"\x95\x5C.xml", b"<x/>"),
            ],
            false,
        );
        let parts = entries_read(&raw).unwrap();
        assert_eq!(parts["ppt/slides/slide1.xml"], b"<sld/>");
        assert!(parts.keys().any(|name| name.contains('\\')));
    }

    #[test]
    fn data_descriptor_entries_count_toward_the_declared_size() {
        let mut parts = vec![entry(b"a.xml", b"<a/>"), entry(b"b.xml", b"<bb/>")];
        parts[1].flags = 0x08;
        let raw = package(&parts, false);
        let mut archive = office_archive(&raw).unwrap();
        assert_eq!(archive.decompressed_size(), None);
        assert_eq!(uncompressed_size(&mut archive).unwrap(), 9);
        assert_eq!(entries_read(&raw).unwrap()["b.xml"], b"<bb/>");
    }

    #[test]
    fn entries_cannot_inflate_past_their_declared_size() {
        let mut parts = vec![entry(b"a.xml", b"<a>long</a>")];
        parts[0].declared = Some(3);
        let error = entries_read(&package(&parts, false))
            .unwrap_err()
            .to_string();
        assert!(error.contains("inflates past its declared size"), "{error}");
    }

    #[test]
    fn rewritten_packages_carry_the_normalized_directory() {
        let mut parts = unread_extra_fields();
        parts[2].name = b"ppt\\slides\\slide2.xml";
        let raw = package(&parts, false);
        let patched = BTreeMap::from([("ppt/presentation.xml".to_owned(), b"<p/>".to_vec())]);
        let written = crate::excel::archive_bytes(&raw, &patched).unwrap();
        plain_read(&written).unwrap();
        let parts = entries_read(&written).unwrap();
        assert_eq!(parts["ppt/slides/slide2.xml"], b"<sld>2</sld>");
        assert_eq!(parts["ppt/presentation.xml"], b"<p/>");
    }
}
