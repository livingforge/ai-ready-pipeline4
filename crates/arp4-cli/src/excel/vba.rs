//! The VBA project of a workbook (MS-OVBA), read for the cell positions its
//! code may name. ARP keeps the project as it is: row and column edits do not
//! rewrite the addresses in the code, as Excel does not either. A workbook
//! with one takes them only once the person editing accepts that, and the
//! lines that may name cells are listed for them to check.
use super::*;
use regex::Regex;
use std::sync::LazyLock;

/// The lines a report lists; `references_total` counts them all.
const LISTED: usize = 20;

/// Calls taking cell addresses or positions, and `[A1]` shortcuts.
static CELL_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(Range|Cells|Rows|Columns|Offset|Resize|Evaluate|ExecuteExcel4Macro)\s*\(|\[\s*\$?[A-Z]{1,3}\$?[0-9]",
    )
    .unwrap()
});
/// An A1 address in text, as `Range("B5")` or a formula string holds one.
static ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[^A-Z0-9_$])\$?[A-Z]{1,3}\$?[1-9][0-9]{0,6}($|[^A-Z0-9_(])").unwrap()
});

/// Row or column operations on a workbook with a VBA project, recorded
/// without accepting that its code keeps the cell addresses it holds.
#[derive(Debug)]
pub struct VbaNotUpdated {
    pub operations: Vec<String>,
}

impl std::fmt::Display for VbaNotUpdated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "operation(s) {} insert, delete or move rows or columns of a workbook with a VBA project, and ARP does not rewrite the cell addresses in its code (nor does Excel); once the macros are checked against the moved cells, record the edit with --vba-unchanged",
            self.operations.join(", ")
        )
    }
}

impl std::error::Error for VbaNotUpdated {}

struct Module {
    name: String,
    source: String,
}

impl Workbook {
    fn vba_project(&self) -> Option<&[u8]> {
        self.parts
            .iter()
            .find(|(name, _)| name.to_ascii_lowercase().ends_with("vbaproject.bin"))
            .map(|(_, bytes)| bytes.as_slice())
    }

    pub(super) fn has_vba_project(&self) -> bool {
        self.vba_project().is_some()
    }

    /// The modules of the VBA project and the lines that may name cells, the
    /// first of them listed; `None` without a VBA project. A project that
    /// cannot be read reports why, as its code then goes unlisted.
    pub fn vba_report(&self) -> Option<Value> {
        let project = self.vba_project()?;
        Some(match modules(project) {
            Ok(modules) => report(&modules),
            Err(error) => json!({"unreadable":format!("{error:#}")}),
        })
    }
}

fn report(modules: &[Module]) -> Value {
    let mut listed = vec![];
    let mut total = 0;
    let mut summary = vec![];
    for module in modules {
        let mut count = 0;
        // Attribute lines are hidden in the VBA editor and left out of its
        // line numbers.
        let lines = module
            .source
            .lines()
            .filter(|line| !line.starts_with("Attribute "));
        for (index, line) in lines.enumerate() {
            if names_cells(line) {
                count += 1;
                if listed.len() < LISTED {
                    listed.push(json!({"module":module.name,"line":index + 1,"text":line.trim()}));
                }
            }
        }
        total += count;
        summary.push(json!({"name":module.name,"references":count}));
    }
    json!({"modules":summary,"references":listed,"references_total":total})
}

/// Whether a line of code may name cells by position. Comments are left out;
/// a position computed elsewhere goes unnoticed, so the list is a guide.
fn names_cells(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed
        .get(..3)
        .is_some_and(|word| word.eq_ignore_ascii_case("rem"))
        && trimmed[3..].chars().next().is_none_or(char::is_whitespace)
    {
        return false;
    }
    let mut code = String::new();
    let mut strings = vec![];
    let mut current: Option<String> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(text) = current.as_mut() {
            if c != '"' {
                text.push(c);
            } else if chars.peek() == Some(&'"') {
                chars.next();
                text.push('"');
            } else {
                strings.extend(current.take());
            }
        } else if c == '"' {
            current = Some(String::new());
        } else if c == '\'' {
            break;
        } else {
            code.push(c);
        }
    }
    strings.extend(current);
    CELL_CALL.is_match(&code) || strings.iter().any(|text| ADDRESS.is_match(text))
}

/// The modules of a VBA project storage, with their source code.
fn modules(project: &[u8]) -> Result<Vec<Module>> {
    let mut file = cfb::CompoundFile::open(Cursor::new(project))
        .context("the VBA project is not an OLE compound file")?;
    let dir = decompress(&stream(&mut file, "/VBA/dir")?)?;
    let mut code_page = 1252;
    let mut modules = vec![];
    let (mut name, mut stream_name, mut offset) = (None, None, None);
    let mut position = 0;
    while position + 6 <= dir.len() {
        let id = u16::from_le_bytes([dir[position], dir[position + 1]]);
        let size = u32::from_le_bytes(dir[position + 2..position + 6].try_into()?) as usize;
        // PROJECTVERSION gives 4 as its size and holds 6 bytes.
        let size = if id == 0x0009 { 6 } else { size };
        let data = dir
            .get(position + 6..position + 6 + size)
            .context("the VBA dir stream ends inside a record")?;
        position += 6 + size;
        match id {
            0x0003 => code_page = u16::from_le_bytes(data.try_into()?),
            0x0019 => name = Some(decode(data, code_page)),
            0x0047 => name = Some(utf16(data)),
            0x001A => stream_name = Some(decode(data, code_page)),
            0x0032 => stream_name = Some(utf16(data)),
            0x0031 => offset = Some(u32::from_le_bytes(data.try_into()?) as usize),
            0x002B => {
                let (Some(name), Some(stream_name), Some(offset)) =
                    (name.take(), stream_name.take(), offset.take())
                else {
                    bail!("a VBA module record lacks its name, stream or source offset");
                };
                let bytes = stream(&mut file, &format!("/VBA/{stream_name}"))?;
                let source = decompress(
                    bytes
                        .get(offset..)
                        .with_context(|| format!("module {name} starts past its stream"))?,
                )?;
                modules.push(Module {
                    name,
                    source: decode(&source, code_page),
                });
            }
            _ => {}
        }
    }
    Ok(modules)
}

fn stream(file: &mut cfb::CompoundFile<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>> {
    let mut bytes = vec![];
    file.open_stream(path)
        .with_context(|| format!("the VBA project has no {path} stream"))?
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A compressed container of MS-OVBA 2.4.1: chunks of up to 4096 bytes, each
/// stored as is or as literal bytes and copies of earlier bytes of the chunk.
fn decompress(container: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        container.first() == Some(&1),
        "a VBA compressed container starts with 1"
    );
    let mut out = vec![];
    let mut position = 1;
    while position + 2 <= container.len() {
        let header = u16::from_le_bytes([container[position], container[position + 1]]);
        ensure!(
            (header >> 12) & 0x7 == 0b011,
            "invalid VBA compressed chunk signature"
        );
        let end = (position + usize::from(header & 0x0FFF) + 3).min(container.len());
        let mut at = position + 2;
        let start = out.len();
        if header & 0x8000 == 0 {
            let raw = container
                .get(at..at + 4096)
                .context("an uncompressed VBA chunk holds 4096 bytes")?;
            out.extend_from_slice(raw);
        }
        while header & 0x8000 != 0 && at < end {
            let flags = container[at];
            at += 1;
            for bit in 0..8 {
                if at >= end {
                    break;
                }
                if flags & (1 << bit) == 0 {
                    out.push(container[at]);
                    at += 1;
                    continue;
                }
                let token = u16::from_le_bytes(
                    container
                        .get(at..at + 2)
                        .context("a VBA copy token is cut off")?
                        .try_into()?,
                );
                at += 2;
                let written = out.len() - start;
                let mut bits = 4;
                while (1 << bits) < written {
                    bits += 1;
                }
                let length = usize::from(token & (0xFFFF >> bits)) + 3;
                let distance = usize::from(token >> (16 - bits)) + 1;
                ensure!(
                    distance <= written,
                    "a VBA copy token reaches before its chunk"
                );
                for _ in 0..length {
                    out.push(out[out.len() - distance]);
                }
            }
        }
        position = end;
    }
    Ok(out)
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Text in the project's Windows code page.
fn decode(bytes: &[u8], code_page: u16) -> String {
    use encoding_rs::*;
    let encoding = match code_page {
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        874 => WINDOWS_874,
        866 => IBM866,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        10000 => MACINTOSH,
        20932 | 51932 => EUC_JP,
        54936 => GB18030,
        65001 => UTF_8,
        _ => WINDOWS_1252,
    };
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompresses_the_ms_ovba_example() {
        let compressed = [
            0x01, 0x2F, 0xB0, 0x00, 0x23, 0x61, 0x61, 0x61, 0x62, 0x63, 0x64, 0x65, 0x82, 0x66,
            0x00, 0x70, 0x61, 0x67, 0x68, 0x69, 0x6A, 0x01, 0x38, 0x08, 0x61, 0x6B, 0x6C, 0x00,
            0x30, 0x6D, 0x6E, 0x6F, 0x70, 0x06, 0x71, 0x02, 0x70, 0x04, 0x10, 0x72, 0x73, 0x74,
            0x75, 0x76, 0x10, 0x77, 0x78, 0x79, 0x7A, 0x00, 0x3C,
        ];
        assert_eq!(
            String::from_utf8(decompress(&compressed).unwrap()).unwrap(),
            "#aaabcdefaaaaghijaaaaaklaaamnopqaaaaaaaaaaaarstuvwxyzaaa"
        );
        assert!(decompress(&[0x01, 0x03, 0xB0, 0x02, 0x01, 0x00]).is_err());
    }

    #[test]
    fn lines_naming_cells_are_told_from_others() {
        for line in [
            r#"Range("B5").Value = 1"#,
            "Cells(i + 4, 3) = x",
            r#"ws.Range("A" & lastRow).Select"#,
            "Set r = target.Offset(1, 0)",
            "[C3] = 2",
            r#"ActiveCell.Formula = "=SUM($D$2:D4)""#,
            r#"Worksheets("課題").Rows(5).Delete"#,
        ] {
            assert!(names_cells(line), "{line}");
        }
        for line in [
            "' Range(\"B5\") is the total",
            "Rem Cells(1, 1)",
            r#"MsgBox "Sheet1 を更新しました""#,
            "x = Ranges + 1",
            r#"Worksheets("Sheet1").Activate"#,
            r#"Call Version2("ok")"#,
        ] {
            assert!(!names_cells(line), "{line}");
        }
    }
}
