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
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Cursor, Read},
    path::Path,
};

mod word;

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const MATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PACKAGE_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const MARKUP_COMPATIBILITY: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const STRICT_REL: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_PAGE: usize = 32 * 1024 * 1024;

/// Excel packages read as workbooks; templates share the workbook body.
pub const EXCEL_FORMATS: &[&str] = &["xlsx", "xlsm", "xltx", "xltm"];
/// Word packages sharing the WordprocessingML body. VBA and template parts are
/// never read and are copied unchanged on writeback.
pub const WORD_FORMATS: &[&str] = &["docx", "docm", "dotx", "dotm"];
const NATIVE_FORMATS: &[&str] = &["txt", "md", "csv", "tsv"];

/// Formats whose extraction supports document structure interpretation.
pub fn structure_formats() -> Vec<&'static str> {
    [EXCEL_FORMATS, WORD_FORMATS, &["pptx", "pdf"]].concat()
}

pub fn input_formats() -> Vec<&'static str> {
    [structure_formats().as_slice(), NATIVE_FORMATS].concat()
}

pub fn is_excel(format: &str) -> bool {
    EXCEL_FORMATS.contains(&format)
}

/// The Open XML formats to resave a binary Office file as, which ARP does not parse.
pub fn binary_office_replacement(format: &str) -> Option<&'static str> {
    match format {
        "xls" | "xlsb" | "xlt" => Some(".xlsx/.xlsm"),
        "doc" | "dot" => Some(".docx/.docm"),
        "ppt" | "pot" | "pps" => Some(".pptx"),
        _ => None,
    }
}

/// Office writes encrypted packages (password, IRM, sensitivity labels) and the
/// binary formats as OLE compound files rather than ZIP packages.
pub(crate) fn ensure_zip_package(raw: &[u8]) -> Result<()> {
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
    /// `unparsed` lists pages whose content stream could not be read to its end;
    /// their text after that point is missing and they are not written back.
    Pdf {
        doc: Box<Pdf>,
        unparsed: Vec<u32>,
    },
    Native,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if is_excel(&format) {
            return Ok(Self::Excel(Workbook::open(path)?));
        }
        ensure!(
            input_formats().contains(&format.as_str()),
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
            let mut unparsed = vec![];
            ensure!(
                doc.get_pages().len() <= 10000,
                "PDF page count exceeds budget"
            );
            for (page, id) in doc.get_pages() {
                let raw_content = doc.get_page_content_with_limit(id, MAX_PAGE)?;
                // The lenient decoder stops quietly at the first token it cannot
                // read; keep what it read but report the page.
                let mut content = match Content::decode_strict(&raw_content) {
                    Ok(content) => content,
                    Err(_) => {
                        unparsed.push(page);
                        Content::decode(&raw_content)?
                    }
                };
                let values = pdf_text(&doc, id, &mut content, &BTreeMap::new(), None)?;
                sheets.push(text_sheet(
                    &format!("page-{page}"),
                    &format!("pdf:{page}"),
                    values,
                ));
            }
            (
                TextBackend::Pdf {
                    doc: Box::new(doc),
                    unparsed,
                },
                sheets,
            )
        } else {
            let parts = office_parts(&raw)?;
            let containers = office_containers(&parts, &format)?;
            let mut sheets = vec![];
            for (name, part) in containers {
                let (text, _) = part_text(parts.get(&part).context("missing document part")?)?;
                let xml = Document::parse(&text)?;
                if word(&format) {
                    sheets.push(word::layout(&xml)?.sheet(&name, &part));
                } else {
                    let values = text_nodes(&xml, &format)
                        .into_iter()
                        .map(|n| n.text().unwrap_or("").to_owned())
                        .collect();
                    let mut sheet = text_sheet(&name, &part, values);
                    // A slide hidden from the show is marked like a hidden sheet.
                    if xml.root_element().attribute("show") == Some("0") {
                        sheet["state"] = json!("hidden");
                    }
                    sheets.push(sheet);
                }
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

    /// The sheets as the extraction records them. A workbook builds them from
    /// its typed cells on each call.
    pub fn sheets(&self) -> Cow<'_, [Value]> {
        match self {
            Self::Excel(book) => Cow::Owned(book.sheet_values()),
            Self::Text(doc) => Cow::Borrowed(&doc.sheets),
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
            Self::Text(doc) if word(&doc.format) => "word-blocks/1",
            Self::Text(_) => "text-runs/1",
        }
    }

    pub fn note(&self) -> String {
        let mut note = self.skipped_note();
        if let Self::Excel(book) = self
            && book.date1904
        {
            note.push_str("このブックは1904年日付系です。日付・時刻のセル値は1904年1月1日を0とするシリアル値で、1900年日付系より1462日小さい値です。");
        }
        if let Self::Excel(book) = self
            && !book.rich_values.is_empty()
        {
            note.push_str(&format!(
                "次のセルはセル内の画像またはリンクされたデータ型（株価・地理など）で、値はセル外に保存されているため抽出していません（セルの値は#VALUE!と記録されます）: {}。",
                book.rich_values.join("、")
            ));
        }
        if let Self::Text(TextSource {
            backend: TextBackend::Pdf { unparsed, .. },
            ..
        }) = self
            && !unparsed.is_empty()
        {
            let pages = unparsed
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("、");
            note.push_str(&format!("次のページは描画命令を途中までしか解析できず、それ以降の文字を抽出していません。書き戻しもできません: {pages}ページ。"));
        }
        note
    }

    fn skipped_note(&self) -> String {
        let base = self.base_note();
        match self {
            Self::Excel(book) if !book.skipped_sheets.is_empty() => {
                let sheets = book
                    .skipped_sheets
                    .iter()
                    .map(|s| {
                        let kind = match s["kind"].as_str() {
                            Some("chartsheet") => "グラフシート",
                            Some("dialogsheet") => "ダイアログシート",
                            _ => "マクロシート",
                        };
                        format!("{}「{}」", kind, s["name"].as_str().unwrap_or(""))
                    })
                    .collect::<Vec<_>>()
                    .join("、");
                format!(
                    "{base}次のシートはセルを持つワークシートではないため抽出していません: {sheets}。書き戻しでは変更せず原本の部品を保持します。"
                )
            }
            _ => base.to_owned(),
        }
    }

    fn base_note(&self) -> &'static str {
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
                "セル値・数式原文・結合範囲・書式の識別情報・Excelテーブル定義とDrawingMLの図形文字・配置・明示的な接続を抽出します。埋込み画像はassetsに保存し、WindowsではOCRを自動実行して結果を記録します。表・見出しの推定はstructure initで行い、レビューが必要です。グループ内位置は変換情報を保持します。メモ・スレッドコメント、図形に割り当てたマクロ、フォームコントロールの種類・リンク先セル・選択肢範囲も抽出します。接続のない矢印の意味、ActiveXコントロールの設定、印刷情報、グラフ内部の内容は未抽出です。"
            }
            Self::Text(doc) if doc.format == "pdf" => {
                "ページのテキスト描画命令を文字列単位で抽出します。A1等は文字列の通し番号です。画像・OCR・フォーム・注釈・Form XObject内の文字は未抽出です。元フォントで表現可能な文字だけ書き戻せます。自動改行・再配置はしないため、文字の重なりやはみ出しを出力PDFで確認してください。"
            }
            Self::Text(doc) if word(&doc.format) => {
                "Word本文・表・テキストボックス・ヘッダー・フッター・脚注・文末脚注・コメントを段落と表のセル単位で抽出します。表の外の段落は1段落1行でA列に並び、表は行・列の位置を保ち、結合セルをmerges、表の範囲をtablesに記録します。見出し行はWordの見出し行の繰り返し、太字・網掛けの先頭行、3列以上の表の先頭行から推定するため、構造のレビューで確認してください。セル内の段落・改行は改行、タブはタブ文字で表し、入れ子の表は外側のセルの本文に含めます。書き戻しは変更箇所を含むrunだけを書き換え、段落・改行・タブをまたぐ変更は拒否します。テキストボックスは表示される1組だけを抽出し、書き戻しでは互換用の複製（VML）も同じ文字にします。画像・OCR・フィールド命令は未抽出です。フィールドの表示結果（日付・ページ番号・目次・相互参照等）はWordが再計算するため書き戻しを拒否します。ページは再組版されるため出力Wordの表示を確認してください。"
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
    ensure_zip_package(raw)?;
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

/// Encodings an XML part may use; UTF-16 requires a byte order mark (XML 1.0 4.3.3).
#[derive(Clone, Copy)]
enum PartEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn part_text(bytes: &[u8]) -> Result<(Cow<'_, str>, PartEncoding)> {
    let utf16 = |body: &[u8], unit: fn([u8; 2]) -> u16| -> Result<String> {
        let (pairs, rest) = body.as_chunks::<2>();
        ensure!(rest.is_empty(), "truncated UTF-16 Office part");
        Ok(String::from_utf16(
            &pairs.iter().map(|pair| unit(*pair)).collect::<Vec<_>>(),
        )?)
    };
    Ok(match bytes {
        [0xFF, 0xFE, body @ ..] => (
            Cow::Owned(utf16(body, u16::from_le_bytes)?),
            PartEncoding::Utf16Le,
        ),
        [0xFE, 0xFF, body @ ..] => (
            Cow::Owned(utf16(body, u16::from_be_bytes)?),
            PartEncoding::Utf16Be,
        ),
        _ => (
            Cow::Borrowed(std::str::from_utf8(
                bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes),
            )?),
            PartEncoding::Utf8,
        ),
    })
}

/// Encodes a patched part like the original. The UTF-8 BOM is optional and dropped.
fn encode_part(text: &str, encoding: PartEncoding) -> Vec<u8> {
    let units = |order: fn(u16) -> [u8; 2], mark: [u8; 2]| -> Vec<u8> {
        mark.into_iter()
            .chain(text.encode_utf16().flat_map(order))
            .collect()
    };
    match encoding {
        PartEncoding::Utf8 => text.as_bytes().to_vec(),
        PartEncoding::Utf16Le => units(u16::to_le_bytes, [0xFF, 0xFE]),
        PartEncoding::Utf16Be => units(u16::to_be_bytes, [0xFE, 0xFF]),
    }
}

fn xml_part<'a>(parts: &'a BTreeMap<String, Vec<u8>>, part: &str) -> Result<Cow<'a, str>> {
    Ok(part_text(
        parts
            .get(part)
            .with_context(|| format!("missing Office part: {part}"))?,
    )?
    .0)
}

fn relation_target(parts: &BTreeMap<String, Vec<u8>>, part: &str, id: &str) -> Result<String> {
    let (directory, filename) = part.rsplit_once('/').unwrap_or(("", part));
    let relationships = if directory.is_empty() {
        format!("_rels/{filename}.rels")
    } else {
        format!("{directory}/_rels/{filename}.rels")
    };
    let text = xml_part(parts, &relationships)?;
    let xml = Document::parse(&text)?;
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
        !target.contains(['\\', ':', '#', '?']),
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
    let literal = normalized.join("/");
    if parts.contains_key(&literal) || !literal.contains('%') {
        return Ok(literal);
    }
    // Targets are URIs: tools that write spaced or non-ASCII part names
    // percent-encode them in the target while the ZIP entry holds the name.
    Ok(normalized
        .iter()
        .map(|segment| percent_decode(segment))
        .collect::<Result<Vec<_>>>()?
        .join("/"))
}

fn percent_decode(segment: &str) -> Result<String> {
    let bytes = segment.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = segment
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .context("invalid percent-encoding in Office part target")?;
            output.push(byte);
            i += 3;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(output)?;
    ensure!(
        !decoded.contains(['/', '\\', '\0']) && decoded != ".." && decoded != ".",
        "unsupported Office part target"
    );
    Ok(decoded)
}

fn office_containers(
    parts: &BTreeMap<String, Vec<u8>>,
    format: &str,
) -> Result<Vec<(String, String)>> {
    let roots_text = xml_part(parts, "_rels/.rels")?;
    let roots = Document::parse(&roots_text)?;
    let main_relationship = |namespace: &str| {
        roots.descendants().find(|n| {
            n.has_tag_name((PACKAGE_REL, "Relationship"))
                && n.attribute("Type")
                    .is_some_and(|v| v == format!("{namespace}/officeDocument"))
        })
    };
    ensure!(
        main_relationship(STRICT_REL).is_none(),
        "Strict Open XML documents are not supported; save the file in Office as a standard Open XML document and import that copy"
    );
    let root = main_relationship(REL).context("Office document relationship missing")?;
    let main = relation_target(
        parts,
        "",
        root.attribute("Id").context("missing relationship id")?,
    )?;
    let main_text = xml_part(parts, &main)?;
    let xml = Document::parse(&main_text)?;
    if word(format) {
        ensure!(
            xml.root_element().has_tag_name((WORD, "document")),
            "not a supported Word document"
        );
        let mut result = vec![("document".into(), main.clone())];
        let (directory, filename) = main.rsplit_once('/').unwrap_or(("", &main));
        let rel_path = format!("{directory}/_rels/{filename}.rels");
        if parts.contains_key(&rel_path) {
            let rels_text = xml_part(parts, &rel_path)?;
            let rels = Document::parse(&rels_text)?;
            for rel in rels
                .descendants()
                .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
            {
                let kind = rel
                    .attribute("Type")
                    .unwrap_or("")
                    .strip_prefix(&format!("{REL}/"))
                    .unwrap_or("");
                if matches!(
                    kind,
                    "header" | "footer" | "footnotes" | "endnotes" | "comments"
                ) {
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

fn word(format: &str) -> bool {
    WORD_FORMATS.contains(&format)
}

/// Flags each element of `text_nodes` that displays a field result. Complex
/// fields show their result between `separate` and `end`; nested fields count
/// as results when any enclosing field has reached its result.
fn word_field_results(xml: &Document<'_>) -> Vec<bool> {
    let mut open: Vec<bool> = vec![];
    let mut flags = vec![];
    for node in xml.descendants().filter(|n| !hidden_copy(*n, "docx")) {
        if node.has_tag_name((WORD, "fldChar")) {
            match node.attribute((WORD, "fldCharType")) {
                Some("begin") => open.push(false),
                Some("separate") => {
                    if let Some(separated) = open.last_mut() {
                        *separated = true;
                    }
                }
                Some("end") => {
                    open.pop();
                }
                _ => {}
            }
        } else if node.has_tag_name((WORD, "t")) {
            flags.push(
                open.contains(&true)
                    || node
                        .ancestors()
                        .any(|n| n.has_tag_name((WORD, "fldSimple"))),
            );
        }
    }
    flags
}

fn text_node(node: Node<'_, '_>, format: &str) -> bool {
    node.has_tag_name((if word(format) { WORD } else { DRAWING }, "t"))
}

fn alternate_branch(node: Node<'_, '_>) -> bool {
    (node.has_tag_name((MARKUP_COMPATIBILITY, "Choice"))
        || node.has_tag_name((MARKUP_COMPATIBILITY, "Fallback")))
        && node
            .parent()
            .is_some_and(|p| p.has_tag_name((MARKUP_COMPATIBILITY, "AlternateContent")))
}

/// Word writes text boxes and shapes twice in `mc:AlternateContent`: DrawingML
/// in `mc:Choice` and VML in `mc:Fallback`. Readers show the first branch that
/// holds text, so text in a later branch is a hidden copy.
fn hidden_copy(node: Node<'_, '_>, format: &str) -> bool {
    node.ancestors().any(|branch| {
        alternate_branch(branch)
            && branch.prev_siblings().skip(1).any(|earlier| {
                // An equation (a14:m) is text a reader shows, though not a:t.
                alternate_branch(earlier)
                    && earlier
                        .descendants()
                        .any(|n| text_node(n, format) || n.has_tag_name((MATH, "t")))
            })
    })
}

/// Text elements a reader sees, in document order; their ordinals are A1, A2, ...
/// A PowerPoint table cell merged into its neighbor (`hMerge`, `vMerge`) is not
/// shown, whatever text it still holds.
fn text_nodes<'a, 'input>(xml: &'a Document<'input>, format: &str) -> Vec<Node<'a, 'input>> {
    xml.descendants()
        .filter(|n| text_node(*n, format) && !hidden_copy(*n, format))
        .filter(|n| {
            word(format)
                || !n.ancestors().any(|cell| {
                    cell.has_tag_name((DRAWING, "tc"))
                        && ["hMerge", "vMerge"]
                            .iter()
                            .any(|merge| matches!(cell.attribute(*merge), Some("1" | "true")))
                })
        })
        .collect()
}

/// Hidden copies of a visible text element, matched by position within each
/// alternate branch, so that an edit keeps both renderings of a text box equal.
fn hidden_copies<'a, 'input>(
    node: Node<'a, 'input>,
    format: &str,
) -> Result<Vec<Node<'a, 'input>>> {
    let texts = |branch: Node<'a, 'input>| -> Vec<Node<'a, 'input>> {
        branch
            .descendants()
            .filter(|n| text_node(*n, format))
            .collect()
    };
    let mut copies = vec![];
    for branch in node.ancestors().filter(|n| alternate_branch(*n)) {
        let own = texts(branch);
        let index = own
            .iter()
            .position(|n| *n == node)
            .context("text element outside its branch")?;
        for other in branch
            .next_siblings()
            .skip(1)
            .filter(|n| alternate_branch(*n))
        {
            let theirs = texts(other);
            if theirs.is_empty() {
                continue;
            }
            ensure!(
                theirs.len() == own.len(),
                "the text box copies in mc:AlternateContent differ; edit this text in Office"
            );
            copies.push(theirs[index]);
        }
    }
    Ok(copies)
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
            // Word blocks hold paragraph and line breaks; the edit is checked per run.
            if self.format != "pdf" && !word(&self.format) {
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
                    let (original, encoding) = part_text(&parts[&part])?;
                    let original = original.as_ref();
                    let xml = Document::parse(original)?;
                    let nodes = text_nodes(&xml, &self.format);
                    let mut edits = vec![];
                    if word(&self.format) {
                        let fields: std::collections::BTreeSet<_> = nodes
                            .iter()
                            .zip(word_field_results(&xml))
                            .filter(|(_, field)| *field)
                            .map(|(node, _)| node.range().start)
                            .collect();
                        let layout = word::layout(&xml)?;
                        for (i, text) in changes {
                            let block = layout.blocks.get(i).context("text block disappeared")?;
                            for (node, new) in word::edit(block, &text)? {
                                ensure!(
                                    !fields.contains(&node.range().start),
                                    "Word field result text cannot be edited because Word recalculates it (date, page number, table of contents, cross-reference, etc.): {part} {}; edit the field in Word",
                                    block.address
                                );
                                ensure!(
                                    !word::data_bound(node),
                                    "Word content control text bound to document data (such as a cover page title or author) cannot be edited because Word restores it from that data: {part} {}; edit it in Word",
                                    block.address
                                );
                                edits.push((node, new));
                            }
                        }
                    } else {
                        for (i, text) in changes {
                            let node = *nodes.get(i).context("text node disappeared")?;
                            // Slide numbers and dates are fields PowerPoint fills in again.
                            ensure!(
                                !node.ancestors().any(|a| a.has_tag_name((DRAWING, "fld"))),
                                "PowerPoint field text (slide number, date, etc.) cannot be edited because PowerPoint recalculates it: {part} A{}; edit the field in PowerPoint",
                                i + 1
                            );
                            edits.push((node, text));
                        }
                    }
                    let mut targets = vec![];
                    for (node, text) in edits {
                        for copy in hidden_copies(node, &self.format)? {
                            targets.push((copy, text.clone()));
                        }
                        targets.push((node, text));
                    }
                    let mut text_edits = vec![];
                    for (node, text) in targets {
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
                        if word(&self.format) {
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
                        text_edits.push((node.range(), replacement));
                    }
                    let result = splice(original, text_edits, "text")?;
                    Document::parse(&result)?;
                    patched.insert(part, encode_part(&result, encoding));
                }
                crate::excel::write_archive(&self.raw, output, &patched)?;
            }
            TextBackend::Pdf {
                doc: original,
                unparsed,
            } => {
                // lopdf decrypts a PDF that opens without a password (one with only
                // an editing or printing restriction) and would save it unprotected.
                ensure!(
                    !original.was_encrypted(),
                    "encrypted PDF writeback is not supported, including PDFs protected only against editing or printing"
                );
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
                // Saving through lopdf re-serializes every object even without edits.
                if replacements.is_empty() {
                    crate::excel::write_unchanged(&self.raw, output)?;
                } else {
                    let mut doc = original.clone();
                    let glyphs = subset_glyphs(original)?;
                    for (page, id) in original.get_pages() {
                        if let Some(changes) = replacements.get(&format!("pdf:{page}")) {
                            ensure!(
                                !unparsed.contains(&page),
                                "PDF page {page} has drawing commands ARP cannot read to the end, so it cannot be rewritten"
                            );
                            let mut content = Content::decode_strict(
                                &original.get_page_content_with_limit(id, MAX_PAGE)?,
                            )?;
                            // lopdf cannot write an inline image back.
                            ensure!(
                                content.operations.iter().all(|o| o.operator != "BI"),
                                "PDF page {page} has inline images, which ARP cannot write back; edit the text in a PDF editor"
                            );
                            pdf_text(original, id, &mut content, changes, Some(&glyphs))?;
                            // Always allocate a new stream: an original stream may be shared by pages.
                            let stream = doc.add_object(Stream::new(
                                lopdf::Dictionary::new(),
                                content.encode()?,
                            ));
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
        }
        Ok(json!({"format":self.format,"text_changes":changes.len(),"layout_review_required":true}))
    }
}

enum PdfEncoding<'a> {
    /// `width` is the length of a character code in bytes.
    Mapped {
        encoding: lopdf::Encoding<'a>,
        width: usize,
    },
    Unicode {
        ucs2: bool,
    },
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
        Ok(Self::Mapped {
            encoding,
            width: code_width(font),
        })
    }

    fn decode(&self, bytes: &[u8]) -> Result<String> {
        match self {
            // Decoded code by code: lopdf reads a ToUnicode map greedily, so a code
            // missing from it would otherwise swallow the characters after it.
            Self::Mapped {
                encoding: encoding @ lopdf::Encoding::UnicodeMapEncoding(_),
                width,
            } => bytes
                .chunks(*width)
                .map(|code| Ok(Pdf::decode_text(encoding, code)?))
                .collect(),
            Self::Mapped { encoding, .. } => Ok(Pdf::decode_text(encoding, bytes)?),
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
            Self::Mapped { encoding, .. } => Ok(Pdf::encode_text(encoding, text)),
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

/// Identity of a font dictionary in a loaded document.
fn font_key(font: &lopdf::Dictionary) -> usize {
    std::ptr::from_ref(font) as usize
}

/// An embedded subset (`ABCDEF+Name`) holds only the glyphs its document uses.
fn subset_font(font: &lopdf::Dictionary) -> bool {
    font.get(b"BaseFont")
        .and_then(Object::as_name)
        .is_ok_and(|name| {
            name.len() > 7 && name[6] == b'+' && name[..6].iter().all(u8::is_ascii_uppercase)
        })
}

/// Bytes per character code: two for composite (Type0) fonts, one otherwise.
fn code_width(font: &lopdf::Dictionary) -> usize {
    if font
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|n| n == b"Type0")
    {
        2
    } else {
        1
    }
}

/// Character codes each subset font draws anywhere in the document; a subset
/// is known to hold a glyph only for these.
fn subset_glyphs(doc: &Pdf) -> Result<BTreeMap<usize, BTreeSet<Vec<u8>>>> {
    let mut used: BTreeMap<usize, BTreeSet<Vec<u8>>> = BTreeMap::new();
    for (_, id) in doc.get_pages() {
        let fonts = doc.get_page_fonts(id)?;
        let Ok(content) = Content::decode_strict(&doc.get_page_content_with_limit(id, MAX_PAGE)?)
        else {
            continue;
        };
        let mut font = None;
        for operation in &content.operations {
            match operation.operator.as_str() {
                "Tf" => {
                    font = operation
                        .operands
                        .first()
                        .and_then(|name| name.as_name().ok())
                        .and_then(|name| fonts.get(name))
                        .copied();
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    let Some(current) = font.filter(|f| subset_font(f)) else {
                        continue;
                    };
                    let strings: Vec<&Object> = match operation.operands.last() {
                        Some(Object::Array(items)) => items.iter().collect(),
                        Some(other) => vec![other],
                        None => vec![],
                    };
                    let codes = used.entry(font_key(current)).or_default();
                    for object in strings {
                        if let Object::String(bytes, _) = object {
                            codes.extend(bytes.chunks(code_width(current)).map(<[u8]>::to_vec));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(used)
}

/// Decodes the page's text strings, and with `changes` replaces the strings at
/// those indexes. `glyphs` (from [`subset_glyphs`]) limits a replacement drawn
/// with an embedded subset font to the codes that subset is known to hold.
fn pdf_text(
    doc: &Pdf,
    page: lopdf::ObjectId,
    content: &mut Content<Vec<lopdf::content::Operation>>,
    changes: &BTreeMap<usize, String>,
    glyphs: Option<&BTreeMap<usize, BTreeSet<Vec<u8>>>>,
) -> Result<Vec<String>> {
    let fonts = doc.get_page_fonts(page)?;
    // A ToUnicode map says what a simple font's codes mean even when the font
    // also names an /Encoding, which lopdf would otherwise use alone (and replace
    // by StandardEncoding when it cannot read its glyph names).
    let mapped: BTreeMap<&[u8], lopdf::Dictionary> = fonts
        .iter()
        .filter(|(_, font)| {
            code_width(font) == 1 && font.has(b"ToUnicode") && font.has(b"Encoding")
        })
        .map(|(name, font)| {
            let mut font = (*font).clone();
            font.remove(b"Encoding");
            (name.as_slice(), font)
        })
        .collect();
    let mut encodings = BTreeMap::new();
    let mut font = Vec::new();
    // Text render mode (Tr): 3 draws nothing and 7 only clips, as in the OCR
    // layer of a scanned page.
    let mut render = 0;
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
            "Tr" => {
                render = operation
                    .operands
                    .first()
                    .and_then(|mode| mode.as_i64().ok())
                    .unwrap_or(0);
            }
            "q" => stack.push((font.clone(), render)),
            "Q" => {
                (font, render) = stack.pop().context("unbalanced PDF graphics state")?;
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
                // Reading an encoding parses the font's ToUnicode map, so it is
                // read once per font, not for every string drawn with it.
                if !encodings.contains_key(&font) {
                    let font_dictionary = *fonts.get(&font).context("PDF text font not found")?;
                    let encoding = PdfEncoding::for_font(
                        mapped.get(font.as_slice()).unwrap_or(font_dictionary),
                        doc,
                    )?;
                    encodings.insert(font.clone(), (font_dictionary, encoding));
                }
                let (font_dictionary, encoding) = &encodings[&font];
                let font_dictionary = *font_dictionary;
                for object in strings {
                    let Object::String(bytes, _) = object else {
                        unreachable!()
                    };
                    let value = encoding
                        .decode(bytes)
                        .context("PDF font encoding cannot be decoded")?;
                    if let Some(replacement) = changes.get(&values.len()) {
                        ensure!(
                            !matches!(render, 3 | 7),
                            "PDF text {value:?} is drawn invisibly (such as the OCR text layer of a scanned page), so an edit would not change what the page shows; edit it in a PDF editor"
                        );
                        ensure!(
                            !replacement.contains(['\n', '\r', '\t']),
                            "PDF text replacement cannot contain line breaks or tabs; layout operations are not supported"
                        );
                        // The string must be what its codes say: codes the encoding
                        // drops, or several codes for one character (such as the
                        // vertical forms of a Japanese font), would change on re-encoding.
                        ensure!(
                            encoding.encode(&value)? == *bytes,
                            "PDF text {value:?} uses character codes its font does not map one-to-one to text, so rewriting it would change the unedited characters; edit it in a PDF editor"
                        );
                        let encoded = encoding.encode(replacement)?;
                        ensure!(
                            encoding.decode(&encoded)? == *replacement,
                            "replacement contains characters unavailable in the original PDF font encoding"
                        );
                        if let Some(glyphs) = glyphs
                            && subset_font(font_dictionary)
                        {
                            let known = glyphs.get(&font_key(font_dictionary));
                            for code in encoded.chunks(code_width(font_dictionary)) {
                                let text = encoding.decode(code).unwrap_or_default();
                                ensure!(
                                    known.is_some_and(|codes| codes.contains(code)),
                                    "the embedded font subset has no known glyph for {text:?}, which the document never draws in that font; use characters the PDF already shows in that text, or edit it in a PDF editor"
                                );
                            }
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    const BODY: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t>plain</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>DATE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2026/09/25</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p>
        <w:p><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>TOC</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:r><w:t>entry</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>tail</w:t></w:r></w:p>
        </w:body></w:document>"#;

    #[test]
    fn word_field_results_cover_complex_simple_and_multi_paragraph_fields() {
        let xml = Document::parse(BODY).unwrap();
        assert_eq!(
            word_field_results(&xml),
            [false, true, false, true, true, false]
        );
    }

    fn package(path: &Path, parts: &[(&str, Vec<u8>)]) {
        let mut zip = ZipWriter::new(fs::File::create(path).unwrap());
        for (name, content) in parts {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    fn word_package(path: &Path, body: &str, extra: &[(&str, Vec<u8>)]) {
        let mut parts = vec![
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            ("word/document.xml", body.as_bytes().to_vec()),
        ];
        parts.extend(extra.iter().cloned());
        package(path, &parts);
    }

    #[test]
    fn word_field_result_edit_is_rejected_before_output_and_plain_run_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fields.docm");
        word_package(
            &source,
            BODY,
            &[("word/vbaProject.bin", b"opaque".to_vec())],
        );
        let Source::Text(doc) = Source::open(&source).unwrap() else {
            panic!("Word source expected")
        };
        // Paragraphs are blocks: the field result and the run after it share A2.
        assert_eq!(
            values(&doc, 0),
            ["plain", "2026/09/25after", "1", "entry", "tail"]
        );
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        for (cell, before, after) in [
            ("A2", "2026/09/25after", "2026/09/26after"),
            ("A3", "1", "2"),
            ("A4", "entry", "edited"),
        ] {
            let rejected = dir.path().join("rejected.docm");
            let error = doc
                .patch(&rejected, &[change(cell, before, after)])
                .unwrap_err();
            assert!(
                error.to_string().contains("Word field result"),
                "{cell}: {error}"
            );
            assert!(!rejected.exists());
        }
        let written = dir.path().join("written.docm");
        doc.patch(
            &written,
            &[change("A2", "2026/09/25after", "2026/09/25edited")],
        )
        .unwrap();
        assert_eq!(
            values(&open_word(&written), 0),
            ["plain", "2026/09/25edited", "1", "entry", "tail"]
        );
    }

    #[test]
    fn word_templates_share_the_document_body() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["dotx", "dotm"] {
            let source = dir.path().join(format!("template.{extension}"));
            word_package(&source, BODY, &[]);
            let Source::Text(doc) = Source::open(&source).unwrap() else {
                panic!("Word source expected")
            };
            assert_eq!(doc.sheets[0]["cells"][0]["value"], "plain");
        }
    }

    #[test]
    fn strict_word_is_rejected_with_resave_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("strict.docx");
        package(
            &source,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{STRICT_REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                (
                    "word/document.xml",
                    br#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"/>"#
                        .to_vec(),
                ),
            ],
        );
        let error = Source::open(&source).err().unwrap().to_string();
        assert!(error.contains("Strict Open XML"), "{error}");
    }

    #[test]
    fn ole_compound_files_are_reported_as_encrypted_or_binary() {
        let ole = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
        let mut encrypted = ole.to_vec();
        encrypted.extend([0; 64]);
        encrypted.extend("EncryptedPackage".encode_utf16().flat_map(u16::to_le_bytes));
        let error = ensure_zip_package(&encrypted).unwrap_err().to_string();
        assert!(error.contains("encrypted"), "{error}");
        let mut binary = ole.to_vec();
        binary.extend("WordDocument".encode_utf16().flat_map(u16::to_le_bytes));
        let error = ensure_zip_package(&binary).unwrap_err().to_string();
        assert!(error.contains("binary Office document"), "{error}");
        ensure_zip_package(b"PK\x03\x04").unwrap();
        let dir = tempfile::tempdir().unwrap();
        for extension in ["docx", "xlsm"] {
            let source = dir.path().join(format!("protected.{extension}"));
            fs::write(&source, &encrypted).unwrap();
            let error = Source::open(&source).err().unwrap().to_string();
            assert!(error.contains("encrypted"), "{extension}: {error}");
        }
    }

    #[test]
    fn format_lists_compose_without_duplicates() {
        let inputs = input_formats();
        let unique: std::collections::BTreeSet<_> = inputs.iter().collect();
        assert_eq!(unique.len(), inputs.len());
        assert!(structure_formats().iter().all(|f| inputs.contains(f)));
        assert_eq!(binary_office_replacement("xlsb"), Some(".xlsx/.xlsm"));
        assert_eq!(binary_office_replacement("xlsx"), None);
    }

    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    fn word_rels(extra: &str) -> (&'static str, Vec<u8>) {
        (
            "word/_rels/document.xml.rels",
            format!(r#"<Relationships xmlns="{PACKAGE_REL}">{extra}</Relationships>"#).into_bytes(),
        )
    }

    fn open_word(source: &Path) -> TextSource {
        let Source::Text(doc) = Source::open(source).unwrap() else {
            panic!("Word source expected")
        };
        doc
    }

    fn values(doc: &TextSource, sheet: usize) -> Vec<String> {
        doc.sheets[sheet]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_str().unwrap().to_owned())
            .collect()
    }

    fn written_part(path: &Path, name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
        let mut bytes = vec![];
        zip.by_name(name).unwrap().read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn text_box_is_read_once_and_edits_reach_both_renderings() {
        let text_box = |text: &str| {
            format!(r#"<w:txbxContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:txbxContent>"#)
        };
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{}</w:drawing></mc:Choice><mc:Fallback><w:pict>{}</w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2026/09/25</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p></w:body></w:document>"#,
            text_box("note"),
            text_box("note")
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("box.docx");
        word_package(&source, &body, &[]);
        let doc = open_word(&source);
        // The anchor paragraph has no text of its own; the text box is its own block.
        assert_eq!(values(&doc, 0), ["note", "2026/09/25after"]);
        let change = |cell: &str, before: &str| json!({"sheet":"document","cell":cell,"before":before,"after":"edited"});
        let error = doc
            .patch(
                &dir.path().join("field.docx"),
                &[change("A2", "2026/09/25after")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("Word field result"), "{error}");
        let written = dir.path().join("written.docx");
        doc.patch(&written, &[change("A1", "note")]).unwrap();
        let xml = String::from_utf8(written_part(&written, "word/document.xml")).unwrap();
        assert_eq!(xml.matches(">edited</w:t>").count(), 2, "{xml}");
        assert!(!xml.contains(">note<"));

        let differing = body.replacen(&text_box("note"), &(text_box("a") + &text_box("b")), 2);
        let differing = differing.replacen(&(text_box("a") + &text_box("b")), &text_box("note"), 1);
        let source = dir.path().join("differing.docx");
        word_package(&source, &differing, &[]);
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0)[0], "note");
        let error = doc
            .patch(
                &dir.path().join("differing-out.docx"),
                &[change("A1", "note")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("copies"), "{error}");
    }

    #[test]
    fn empty_choice_falls_back_to_the_branch_with_text() {
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"/><mc:Fallback><w:t>fallback</w:t></mc:Fallback></mc:AlternateContent></w:r></w:p></w:body></w:document>"#
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fallback.docx");
        word_package(&source, &body, &[]);
        assert_eq!(values(&open_word(&source), 0), ["fallback"]);
    }

    #[test]
    fn percent_encoded_targets_and_comments_are_read() {
        let part = |text: &str, root: &str| {
            format!(r#"<w:{root} xmlns:w="{W}"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:{root}>"#)
                .into_bytes()
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("encoded.docx");
        word_package(
            &source,
            BODY,
            &[
                word_rels(&format!(
                    r#"<Relationship Id="h1" Type="{REL}/header" Target="header%201.xml"/><Relationship Id="h2" Type="{REL}/header" Target="%E3%83%98%E3%83%83%E3%83%80.xml"/><Relationship Id="h3" Type="{REL}/footer" Target="footer%201.xml"/><Relationship Id="c" Type="{REL}/comments" Target="comments.xml"/>"#
                )),
                ("word/header 1.xml", part("spaced", "hdr")),
                ("word/ヘッダ.xml", part("japanese", "hdr")),
                ("word/footer%201.xml", part("literal", "ftr")),
                ("word/comments.xml", part("comment body", "comments")),
            ],
        );
        let doc = open_word(&source);
        let names: Vec<_> = doc
            .sheets
            .iter()
            .map(|s| s["part"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "word/document.xml",
                "word/header 1.xml",
                "word/ヘッダ.xml",
                "word/footer%201.xml",
                "word/comments.xml"
            ]
        );
        assert_eq!(values(&doc, 4), ["comment body"]);
        assert!(
            doc.sheets[4]["name"]
                .as_str()
                .unwrap()
                .starts_with("comments-")
        );

        for target in ["..%2Fescape.xml", "bad%zz.xml"] {
            let source = dir.path().join("rejected.docx");
            let _ = fs::remove_file(&source);
            word_package(
                &source,
                BODY,
                &[word_rels(&format!(
                    r#"<Relationship Id="h1" Type="{REL}/header" Target="{target}"/>"#
                ))],
            );
            assert!(Source::open(&source).is_err(), "{target}");
        }
    }

    #[test]
    fn utf16_parts_are_read_and_written_back_in_utf16() {
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-16" standalone="yes"?><w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>本文</w:t></w:r></w:p></w:body></w:document>"#
        );
        let encoded = |text: &str| -> Vec<u8> {
            [0xFF, 0xFE]
                .into_iter()
                .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
                .collect()
        };
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("utf16.docx");
        package(
            &source,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                ("word/document.xml", encoded(&body)),
            ],
        );
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0), ["本文"]);
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[json!({"sheet":"document","cell":"A1","before":"本文","after":"更新"})],
        )
        .unwrap();
        assert_eq!(
            written_part(&written, "word/document.xml"),
            encoded(&body.replace("<w:t>本文", r#"<w:t xml:space="preserve">更新"#))
        );
        assert_eq!(values(&open_word(&written), 0), ["更新"]);
    }
}
