//! Format adapters for the reviewed document workflow.
//!
//! The existing mapping contract uses sheets/cells. For text formats these are
//! logical text containers and A1, A2, ... are stable text-run ordinals, not
//! physical spreadsheet coordinates. `part` identifies the original container.
//! Native text carries original byte/line positions within the hashed revision;
//! its derived views are read-only and edits happen in the original file.
use crate::{data::*, excel::Workbook};
use anyhow::{Context, Result, bail, ensure};
use lopdf::{Document as Pdf, Object, Stream, content::Content};
use roxmltree::{Document, Node};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::Path,
};

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_PAGE: usize = 32 * 1024 * 1024;

pub enum Source {
    Excel(Workbook),
    Text(TextSource),
}

pub struct TextSource {
    raw: Vec<u8>,
    sheets: Vec<Value>,
    format: String,
    backend: TextBackend,
}

enum TextBackend {
    Office(BTreeMap<String, Vec<u8>>),
    Pdf(Box<Pdf>),
    Native,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if matches!(format.as_str(), "xlsx" | "xlsm") {
            return Ok(Self::Excel(Workbook::open(path)?));
        }
        ensure!(
            matches!(
                format.as_str(),
                "pdf" | "docx" | "pptx" | "txt" | "md" | "csv" | "tsv"
            ),
            "unsupported document format: {format}"
        );
        ensure!(
            fs::metadata(path)?.len() <= MAX_BYTES as u64,
            "document exceeds size budget"
        );
        let native = matches!(format.as_str(), "txt" | "md" | "csv" | "tsv");
        if native {
            ensure!(
                fs::metadata(path)?.len() <= MAX_PAGE as u64,
                "text document exceeds 32 MiB size budget"
            );
        }
        let raw = fs::read(path)?;
        let (backend, sheets) = if native {
            let text = std::str::from_utf8(&raw)
                .context("text documents require UTF-8 (optional BOM); convert the original explicitly before import")?;
            ensure!(
                !text.contains('\0'),
                "text document contains NUL; binary/UTF-16 input is not supported"
            );
            let sheet = match format.as_str() {
                "md" => crate::native_text::markdown(text)?,
                "csv" => crate::native_text::delimited(text, b',')?,
                "tsv" => crate::native_text::delimited(text, b'\t')?,
                _ => crate::native_text::lines(text)?,
            };
            (TextBackend::Native, vec![sheet])
        } else if format == "pdf" {
            let doc = Pdf::load_mem(&raw)?;
            ensure!(
                !doc.is_encrypted() && doc.trailer.get(b"Encrypt").is_err(),
                "encrypted PDF is not supported"
            );
            let mut sheets = vec![];
            ensure!(
                doc.get_pages().len() <= 10000,
                "PDF page count exceeds budget"
            );
            for (page, id) in doc.get_pages() {
                let mut content = Content::decode(&doc.get_page_content_with_limit(id, MAX_PAGE)?)?;
                let values = pdf_text(&doc, id, &mut content, &BTreeMap::new())?;
                sheets.push(text_sheet(
                    &format!("page-{page}"),
                    &format!("pdf:{page}"),
                    values,
                ));
            }
            (TextBackend::Pdf(Box::new(doc)), sheets)
        } else {
            let parts = office_parts(&raw)?;
            let containers = office_containers(&parts, &format)?;
            let mut sheets = vec![];
            for (name, part) in containers {
                let xml = Document::parse(std::str::from_utf8(
                    parts.get(&part).context("missing document part")?,
                )?)?;
                let values = xml
                    .descendants()
                    .filter(|n| text_node(*n, &format))
                    .map(|n| n.text().unwrap_or("").to_owned())
                    .collect();
                sheets.push(text_sheet(&name, &part, values));
            }
            (TextBackend::Office(parts), sheets)
        };
        ensure!(
            !sheets.is_empty(),
            "document has no pages or text containers"
        );
        Ok(Self::Text(TextSource {
            raw,
            sheets,
            format,
            backend,
        }))
    }

    pub fn raw(&self) -> &[u8] {
        match self {
            Self::Excel(book) => &book.raw,
            Self::Text(doc) => &doc.raw,
        }
    }

    pub fn sheets(&self) -> &[Value] {
        match self {
            Self::Excel(book) => &book.sheets,
            Self::Text(doc) => &doc.sheets,
        }
    }

    pub fn engine(&self) -> &'static str {
        match self {
            Self::Excel(_) => "xml",
            Self::Text(doc) if matches!(doc.backend, TextBackend::Native) => "text",
            Self::Text(doc) if doc.format == "pdf" => "pdf",
            _ => "xml",
        }
    }

    pub fn parser(&self) -> &'static str {
        match self {
            Self::Excel(_) => "cells/1",
            Self::Text(doc) if matches!(doc.backend, TextBackend::Native) => "native-text/1",
            Self::Text(_) => "text-runs/1",
        }
    }

    pub fn note(&self) -> &'static str {
        match self {
            Self::Text(doc) if doc.format == "md" => {
                "Markdownの見出し階層・段落・リスト・表・コード等を原文のままブロック単位で抽出します。textのA1等はブロック番号です。positionの行範囲・UTF-8バイト範囲と原本ハッシュで出典を確認してください。リンク先・画像は読み込みません。本文は原本を直接編集し、同じ文書IDで再取込してください。確認用YAMLの本文変更・export/applyは未対応です。"
            }
            Self::Text(doc) if matches!(doc.format.as_str(), "csv" | "tsv") => {
                "CSV/TSVを全項目文字列としてレコード・列単位で抽出します。先頭行も通常のレコードで、ヘッダーや型を推測しません。引用符内の改行・区切り文字、先頭ゼロ、空欄を保持します。recordsのA1等は列・レコード番号で、物理行番号ではありません。positionは原本の引用符を含むフィールドの行・UTF-8バイト範囲です。引用符は構文として復号します。原本を直接編集して再取込してください。YAML本文変更・export/applyは未対応です。"
            }
            Self::Text(doc) if matches!(doc.backend, TextBackend::Native) => {
                "UTF-8テキストを空行を含め行単位で抽出します。textのA1等は原本の行番号です。positionの行・UTF-8バイト範囲と原本ハッシュで出典を確認してください。原本を直接編集して同じ文書IDで再取込してください。確認用YAMLの本文変更・export/applyは未対応です。"
            }
            Self::Excel(_) => {
                "セル値・数式原文・結合範囲・書式の識別情報・Excelテーブル定義とDrawingMLの図形文字・配置・明示的な接続を抽出します。埋込み画像はassetsに保存します。表・見出しの推定はstructure initで行い、レビューが必要です。グループ内位置は変換情報を保持し、接続のない矢印の意味、コメント・印刷情報・OCR・グラフ内部の内容は未抽出です。"
            }
            Self::Text(doc) if doc.format == "pdf" => {
                "ページのテキスト描画命令を文字列単位で抽出します。A1等は文字列の通し番号です。画像・OCR・フォーム・注釈・Form XObject内の文字は未抽出です。元フォントで表現可能な文字だけ書き戻せます。自動改行・再配置はしないため、文字の重なりやはみ出しを出力PDFで確認してください。"
            }
            Self::Text(doc) if doc.format == "docx" => {
                "Word本文・表・ヘッダー・フッター・脚注・文末脚注の文字列をrun単位で抽出します。A1等は文字列の通し番号です。書式境界は別行です。画像・OCR・フィールド命令・コメントは未抽出です。ページは再組版されるため出力Wordの表示を確認してください。"
            }
            _ => {
                "PPTXの各スライドの本文・表をrun単位で抽出します。A1等は文字列の通し番号です。書式境界は別行です。ノート・マスター・画像・OCR・グラフ内部の文字は未抽出です。自動サイズ調整を行わないため、出力PPTXではみ出しを確認してください。"
            }
        }
    }

    pub fn patch(
        &self,
        output: &Path,
        operations: &[Value],
        changes: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        match self {
            Self::Excel(book) => {
                book.patch_with_operations_and_assets(output, operations, changes, assets)
            }
            Self::Text(doc) => {
                ensure!(
                    operations.is_empty() && assets.is_empty(),
                    "text formats support existing text edits only; structural/image operations are Excel-only"
                );
                doc.patch(output, changes)
            }
        }
    }
}

fn text_sheet(name: &str, part: &str, texts: Vec<String>) -> Value {
    let cells: Vec<_> = texts
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            json!({
                "id":format!("text-{}",i+1), "address":format!("A{}",i+1),
                "type":"string", "value":value, "formula":null, "cached":null, "number_format":""
            })
        })
        .collect();
    json!({"name":name,"part":part,"state":"visible","merges":[],"cells":cells})
}

fn office_parts(raw: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(raw))?;
    ensure!(
        zip.len() <= 10000 && zip.decompressed_size().unwrap_or(u128::MAX) <= MAX_BYTES as u128,
        "Office archive exceeds size budget"
    );
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_owned();
        ensure!(
            entry.enclosed_name().is_some() && !name.contains('\\'),
            "invalid Office part path"
        );
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        ensure!(parts.insert(name, bytes).is_none(), "duplicate Office part");
    }
    Ok(parts)
}

fn xml_part<'a>(parts: &'a BTreeMap<String, Vec<u8>>, part: &str) -> Result<Document<'a>> {
    Ok(Document::parse(std::str::from_utf8(
        parts
            .get(part)
            .with_context(|| format!("missing Office part: {part}"))?,
    )?)?)
}

fn relation_target(parts: &BTreeMap<String, Vec<u8>>, part: &str, id: &str) -> Result<String> {
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let relationships = if directory.is_empty() {
        format!("_rels/{filename}.rels")
    } else {
        format!("{directory}/_rels/{filename}.rels")
    };
    let xml = xml_part(parts, &relationships)?;
    let rel = xml
        .descendants()
        .find(|n| n.has_tag_name((PACKAGE_REL, "Relationship")) && n.attribute("Id") == Some(id))
        .context("missing Office relationship")?;
    ensure!(
        rel.attribute("TargetMode") != Some("External"),
        "external document part is not supported"
    );
    let target = rel
        .attribute("Target")
        .context("relationship target missing")?;
    ensure!(
        !target.contains(['\\', ':', '#', '?', '%']),
        "unsupported Office part target"
    );
    let joined = if target.starts_with('/') {
        target.trim_start_matches('/').to_owned()
    } else if directory.is_empty() {
        target.to_owned()
    } else {
        format!("{directory}/{target}")
    };
    let mut normalized = vec![];
    for component in joined.split('/') {
        match component {
            ".." => {
                ensure!(normalized.pop().is_some(), "Office target escapes package");
            }
            "." | "" => {}
            _ => normalized.push(component),
        }
    }
    Ok(normalized.join("/"))
}

fn office_containers(
    parts: &BTreeMap<String, Vec<u8>>,
    format: &str,
) -> Result<Vec<(String, String)>> {
    let roots = xml_part(parts, "_rels/.rels")?;
    let root = roots
        .descendants()
        .find(|n| {
            n.has_tag_name((PACKAGE_REL, "Relationship"))
                && n.attribute("Type")
                    .is_some_and(|v| v == format!("{REL}/officeDocument"))
        })
        .context("Office document relationship missing")?;
    let main = relation_target(
        parts,
        "",
        root.attribute("Id").context("missing relationship id")?,
    )?;
    let xml = xml_part(parts, &main)?;
    if format == "docx" {
        ensure!(
            xml.root_element().has_tag_name((WORD, "document")),
            "not a supported Word document"
        );
        let mut result = vec![("document".into(), main.clone())];
        let (directory, filename) = main.rsplit_once('/').unwrap_or(("", &main));
        let rel_path = format!("{directory}/_rels/{filename}.rels");
        if parts.contains_key(&rel_path) {
            let rels = xml_part(parts, &rel_path)?;
            for rel in rels
                .descendants()
                .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
            {
                let kind = rel
                    .attribute("Type")
                    .unwrap_or("")
                    .strip_prefix(&format!("{REL}/"))
                    .unwrap_or("");
                if matches!(kind, "header" | "footer" | "footnotes" | "endnotes") {
                    let part = relation_target(
                        parts,
                        &main,
                        rel.attribute("Id").context("missing relationship id")?,
                    )?;
                    if !result.iter().any(|(_, p)| p == &part) {
                        result.push((format!("{kind}-{}", result.len()), part));
                    }
                }
            }
        }
        Ok(result)
    } else {
        const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
        ensure!(
            xml.root_element()
                .has_tag_name((PRESENTATION, "presentation")),
            "not a supported presentation"
        );
        xml.descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "sldId")))
            .enumerate()
            .map(|(i, n)| {
                Ok((
                    format!("slide-{}", i + 1),
                    relation_target(
                        parts,
                        &main,
                        n.attribute((REL, "id"))
                            .context("missing slide relationship")?,
                    )?,
                ))
            })
            .collect()
    }
}

fn text_node(node: Node<'_, '_>, format: &str) -> bool {
    node.has_tag_name((if format == "docx" { WORD } else { DRAWING }, "t"))
}

fn xml_text(value: &str) -> Result<String> {
    ensure!(value.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')), "invalid XML character in replacement");
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;"))
}

impl TextSource {
    fn patch(&self, output: &Path, changes: &[Value]) -> Result<Value> {
        ensure!(
            !matches!(self.backend, TextBackend::Native),
            "text documents use direct editing; edit the original and re-import with the same document ID (export/apply is not supported)"
        );
        ensure!(!output.exists(), "output already exists");
        let mut replacements: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        for change in changes {
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == change["sheet"])
                .context("missing text container")?;
            let cell = array(&sheet["cells"])?
                .iter()
                .position(|c| c["address"] == change["cell"])
                .context("missing text target")?;
            ensure!(
                sheet["cells"][cell]["value"] == change["before"],
                "text baseline mismatch"
            );
            let value = string(&change["after"])
                .context("text replacement must be a string; use an empty string to clear text")?;
            if self.format != "pdf" {
                ensure!(
                    !value.contains(['\n', '\r', '\t']),
                    "Office text replacement cannot contain line breaks or tabs; edit existing runs separately"
                );
            }
            ensure!(
                replacements
                    .entry(string(&sheet["part"])?.into())
                    .or_default()
                    .insert(cell, value.into())
                    .is_none(),
                "duplicate text replacement"
            );
        }
        match &self.backend {
            TextBackend::Native => unreachable!("native text writeback rejected above"),
            TextBackend::Office(parts) => {
                ensure!(
                    !parts
                        .keys()
                        .any(|p| p.to_ascii_lowercase().starts_with("_xmlsignatures/")),
                    "signed Office writeback is not supported"
                );
                let mut patched = BTreeMap::new();
                for (part, changes) in replacements {
                    let original = std::str::from_utf8(&parts[&part])?;
                    let xml = Document::parse(original)?;
                    let nodes: Vec<_> = xml
                        .descendants()
                        .filter(|n| text_node(*n, &self.format))
                        .collect();
                    let mut result = original.to_owned();
                    for (i, text) in changes.into_iter().rev() {
                        let node = nodes.get(i).context("text node disappeared")?;
                        let raw = &original[node.range()];
                        let end = raw.find('>').context("invalid text element")?;
                        let opening = &raw[..end];
                        let name = opening
                            .trim_start_matches('<')
                            .split(|c: char| c.is_whitespace() || c == '/')
                            .next()
                            .context("missing text tag")?;
                        // Preserve attributes, enforcing whitespace preservation for Word.
                        let mut opening = opening.trim_end_matches('/').to_owned();
                        if self.format == "docx" {
                            if let Some(attr) = node.attributes().find(|a| {
                                a.namespace() == Some("http://www.w3.org/XML/1998/namespace")
                                    && a.name() == "space"
                            }) {
                                let range = attr.range();
                                opening.replace_range(
                                    range.start - node.range().start
                                        ..range.end - node.range().start,
                                    "xml:space=\"preserve\"",
                                );
                            } else {
                                opening.push_str(" xml:space=\"preserve\"");
                            }
                        }
                        let replacement = format!("{opening}>{}</{name}>", xml_text(&text)?);
                        result.replace_range(node.range(), &replacement);
                    }
                    Document::parse(&result)?;
                    patched.insert(part, result.into_bytes());
                }
                crate::excel::write_archive(&self.raw, output, &patched)?;
            }
            TextBackend::Pdf(original) => {
                ensure!(
                    !original
                        .objects
                        .values()
                        .any(|o| o.as_dict().is_ok_and(|d| d.get(b"ByteRange").is_ok()
                            || d.get(b"Type")
                                .and_then(Object::as_name)
                                .is_ok_and(|v| v == b"Sig"))),
                    "signed PDF writeback is not supported"
                );
                let mut doc = original.clone();
                for (page, id) in original.get_pages() {
                    if let Some(changes) = replacements.get(&format!("pdf:{page}")) {
                        let mut content =
                            Content::decode(&original.get_page_content_with_limit(id, MAX_PAGE)?)?;
                        pdf_text(original, id, &mut content, changes)?;
                        // Always allocate a new stream: an original stream may be shared by pages.
                        let stream = doc
                            .add_object(Stream::new(lopdf::Dictionary::new(), content.encode()?));
                        doc.get_object_mut(id)?
                            .as_dict_mut()?
                            .set("Contents", Object::Reference(stream));
                    }
                }
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)?;
                doc.save_to(&mut file)?;
                file.sync_all()?;
            }
        }
        Ok(json!({"format":self.format,"text_changes":changes.len(),"layout_review_required":true}))
    }
}

enum PdfEncoding<'a> {
    Mapped(lopdf::Encoding<'a>),
    Unicode { ucs2: bool },
}

impl<'a> PdfEncoding<'a> {
    fn for_font(font: &'a lopdf::Dictionary, doc: &'a Pdf) -> Result<Self> {
        let name = font
            .get_deref(b"Encoding", doc)
            .and_then(Object::as_name)
            .unwrap_or(b"");
        // Predefined Unicode CMaps use big-endian character codes without a BOM.
        // Identity-H/V are CID codes, NOT Unicode; they require a ToUnicode map.
        if matches!(
            name,
            b"UniJIS-UCS2-H"
                | b"UniJIS-UCS2-V"
                | b"UniGB-UCS2-H"
                | b"UniGB-UCS2-V"
                | b"UniCNS-UCS2-H"
                | b"UniCNS-UCS2-V"
                | b"UniKS-UCS2-H"
                | b"UniKS-UCS2-V"
        ) {
            return Ok(Self::Unicode { ucs2: true });
        }
        if matches!(
            name,
            b"UniJIS-UTF16-H"
                | b"UniJIS-UTF16-V"
                | b"UniGB-UTF16-H"
                | b"UniGB-UTF16-V"
                | b"UniCNS-UTF16-H"
                | b"UniCNS-UTF16-V"
                | b"UniKS-UTF16-H"
                | b"UniKS-UTF16-V"
        ) {
            return Ok(Self::Unicode { ucs2: false });
        }
        let encoding = font.get_font_encoding_with_limit(doc, MAX_PAGE)?;
        if font
            .get(b"Subtype")
            .and_then(Object::as_name)
            .is_ok_and(|n| n == b"Type0")
        {
            ensure!(
                matches!(encoding, lopdf::Encoding::UnicodeMapEncoding(_)),
                "PDF composite font requires a supported Unicode CMap or valid ToUnicode map"
            );
        }
        Ok(Self::Mapped(encoding))
    }

    fn decode(&self, bytes: &[u8]) -> Result<String> {
        match self {
            Self::Mapped(encoding) => Ok(Pdf::decode_text(encoding, bytes)?),
            Self::Unicode { ucs2 } => {
                ensure!(
                    bytes.len().is_multiple_of(2),
                    "invalid PDF Unicode byte length"
                );
                let units: Vec<_> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| u16::from_be_bytes([b[0], b[1]]))
                    .collect();
                ensure!(
                    !ucs2 || units.iter().all(|u| !(0xd800..=0xdfff).contains(u)),
                    "surrogate in PDF UCS2 text"
                );
                Ok(String::from_utf16(&units)?)
            }
        }
    }

    fn encode(&self, text: &str) -> Result<Vec<u8>> {
        match self {
            Self::Mapped(encoding) => Ok(Pdf::encode_text(encoding, text)),
            Self::Unicode { ucs2 } => {
                ensure!(
                    !ucs2 || text.chars().all(|c| c as u32 <= 0xffff),
                    "replacement contains characters unavailable in PDF UCS2 encoding"
                );
                Ok(text.encode_utf16().flat_map(u16::to_be_bytes).collect())
            }
        }
    }
}

fn pdf_text(
    doc: &Pdf,
    page: lopdf::ObjectId,
    content: &mut Content<Vec<lopdf::content::Operation>>,
    changes: &BTreeMap<usize, String>,
) -> Result<Vec<String>> {
    let fonts = doc.get_page_fonts(page)?;
    let mut font = Vec::new();
    let mut stack = vec![];
    let mut values = vec![];
    for operation in &mut content.operations {
        match operation.operator.as_str() {
            "Tf" => {
                font = operation
                    .operands
                    .first()
                    .context("missing PDF font")?
                    .as_name()?
                    .to_vec();
            }
            "q" => stack.push(font.clone()),
            "Q" => {
                font = stack.pop().context("unbalanced PDF graphics state")?;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                let operand = operation
                    .operands
                    .last_mut()
                    .context("missing PDF text operand")?;
                let strings: Vec<&mut Object> = match operand {
                    Object::Array(items) => items
                        .iter_mut()
                        .filter(|o| matches!(o, Object::String(..)))
                        .collect(),
                    Object::String(..) => vec![operand],
                    _ => bail!("invalid PDF text operand"),
                };
                let encoding = PdfEncoding::for_font(
                    fonts.get(&font).context("PDF text font not found")?,
                    doc,
                )?;
                for object in strings {
                    let Object::String(bytes, _) = object else {
                        unreachable!()
                    };
                    let value = encoding
                        .decode(bytes)
                        .context("PDF font encoding cannot be decoded")?;
                    if let Some(replacement) = changes.get(&values.len()) {
                        ensure!(
                            !replacement.contains(['\n', '\r', '\t']),
                            "PDF text replacement cannot contain line breaks or tabs; layout operations are not supported"
                        );
                        let encoded = encoding.encode(replacement)?;
                        ensure!(
                            encoding.decode(&encoded)? == *replacement,
                            "replacement contains characters unavailable in the original PDF font encoding"
                        );
                        *bytes = encoded;
                    }
                    values.push(value);
                }
            }
            _ => {}
        }
    }
    ensure!(
        changes.keys().all(|i| *i < values.len()),
        "PDF text target missing"
    );
    Ok(values)
}
