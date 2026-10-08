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
    path::Path,
};

pub mod encryption;
mod package;
mod slide_drawings;
mod slide_fonts;
mod slide_shapes;
mod slide_text;
mod slides;
mod word;
mod word_fonts;

pub(crate) use package::{office_archive, office_package, read_entry, uncompressed_size};
pub use slide_shapes::{
    ConnectorEnd, ShapeEdit, ShapeOperation, ShapeProperties, is_shape_operation,
    parse_shape_operations,
};
pub use slides::{
    SlideOperation, SlidePosition, is_slide_operation, parse_slide_operations, slide_order,
    slide_view,
};

/// Whether `value` is an operation on PowerPoint slides or their shapes.
pub fn is_presentation_operation(value: &Value) -> bool {
    is_slide_operation(value) || is_shape_operation(value)
}

const WORD: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const MATH: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const DRAWING: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PRESENTATION: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
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

pub enum Source {
    Excel(Workbook),
    Text(TextSource),
}

pub struct TextSource {
    raw: Vec<u8>,
    sheets: Vec<Value>,
    format: String,
    backend: TextBackend,
    /// The slide layouts of a presentation (see [`slides::layouts`]).
    layouts: Vec<Value>,
    /// The pictures of a presentation's slides by asset name.
    images: BTreeMap<String, Vec<u8>>,
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
    /// Images in Word's media folder are kept as independently reviewable
    /// assets. Their bytes are read from the original package only on import.
    pub fn word_images(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let Self::Text(doc) = self else {
            return Ok(BTreeMap::new());
        };
        if doc.format == "pptx" {
            return Ok(doc.images.clone());
        }
        if !word(&doc.format) {
            return Ok(BTreeMap::new());
        }
        let mut archive = office_archive(&doc.raw)?;
        let mut images = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            if !entry.name().starts_with("word/media/") || entry.is_dir() {
                continue;
            }
            ensure!(
                entry.size() <= 256 * 1024 * 1024,
                "Word image exceeds size budget"
            );
            let bytes = read_entry(&mut entry)?;
            images.insert(entry.name().to_owned(), bytes);
        }
        let mut named = BTreeMap::new();
        for (index, (part, bytes)) in images.into_iter().enumerate() {
            let extension = Path::new(&part)
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                .unwrap_or("bin");
            named.insert(format!("image-{:03}.{extension}", index + 1), bytes);
        }
        Ok(named)
    }

    /// Word field instructions are read-only metadata, separate from the
    /// displayed, possibly stale field results in the extraction cells.
    pub fn word_field_codes(&self) -> Result<Vec<Value>> {
        let Self::Text(doc) = self else {
            return Ok(vec![]);
        };
        if !word(&doc.format) {
            return Ok(vec![]);
        }
        let TextBackend::Office(parts) = &doc.backend else {
            return Ok(vec![]);
        };
        let mut result = Vec::new();
        for sheet in &doc.sheets {
            let part = string(&sheet["part"])?;
            let (text, _) = part_text(parts.get(part).context("missing Word part")?)?;
            result.extend(word::extract_field_codes(&Document::parse(&text)?, part));
        }
        Ok(result)
    }

    /// Print-related Open XML elements with their source part. Keep the XML
    /// fragment so settings not modeled by ARP remain visible and auditable.
    pub fn print_settings(&self) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let parts = match self {
            Self::Excel(book) => &book.parts,
            Self::Text(doc) if word(&doc.format) => {
                let TextBackend::Office(parts) = &doc.backend else {
                    return Ok(result);
                };
                parts
            }
            _ => return Ok(result),
        };
        for (part, bytes) in parts {
            let excel_workbook = part == "xl/workbook.xml";
            let excel_sheet = part.starts_with("xl/worksheets/") && part.ends_with(".xml");
            let word_part = part.starts_with("word/") && part.ends_with(".xml");
            if !(excel_workbook || excel_sheet || word_part) {
                continue;
            }
            let (text, _) = part_text(bytes)?;
            let xml = Document::parse(&text)?;
            for node in xml.descendants().filter(Node::is_element) {
                let name = node.tag_name().name();
                let relevant = if excel_workbook {
                    name == "definedName"
                        && matches!(
                            node.attribute("name"),
                            Some("_xlnm.Print_Area" | "_xlnm.Print_Titles")
                        )
                } else if excel_sheet {
                    matches!(
                        name,
                        "printOptions"
                            | "pageMargins"
                            | "pageSetup"
                            | "headerFooter"
                            | "rowBreaks"
                            | "colBreaks"
                            | "pageSetUpPr"
                    )
                } else {
                    name == "sectPr" && node.tag_name().namespace() == Some(WORD)
                };
                if relevant {
                    result.push(json!({"part":part,"component":name,"xml":&text[node.range()]}));
                }
            }
        }
        Ok(result)
    }

    /// Expose complete chart XML and its source formulas for read-only review.
    /// Chart editing continues through Excel.
    pub fn chart_parts(&self) -> Result<Vec<Value>> {
        let Self::Excel(book) = self else {
            return Ok(vec![]);
        };
        let mut result = Vec::new();
        for (part, bytes) in &book.parts {
            if !part.starts_with("xl/charts/") || !part.ends_with(".xml") {
                continue;
            }
            let (text, _) = part_text(bytes)?;
            let xml = Document::parse(&text)?;
            let formulas: Vec<_> = xml
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "f")
                .filter_map(|n| n.text().map(str::to_owned))
                .collect();
            result.push(json!({"part":part,"formulas":formulas,"xml":text.as_ref()}));
        }
        Ok(result)
    }

    /// Binary controls and printer settings cannot be interpreted safely, but
    /// their identity and hash are part of the source fidelity record.
    pub fn opaque_parts(&self) -> Vec<Value> {
        let Self::Excel(book) = self else {
            return vec![];
        };
        book.parts
            .iter()
            .filter_map(|(part, bytes)| {
                let lower = part.to_ascii_lowercase();
                let kind = if lower.starts_with("xl/activex/") {
                    "activex"
                } else if lower.starts_with("xl/printersettings/") {
                    "printer_settings"
                } else {
                    return None;
                };
                Some(json!({"part":part,"kind":kind,"sha256":hash(bytes)}))
            })
            .collect()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !is_excel(&format) {
            ensure!(
                input_formats().contains(&format.as_str()),
                "unsupported document format: {format}"
            );
            let size = fs::metadata(path)?.len();
            ensure!(size <= MAX_BYTES as u64, "document exceeds size budget");
            if matches!(format.as_str(), "txt" | "md" | "csv" | "tsv") {
                ensure!(
                    size <= MAX_PAGE as u64,
                    "text document exceeds 32 MiB size budget"
                );
            }
        }
        let raw = fs::read(path)?;
        Self::from_bytes(path, raw)
    }

    /// Parse bytes already read while checking whether an original changed.
    pub(crate) fn from_bytes(path: &Path, raw: Vec<u8>) -> Result<Self> {
        let format = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("")
            .to_lowercase();
        if is_excel(&format) {
            return Ok(Self::Excel(Workbook::from_bytes(raw)?));
        }
        ensure!(
            input_formats().contains(&format.as_str()),
            "unsupported document format: {format}"
        );
        ensure!(raw.len() <= MAX_BYTES, "document exceeds size budget");
        let native = matches!(format.as_str(), "txt" | "md" | "csv" | "tsv");
        if native {
            ensure!(
                raw.len() <= MAX_PAGE,
                "text document exceeds 32 MiB size budget"
            );
        }
        // A presentation's pictures, which the XML parts leave out.
        let media = if format == "pptx" {
            slide_drawings::media(&raw)?
        } else {
            BTreeMap::new()
        };
        let (backend, mut sheets) = if native {
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
                let values = pdf_text(&doc, id, &mut content, &BTreeMap::new(), None)?
                    .into_iter()
                    .map(|(text, font)| (text, Some(font)))
                    .collect();
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
            let parts = office_parts(&raw, false)?;
            let containers = office_containers(&parts, &format)?;
            let styles = word(&format)
                .then(|| word_fonts::WordStyles::new(&parts))
                .transpose()?;
            let mut sheets = vec![];
            for (name, part) in containers {
                let (text, _) = part_text(parts.get(&part).context("missing document part")?)?;
                let xml = Document::parse(&text)?;
                if let Some(styles) = &styles {
                    sheets.push(word::layout(&xml)?.sheet(&name, &part, styles));
                } else {
                    let fonts = slide_fonts::SlideFonts::new(&parts, &part, &xml)?;
                    let mut sheet = slide_text::layout(&xml)?.sheet(&name, &part, &fonts);
                    if xml.root_element().has_tag_name((PRESENTATION, "sld")) {
                        sheet["drawings"] = json!(slide_drawings::drawings(
                            &parts, &media, &part, &xml, &fonts
                        )?);
                    }
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
        let layouts = match &backend {
            TextBackend::Office(parts) if format == "pptx" => slides::layouts(parts)?,
            _ => vec![],
        };
        // Pictures are named in first-use order across the slides, as Excel's are.
        let mut images = BTreeMap::new();
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        if format == "pptx" {
            for sheet in &mut sheets {
                for drawing in sheet
                    .get_mut("drawings")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    let (Some(sha), Some(part)) = (
                        drawing["image"]["sha256"].as_str().map(str::to_owned),
                        drawing["image"]["part"].as_str().map(str::to_owned),
                    ) else {
                        continue;
                    };
                    let next = names.len() + 1;
                    let extension = Path::new(&part)
                        .extension()
                        .and_then(|s| s.to_str())
                        .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric()))
                        .unwrap_or("bin")
                        .to_owned();
                    let name = names
                        .entry(sha)
                        .or_insert_with(|| format!("image-{next:03}.{extension}"))
                        .clone();
                    if let Some(bytes) = media.get(&part) {
                        images.insert(name.clone(), bytes.clone());
                    }
                    drawing["image"]["asset"] = json!(name);
                }
            }
        }
        Ok(Self::Text(TextSource {
            raw,
            sheets,
            format,
            backend,
            layouts,
            images,
        }))
    }

    /// The slide layouts a new slide can be made from; none outside PowerPoint.
    pub fn slide_layouts(&self) -> &[Value] {
        match self {
            Self::Text(doc) => &doc.layouts,
            Self::Excel(_) => &[],
        }
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
            Self::Text(doc) if doc.format == "pptx" => "slide-blocks/1",
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
                "セル値・数式原文・結合範囲・書式の識別情報・Excelテーブル定義とDrawingMLの図形文字・配置・明示的な接続を抽出します。埋込み画像はassetsに保存し、WindowsではOCRを自動実行して結果を記録します。表・見出しの推定はstructure initで行い、レビューが必要です。グループ内位置は変換情報を保持します。メモ・スレッドコメント、図形に割り当てたマクロ、フォームコントロールの種類・リンク先セル・選択肢範囲も抽出します。接続のない矢印の意味とActiveXコントロールの内部設定は未抽出です。印刷設定とグラフXMLは読み取り専用で抽出し、ActiveX部品はハッシュで記録します。"
            }
            Self::Text(doc) if doc.format == "pdf" => {
                "ページのテキスト描画命令を文字列単位で抽出します。A1等は文字列の通し番号です。画像・OCR・フォーム・注釈・Form XObject内の文字は未抽出です。元フォントで表現可能な文字だけ書き戻せます。自動改行・再配置はしないため、文字の重なりやはみ出しを出力PDFで確認してください。"
            }
            Self::Text(doc) if word(&doc.format) => {
                "Word本文・表・テキストボックス・ヘッダー・フッター・脚注・文末脚注・コメントを段落と表のセル単位で抽出します。表の外の段落は1段落1行でA列に並び、表は行・列の位置を保ち、結合セルをmerges、表の範囲をtablesに記録します。見出し行はWordの見出し行の繰り返し、太字・網掛けの先頭行、3列以上の表の先頭行から推定するため、構造のレビューで確認してください。セル内の段落・改行は改行、タブはタブ文字で表し、入れ子の表は外側のセルの本文に含めます。書き戻しは変更箇所を含むrunだけを書き換えます。段落内の改行・タブは追加・削除・置換でき、runの改行（w:br）・タブ（w:tab）として書き込みます。段落の分割・結合となる変更は拒否します。テキストボックスは表示される1組だけを抽出し、書き戻しでは互換用の複製（VML）も同じ文字にします。画像はassetsに保存し、WindowsではOCRを自動実行します。フィールド命令はfield_codesへ読み取り専用で記録します。フィールドの表示結果（日付・ページ番号・目次・相互参照等）はWordが再計算するため書き戻しを拒否します。ページは再組版されるため出力Wordの表示を確認してください。"
            }
            _ => {
                "PPTXの各スライドの本文・表を段落と表のセル単位で抽出し、スライドのノートはスライドの後にnotes-N（Nはスライド番号）として抽出します。表の外の段落は図形をまたいでスライドの上から1段落1行でA列に並び、表は行・列の位置を保ち、結合セルをmerges、表の範囲をtablesに記録します。セル内の段落は改行で表します。ノートのスライド番号・日付・ヘッダー・フッターはノートマスターから表示されるため抽出しません。書き戻しは変更箇所を含むrunだけを書き換え、段落内の改行・タブを追加・削除・置換できます。段落の分割・結合となる変更は拒否し、段落・表の行はdocuments rowsで追加・削除します。スライドはdocuments slidesで複製・削除でき、複製したスライドは操作IDの名前で本文を編集します。マスター・画像・OCR・グラフ内部の文字は未抽出です。自動サイズ調整を行わないため、出力PPTXではみ出しを確認してください。"
            }
        }
    }

    /// Rejects row/column `operations` this document cannot take, before any
    /// output is written: Excel refuses what it cannot move, and Word and
    /// PowerPoint what they cannot copy or remove (text boxes, section breaks,
    /// vertical merges). `all` holds every recorded operation, so that rows of
    /// a slide a slide operation inserts are checked on the slide it copies.
    pub fn ensure_row_edits_supported(
        &self,
        operations: &[crate::excel::StructuralOperation],
        all: &[Value],
    ) -> Result<()> {
        match self {
            Self::Excel(book) => book.ensure_structural_edits_supported(operations),
            Self::Text(doc) => {
                if operations.is_empty() {
                    return Ok(());
                }
                ensure!(
                    word(&doc.format) || doc.format == "pptx",
                    "row operations apply to Excel, Word and PowerPoint documents only"
                );
                let TextBackend::Office(parts) = &doc.backend else {
                    bail!("Office document without parts");
                };
                let slide_operations = if doc.format == "pptx" {
                    parse_slide_operations(all, &doc.sheets)?
                } else {
                    vec![]
                };
                let names: BTreeSet<&str> = operations.iter().map(|o| o.sheet.as_str()).collect();
                let origins = slides::origins(&slide_operations, &doc.sheets)?;
                let sheets: BTreeMap<&str, &Value> = doc
                    .sheets
                    .iter()
                    .map(|sheet| Ok((string(&sheet["name"])?, sheet)))
                    .collect::<Result<_>>()?;
                for name in names {
                    let own: Vec<_> = operations.iter().filter(|o| o.sheet == name).collect();
                    // An inserted slide holds the markup of the slide it copies.
                    let sheet = sheets
                        .get(origins.get(name).map(String::as_str).unwrap_or(name))
                        .with_context(|| format!("{name} is not a page of the document"))?;
                    let (text, _) = part_text(&parts[string(&sheet["part"])?])?;
                    let xml = Document::parse(&text)?;
                    let values = word::InsertedText::new();
                    if word(&doc.format) {
                        word::layout(&xml)?.restructure(&text, &own, &values)
                    } else {
                        slide_text::layout(&xml)?.restructure(&text, &own, &values)
                    }
                    .with_context(|| name.to_owned())?;
                }
                Ok(())
            }
        }
    }

    /// Rejects slide `operations` before any output is written: they apply to
    /// PowerPoint only, and a slide another part links to cannot be deleted.
    pub fn ensure_slide_edits_supported(&self, operations: &[Value]) -> Result<()> {
        if !operations.iter().any(is_slide_operation) {
            return Ok(());
        }
        let Self::Text(TextSource {
            format,
            sheets,
            backend: TextBackend::Office(parts),
            ..
        }) = self
        else {
            bail!("slide operations apply to PowerPoint presentations only");
        };
        ensure!(
            format == "pptx",
            "slide operations apply to PowerPoint presentations only"
        );
        let operations = parse_slide_operations(operations, sheets)?;
        slides::restructure(parts, sheets, &operations)?;
        Ok(())
    }

    /// Writes the edited document to `output`.
    pub fn patch(&self, output: &Path, edits: &Edits<'_>) -> Result<Value> {
        match self {
            Self::Excel(book) => book.patch_with_fonts(
                output,
                edits.operations,
                edits.changes,
                edits.formulas,
                edits.fonts,
                edits.assets,
            ),
            Self::Text(doc) => {
                ensure!(
                    edits.formulas.is_empty(),
                    "text formats support text edits only; formulas are Excel-only"
                );
                ensure!(
                    edits.assets.is_empty() || doc.format == "pptx",
                    "images are added to Excel workbooks and PowerPoint slides only"
                );
                // Fonts are written to the original text first, so that copied
                // slides and moved rows take them as they take the text's runs.
                let edited = doc.with_fonts(edits.fonts)?;
                let mut report = edited.as_ref().unwrap_or(doc).patch_with(
                    output,
                    edits.operations,
                    edits.changes,
                    edits.assets,
                )?;
                if edited.is_some() {
                    report["font_changes_verified"] = json!(true);
                }
                Ok(report)
            }
        }
    }

    /// Whether `documents values` can edit fonts of this document.
    pub fn font_edits_supported(&self) -> bool {
        match self {
            Self::Excel(_) => true,
            Self::Text(doc) => word(&doc.format) || doc.format == "pptx",
        }
    }
}

/// The edits a document writer applies.
pub struct Edits<'a> {
    /// Row, column, slide and image operations (`mappings.operations`).
    pub operations: &'a [Value],
    /// New values of cells and text, and with `shape` of shape text.
    pub changes: &'a [Value],
    /// Excel formula edits (see [`Workbook::patch_with_operations_and_assets`]).
    pub formulas: &'a [Value],
    /// Font edits of original cells and shapes: `sheet`, `cell` or `shape`,
    /// and the font properties `after` sets.
    pub fonts: &'a [Value],
    /// Image assets of image operations by path.
    pub assets: &'a BTreeMap<String, Vec<u8>>,
}

/// The sheet of a page's text strings, each with the font it is drawn in
/// when the format records one.
fn text_sheet(name: &str, part: &str, texts: Vec<(String, Option<Value>)>) -> Value {
    let cells: Vec<_> = texts
        .into_iter()
        .enumerate()
        .map(|(i, (value, font))| {
            let mut cell = json!({
                "id":format!("text-{}",i+1), "address":format!("A{}",i+1),
                "type":"string", "value":value, "formula":null, "cached":null, "number_format":""
            });
            if let Some(font) = font {
                cell["font"] = font;
            }
            cell
        })
        .collect();
    json!({"name":name,"part":part,"state":"visible","merges":[],"cells":cells})
}

fn office_parts(raw: &[u8], include_binary: bool) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip = office_archive(raw)?;
    ensure!(
        zip.len() <= 10000 && uncompressed_size(&mut zip)? <= MAX_BYTES as u128,
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
        // Keep the name for relationship checks, but do not retain media and
        // embedded packages until a slide operation actually needs to copy them.
        let bytes = if include_binary || !office_binary(&name) {
            read_entry(&mut entry)?
        } else {
            vec![]
        };
        ensure!(parts.insert(name, bytes).is_none(), "duplicate Office part");
    }
    Ok(parts)
}

fn office_binary(name: &str) -> bool {
    name.rsplit('.').next().is_some_and(|extension| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "tif"
                | "tiff"
                | "emf"
                | "wmf"
                | "bin"
                | "xlsx"
                | "xlsm"
                | "xlsb"
                | "docx"
                | "docm"
                | "pptx"
                | "pdf"
                | "mp3"
                | "mp4"
                | "wav"
                | "avi"
        )
    })
}

fn validate_office_binary(raw: &[u8]) -> Result<()> {
    let mut zip = office_archive(raw)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if office_binary(entry.name()) {
            std::io::copy(&mut entry, &mut std::io::sink())?;
        }
    }
    Ok(())
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
    relationship_target(parts, part, rel)
}

fn relationship_target(
    parts: &BTreeMap<String, Vec<u8>>,
    part: &str,
    rel: Node<'_, '_>,
) -> Result<String> {
    ensure!(
        rel.attribute("TargetMode") != Some("External"),
        "external document part is not supported"
    );
    let target = rel
        .attribute("Target")
        .context("relationship target missing")?;
    resolve_target(part, target, |name| parts.contains_key(name))
}

/// The part an internal relationship `target` of `part` names; `exists` tells
/// whether a part is in the package.
fn resolve_target(part: &str, target: &str, exists: impl Fn(&str) -> bool) -> Result<String> {
    let directory = part.rsplit_once('/').map_or("", |(directory, _)| directory);
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
    if exists(&literal) || !literal.contains('%') {
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
        let rel_path = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
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
                    let part = relationship_target(parts, &main, rel)?;
                    if !result.iter().any(|(_, p)| p == &part) {
                        result.push((format!("{kind}-{}", result.len()), part));
                    }
                }
            }
        }
        Ok(result)
    } else {
        ensure!(
            xml.root_element()
                .has_tag_name((PRESENTATION, "presentation")),
            "not a supported presentation"
        );
        let (directory, filename) = main.rsplit_once('/').unwrap_or(("", &main));
        let rel_path = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
        let rels_text = xml_part(parts, &rel_path)?;
        let rels = Document::parse(&rels_text)?;
        let mut by_id = BTreeMap::new();
        for rel in rels
            .descendants()
            .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
        {
            if let Some(id) = rel.attribute("Id") {
                by_id.entry(id).or_insert(rel);
            }
        }
        let slides = xml
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATION, "sldId")))
            .enumerate()
            .map(|(i, n)| {
                Ok((
                    format!("slide-{}", i + 1),
                    relationship_target(
                        parts,
                        &main,
                        *by_id
                            .get(
                                n.attribute((REL, "id"))
                                    .context("missing slide relationship")?,
                            )
                            .context("missing Office relationship")?,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        // Speaker notes follow all slides, so that a slide keeps its page.
        let mut notes = vec![];
        for (i, (_, slide)) in slides.iter().enumerate() {
            let (directory, filename) = slide.rsplit_once('/').unwrap_or(("", slide));
            let rels_path = if directory.is_empty() {
                format!("_rels/{filename}.rels")
            } else {
                format!("{directory}/_rels/{filename}.rels")
            };
            if !parts.contains_key(&rels_path) {
                continue;
            }
            let rels_text = xml_part(parts, &rels_path)?;
            let rels = Document::parse(&rels_text)?;
            if let Some(id) = rels
                .descendants()
                .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
                .find(|n| n.attribute("Type") == Some(&format!("{REL}/notesSlide")))
                .and_then(|n| n.attribute("Id"))
            {
                notes.push((
                    format!("notes-{}", i + 1),
                    relation_target(parts, slide, id)?,
                ));
            }
        }
        Ok(slides.into_iter().chain(notes).collect())
    }
}

fn word(format: &str) -> bool {
    WORD_FORMATS.contains(&format)
}

/// The text elements, tabs and line breaks that display a field result, keyed
/// by their start in the part. Complex fields show their result between
/// `separate` and `end`; nested fields count as results when any enclosing
/// field has reached its result.
fn word_field_results(xml: &Document<'_>) -> BTreeSet<usize> {
    let mut open: Vec<bool> = vec![];
    let mut results = BTreeSet::new();
    let hidden = HiddenBranches::new(xml, "docx");
    for node in xml.descendants().filter(|n| !hidden.contains(*n)) {
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
        } else if (node.has_tag_name((WORD, "t")) || word::text_break(node))
            && (open.contains(&true)
                || node
                    .ancestors()
                    .any(|n| n.has_tag_name((WORD, "fldSimple"))))
        {
            results.insert(node.range().start);
        }
    }
    results
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

/// AlternateContent branches hidden by an earlier branch containing text.
/// Build once for document-wide scans instead of searching earlier subtrees
/// again for every descendant of a fallback branch.
pub(super) struct HiddenBranches {
    branches: BTreeSet<usize>,
}

impl HiddenBranches {
    pub(super) fn new(xml: &Document<'_>, format: &str) -> Self {
        let mut prior_text = BTreeMap::<usize, bool>::new();
        let mut branches = BTreeSet::new();
        for branch in xml.descendants().filter(|n| alternate_branch(*n)) {
            let parent = branch.parent().unwrap();
            let seen = prior_text.entry(parent.range().start).or_default();
            if *seen {
                branches.insert(branch.range().start);
            }
            if !*seen {
                *seen = branch
                    .descendants()
                    .any(|n| text_node(n, format) || n.has_tag_name((MATH, "t")));
            }
        }
        Self { branches }
    }

    pub(super) fn contains(&self, node: Node<'_, '_>) -> bool {
        node.ancestors()
            .any(|ancestor| self.branches.contains(&ancestor.range().start))
    }
}

/// Hidden copies of a visible text element, or of a Word tab or line break,
/// matched by position among its kind within each alternate branch, so that an
/// edit keeps both renderings of a text box equal.
fn hidden_copies<'a, 'input>(
    node: Node<'a, 'input>,
    format: &str,
    cache: &mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
) -> Result<Vec<Node<'a, 'input>>> {
    let text = text_node(node, format);
    let mut copies = vec![];
    for branch in node.ancestors().filter(|n| alternate_branch(*n)) {
        let own = branch_texts(cache, branch, format, text);
        let len = own.nodes.len();
        let index = *own
            .positions
            .get(&node.range().start)
            .context("text element outside its branch")?;
        for other in branch
            .next_siblings()
            .skip(1)
            .filter(|n| alternate_branch(*n))
        {
            let theirs = branch_texts(cache, other, format, text);
            if theirs.nodes.is_empty() {
                continue;
            }
            ensure!(
                theirs.nodes.len() == len,
                "the text box copies in mc:AlternateContent differ; edit this text in Office"
            );
            copies.push(theirs.nodes[index]);
        }
    }
    Ok(copies)
}

struct BranchTexts<'a, 'input> {
    nodes: Vec<Node<'a, 'input>>,
    positions: BTreeMap<usize, usize>,
}

fn branch_texts<'cache, 'a, 'input>(
    cache: &'cache mut BTreeMap<(usize, bool), BranchTexts<'a, 'input>>,
    branch: Node<'a, 'input>,
    format: &str,
    text: bool,
) -> &'cache BranchTexts<'a, 'input> {
    cache
        .entry((branch.range().start, text))
        .or_insert_with(|| {
            let nodes: Vec<_> = branch
                .descendants()
                .filter(|n| {
                    if text {
                        text_node(*n, format)
                    } else if word(format) {
                        word::text_break(*n)
                    } else {
                        n.has_tag_name((DRAWING, "br"))
                    }
                })
                .collect();
            let positions = nodes
                .iter()
                .enumerate()
                .map(|(index, node)| (node.range().start, index))
                .collect();
            BranchTexts { nodes, positions }
        })
}

pub(crate) fn xml_text(value: &str) -> Result<String> {
    ensure!(value.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')), "invalid XML character in replacement");
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;"))
}

/// Document data that Word content controls show, by store item and XPath.
type BoundData = BTreeMap<(String, String), (word::Binding, String)>;

/// The document data that edited Word content controls are bound to, with the
/// text it takes. Other controls bound to the same data get edits showing
/// that text too, as Word shows it on opening.
fn bound_data_edits<'a, 'input>(
    parts: &[String],
    docs: &'a [Document<'input>],
    edits: &mut [Vec<(Node<'a, 'input>, String)>],
) -> Result<BoundData> {
    let mut data = BoundData::new();
    let mut edited = BTreeSet::new();
    for (index, part_edits) in edits.iter().enumerate() {
        let mut edited_text = BTreeMap::new();
        for (node, value) in part_edits {
            edited_text
                .entry(node.range().start)
                .or_insert(value.as_str());
        }
        let mut controls = vec![];
        let mut control_starts = BTreeSet::new();
        for (node, _) in part_edits {
            if let Some(sdt) = word::bound_control(*node)
                && control_starts.insert(sdt.range().start)
            {
                controls.push(sdt);
            }
        }
        for sdt in controls {
            let place = format!("a content control in {}", parts[index]);
            let binding = word::binding(sdt, &place)?;
            let value = word::control_text(sdt, &edited_text);
            ensure!(
                binding.multiline || !value.contains('\n'),
                "{place} is a single-line text control bound to document data ({}); remove the line break",
                binding.xpath
            );
            let key = (binding.store.clone(), binding.xpath.clone());
            if let Some((_, existing)) = data.get(&key) {
                ensure!(
                    *existing == value,
                    "content controls bound to the same document data ({}) were edited to different text: {existing:?} and {value:?}",
                    binding.xpath
                );
            }
            data.insert(key, (binding, value));
            edited.insert((index, sdt.range().start));
        }
    }
    if data.is_empty() {
        return Ok(data);
    }
    let no_edits = BTreeMap::new();
    for (index, doc) in docs.iter().enumerate() {
        for sdt in doc
            .descendants()
            .filter(|n| n.has_tag_name((WORD, "sdt")) && !hidden_copy(*n, "docx"))
        {
            if edited.contains(&(index, sdt.range().start)) {
                continue;
            }
            let Some((_, value)) = word::binding_key(sdt).and_then(|key| data.get(&key)) else {
                continue;
            };
            if word::control_text(sdt, &no_edits) == *value {
                continue;
            }
            let place = format!("a content control in {}", parts[index]);
            word::binding(sdt, &place)?;
            edits[index].extend(word::fill_control(sdt, value, &place)?);
        }
    }
    Ok(data)
}

/// The part holding data store item `store`: the core or extended document
/// properties, or the custom XML part whose properties name it.
fn bound_data_part(parts: &BTreeMap<String, Vec<u8>>, store: &str) -> Result<String> {
    const CORE_PROPERTIES: &str = "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}";
    const EXTENDED_PROPERTIES: &str = "{6668398D-A668-4E3E-A5EB-62B293D839F1}";
    const CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/customXml";
    // The part `base` relates to with a relationship type ending in `kind`.
    let related = |base: &str, kind: &str| -> Result<Option<String>> {
        let (directory, filename) = base.rsplit_once('/').unwrap_or(("", base));
        let rels = if directory.is_empty() {
            format!("_rels/{filename}.rels")
        } else {
            format!("{directory}/_rels/{filename}.rels")
        };
        if !parts.contains_key(&rels) {
            return Ok(None);
        }
        let text = xml_part(parts, &rels)?;
        let doc = Document::parse(&text)?;
        let Some(id) = doc
            .descendants()
            .filter(|n| n.has_tag_name((PACKAGE_REL, "Relationship")))
            .find(|n| n.attribute("Type").is_some_and(|t| t.ends_with(kind)))
            .and_then(|n| n.attribute("Id"))
        else {
            return Ok(None);
        };
        relation_target(parts, base, id).map(Some)
    };
    let found = match store {
        CORE_PROPERTIES => related("", "/metadata/core-properties")?,
        EXTENDED_PROPERTIES => related("", "/extended-properties")?,
        _ => {
            let mut found = None;
            for part in parts
                .keys()
                .filter(|p| p.starts_with("customXml/") && !p.contains("/_rels/"))
            {
                let Some(properties) = related(part, "/customXmlProps")? else {
                    continue;
                };
                let text = xml_part(parts, &properties)?;
                let doc = Document::parse(&text)?;
                if doc
                    .root_element()
                    .attribute((CUSTOM_XML, "itemID"))
                    .is_some_and(|id| id.eq_ignore_ascii_case(store))
                {
                    found = Some(part.clone());
                    break;
                }
            }
            found
        }
    };
    found.with_context(|| {
        format!(
            "the document data store {store} a content control shows is missing; edit it in Word"
        )
    })
}

/// The element an XPath of a content control binding selects: a path of
/// `prefix:name[n]` steps from the root, with the prefixes of `prefixes`
/// (`xmlns:ns0='...' xmlns:ns1='...'`).
fn bound_element<'a, 'input>(
    doc: &'a Document<'input>,
    prefixes: &str,
    xpath: &str,
) -> Result<Node<'a, 'input>> {
    static PREFIX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"xmlns:([\w.\-]+)\s*=\s*(?:'([^']*)'|"([^"]*)")"#).unwrap()
    });
    static STEP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:([\w.\-]+):)?([\w.\-]+)(?:\[([1-9][0-9]*)\])?$").unwrap()
    });
    let namespaces: BTreeMap<&str, &str> = PREFIX
        .captures_iter(prefixes)
        .map(|c| {
            let uri = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
            (c.get(1).map_or("", |m| m.as_str()), uri)
        })
        .collect();
    let unsupported = || {
        format!(
            "the content control binding {xpath} is not a simple element path ARP can follow; edit it in Word"
        )
    };
    let mut node = doc.root();
    for step in xpath
        .strip_prefix('/')
        .with_context(unsupported)?
        .split('/')
    {
        let captures = STEP.captures(step).with_context(unsupported)?;
        let namespace = match captures.get(1) {
            Some(prefix) => Some(*namespaces.get(prefix.as_str()).with_context(unsupported)?),
            None => None,
        };
        let index: usize = captures.get(3).map_or(Ok(1), |n| n.as_str().parse())?;
        node = node
            .children()
            .filter(|c| {
                c.is_element()
                    && c.tag_name().name() == &captures[2]
                    && c.tag_name().namespace() == namespace
            })
            .nth(index - 1)
            .with_context(|| {
                format!(
                    "the document data {xpath} a content control shows is missing; edit it in Word"
                )
            })?;
    }
    ensure!(
        node.is_element() && !node.children().any(|c| c.is_element()),
        "the document data {xpath} a content control shows is not text; edit it in Word"
    );
    Ok(node)
}

/// `text` with each bound element taking its new text.
fn set_bound_values(text: &str, values: &[(&word::Binding, &String)]) -> Result<String> {
    let doc = Document::parse(text)?;
    let mut edits = vec![];
    for (binding, value) in values {
        let element = bound_element(&doc, &binding.prefixes, &binding.xpath)?;
        let raw = &text[element.range()];
        let name = raw[1..]
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .context("invalid bound element")?;
        let (opening, closing) = match raw.strip_suffix("/>") {
            Some(opening) => (format!("{}>", opening.trim_end()), format!("</{name}>")),
            None => (
                raw[..=raw.find('>').context("invalid bound element")?].to_owned(),
                raw[raw.rfind("</").context("invalid bound element")?..].to_owned(),
            ),
        };
        edits.push((
            element.range(),
            format!("{opening}{}{closing}", xml_text(value)?),
        ));
    }
    let result = splice(text, edits, "document data")?;
    Document::parse(&result)?;
    Ok(result)
}

/// `drawing`, an Excel drawing part, with the text of the shapes of `texts`
/// (by `cNvPr` id, the text before and after) changed run by run, as Word text
/// is: the first run a change touches takes it. Line and paragraph breaks and
/// field text stay as they are.
pub fn edit_shape_texts(
    drawing: &str,
    texts: &BTreeMap<String, (String, String)>,
) -> Result<String> {
    const SPREADSHEET_DRAWING: &str =
        "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let doc = Document::parse(drawing)?;
    let mut edits = vec![];
    let mut found = BTreeSet::new();
    for body in doc
        .descendants()
        .filter(|n| n.has_tag_name((SPREADSHEET_DRAWING, "txBody")))
    {
        let Some(id) = body
            .parent_element()
            .into_iter()
            .flat_map(|shape| shape.children())
            .filter(Node::is_element)
            .flat_map(|properties| properties.children())
            .find(|n| n.is_element() && n.tag_name().name() == "cNvPr")
            .and_then(|n| n.attribute("id"))
        else {
            continue;
        };
        let Some((before, after)) = texts.get(id) else {
            continue;
        };
        // Each line (a paragraph or a line break ends one) with its segments.
        let mut lines: Vec<(String, Vec<word::Segment<'_, '_>>)> = vec![(String::new(), vec![])];
        let mut paragraphs = body
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "p")))
            .peekable();
        while let Some(paragraph) = paragraphs.next() {
            for node in paragraph.descendants() {
                if node.has_tag_name((DRAWING, "t")) {
                    // Field text (a:fld) is filled in by Excel.
                    let run = node
                        .parent()
                        .is_some_and(|p| p.has_tag_name((DRAWING, "r")));
                    let (text, segments) = lines.last_mut().context("shape line")?;
                    let start = text.len();
                    text.push_str(node.text().unwrap_or(""));
                    segments.push(word::Segment {
                        node: run.then_some(node),
                        range: start..text.len(),
                    });
                } else if node.has_tag_name((DRAWING, "br")) {
                    lines.push((String::new(), vec![]));
                }
            }
            if paragraphs.peek().is_some() {
                lines.push((String::new(), vec![]));
            }
        }
        // A copy for older readers (mc:Fallback) may hold other text.
        let text: Vec<&str> = lines.iter().map(|(text, _)| text.as_str()).collect();
        if text.join("\n") != *before {
            continue;
        }
        let new_lines: Vec<&str> = after.split('\n').collect();
        ensure!(
            new_lines.len() == lines.len(),
            "the text of shape {id} must keep its {} line(s); edit its lines in Excel",
            lines.len()
        );
        let mut changed = vec![];
        for (number, ((text, segments), new)) in lines.into_iter().zip(new_lines).enumerate() {
            if text != new {
                let block = word::Block::plain(
                    format!("shape {id} line {}", number + 1),
                    text,
                    segments,
                    "Excel",
                );
                changed.extend(word::edit(&block, new)?);
            }
        }
        for (node, new) in changed {
            let raw = &drawing[node.range()];
            let opening =
                raw[..raw.find('>').context("invalid text element")?].trim_end_matches('/');
            let name = opening
                .trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .context("missing text tag")?;
            edits.push((
                node.range(),
                format!("{opening}>{}</{name}>", xml_text(&new)?),
            ));
        }
        found.insert(id.to_owned());
    }
    if let Some(missing) = texts.keys().find(|id| !found.contains(*id)) {
        bail!(
            "shape {missing} with the text it had on import is missing from the drawing; re-import the workbook"
        );
    }
    let result = splice(drawing, edits, "shape text")?;
    Document::parse(&result)?;
    Ok(result)
}

/// `drawing`, an Excel drawing part, with the runs of the shapes of `edits`
/// (by `cNvPr` id) given the edited font. A run without properties gets them;
/// a copy for older readers (`mc:Fallback`) is edited too.
pub fn edit_shape_fonts(
    drawing: &str,
    edits: &BTreeMap<String, crate::fonts::FontEdit>,
) -> Result<String> {
    const SPREADSHEET_DRAWING: &str =
        "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
    let doc = Document::parse(drawing)?;
    let mut splices = vec![];
    let mut found = BTreeSet::new();
    for body in doc
        .descendants()
        .filter(|n| n.has_tag_name((SPREADSHEET_DRAWING, "txBody")))
    {
        let Some(id) = body
            .parent_element()
            .into_iter()
            .flat_map(|shape| shape.children())
            .filter(Node::is_element)
            .flat_map(|properties| properties.children())
            .find(|n| n.is_element() && n.tag_name().name() == "cNvPr")
            .and_then(|n| n.attribute("id"))
        else {
            continue;
        };
        let Some(edit) = edits.get(id) else {
            continue;
        };
        found.insert(id.to_owned());
        drawing_run_edits(drawing, body, edit, &mut splices)?;
    }
    if let Some(missing) = edits.keys().find(|id| !found.contains(*id)) {
        bail!("shape {missing} has no text in the drawing; re-import the workbook");
    }
    let result = splice(drawing, splices, "shape font")?;
    Document::parse(&result)?;
    Ok(result)
}

/// The edits giving every run of the DrawingML text `body` the font of `edit`,
/// with the paragraph ends that set their own properties.
fn drawing_run_edits(
    source: &str,
    body: Node<'_, '_>,
    edit: &crate::fonts::FontEdit,
    splices: &mut Vec<(std::ops::Range<usize>, String)>,
) -> Result<()> {
    for paragraph in body.children().filter(|n| n.has_tag_name((DRAWING, "p"))) {
        for run in paragraph
            .children()
            .filter(|n| n.has_tag_name((DRAWING, "r")) || n.has_tag_name((DRAWING, "fld")))
        {
            let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, run));
            match run.children().find(|n| n.has_tag_name((DRAWING, "rPr"))) {
                Some(properties) => splices.push((
                    properties.range(),
                    crate::fonts::edited_drawing_run(
                        source,
                        Some(properties),
                        "rPr",
                        prefix,
                        edit,
                    )?,
                )),
                None => {
                    // Run properties come first in a run.
                    let raw = &source[run.range()];
                    let at = run.range().start + raw.find('>').context("invalid run")? + 1;
                    splices.push((
                        at..at,
                        crate::fonts::edited_drawing_run(source, None, "rPr", prefix, edit)?,
                    ));
                }
            }
        }
        if let Some(end) = paragraph
            .children()
            .find(|n| n.has_tag_name((DRAWING, "endParaRPr")))
        {
            let prefix = crate::fonts::prefix_of(crate::fonts::qualified_name(source, end));
            splices.push((
                end.range(),
                crate::fonts::edited_drawing_run(source, Some(end), "endParaRPr", prefix, edit)?,
            ));
        }
    }
    Ok(())
}

/// The namespace prefix of a qualified element name with its colon, such as `w:`.
fn element_prefix(name: &str) -> String {
    name.rsplit_once(':')
        .map_or_else(String::new, |(prefix, _)| format!("{prefix}:"))
}

/// Word run content for `text`: tabs and line breaks become `w:tab` and
/// `w:br`, and the text between them text elements opened with `opening`.
fn word_run_content(text: &str, opening: &str, name: &str, prefix: &str) -> Result<String> {
    let mut content = String::new();
    let mut rest = text;
    loop {
        let end = rest.find(['\n', '\t']).unwrap_or(rest.len());
        if end > 0 {
            content.push_str(&format!("{opening}>{}</{name}>", xml_text(&rest[..end])?));
        }
        let Some(separator) = rest[end..].chars().next() else {
            return Ok(content);
        };
        let element = if separator == '\t' { "tab" } else { "br" };
        content.push_str(&format!("<{prefix}{element}/>"));
        rest = &rest[end + 1..];
    }
}

impl TextSource {
    /// The document with `fonts` (each a `sheet` and `cell` of the original
    /// and the font properties `after` sets) written to the runs of the text,
    /// checked by reading it back; None without font edits.
    fn with_fonts(&self, fonts: &[Value]) -> Result<Option<TextSource>> {
        if fonts.is_empty() {
            return Ok(None);
        }
        let TextBackend::Office(parts) = &self.backend else {
            bail!("font edits are written back to Excel, Word and PowerPoint documents only");
        };
        let mut by_part: BTreeMap<&str, BTreeMap<usize, crate::fonts::FontEdit>> = BTreeMap::new();
        let mut expected: BTreeMap<(&str, &str), crate::fonts::FontEdit> = BTreeMap::new();
        for change in fonts {
            let name = string(&change["sheet"])?;
            let address = string(&change["cell"])?;
            let sheet = self
                .sheets
                .iter()
                .find(|s| s["name"] == name)
                .with_context(|| format!("font edit page {name} is missing"))?;
            let index = array(&sheet["cells"])?
                .iter()
                .position(|cell| cell["address"] == address)
                .with_context(|| format!("font edit target {name}!{address} is missing"))?;
            let edit = crate::fonts::FontEdit::parse(&change["after"])?;
            if word(&self.format) {
                word_fonts::ensure_word_edit(&edit)?;
            }
            ensure!(
                by_part
                    .entry(string(&sheet["part"])?)
                    .or_default()
                    .insert(index, edit.clone())
                    .is_none(),
                "duplicate font edit of {name}!{address}"
            );
            expected.insert((name, address), edit);
        }
        let mut patched = BTreeMap::new();
        for (part, edits) in by_part {
            let (text, encoding) = part_text(parts.get(part).context("missing document part")?)?;
            let xml = Document::parse(&text)?;
            let blocks = if word(&self.format) {
                word::layout(&xml)?.blocks
            } else {
                slide_text::layout(&xml)?.blocks
            };
            let mut cache = BTreeMap::new();
            let mut splices = vec![];
            for (index, edit) in edits {
                let block = blocks.get(index).context("text block disappeared")?;
                splices.extend(if word(&self.format) {
                    word_fonts::run_edits(&text, block, &edit, &self.format, &mut cache)?
                } else {
                    slide_fonts::run_edits(&text, block, &edit, &mut cache)?
                });
            }
            let result = splice(&text, splices, "font")?;
            Document::parse(&result)?;
            patched.insert(part.to_owned(), encode_part(&result, encoding));
        }
        let raw = crate::excel::archive_bytes(&self.raw, &patched)?;
        let Source::Text(written) =
            Source::from_bytes(Path::new(&format!("document.{}", self.format)), raw)?
        else {
            bail!("font edit changed the document format");
        };
        ensure!(
            written.sheets.len() == self.sheets.len(),
            "font edit changed the pages of the document"
        );
        for (old, new) in self.sheets.iter().zip(&written.sheets) {
            let name = string(&old["name"])?;
            let (old_cells, new_cells) = (array(&old["cells"])?, array(&new["cells"])?);
            ensure!(
                old_cells.len() == new_cells.len(),
                "font edit changed the text of {name}"
            );
            for (before, after) in old_cells.iter().zip(new_cells) {
                let address = string(&before["address"])?;
                let wanted = match expected.get(&(name, address)) {
                    Some(edit) => edit.applied(&before["font"]),
                    None => before["font"].clone(),
                };
                ensure!(
                    after["value"] == before["value"]
                        && after["address"] == before["address"]
                        && crate::fonts::same_font(&after["font"], &wanted),
                    "font read-back failed for {name}!{address}: expected {wanted}, found {}",
                    after["font"]
                );
            }
        }
        Ok(Some(written))
    }

    /// Writes the document with `changes` to its text and its `operations`:
    /// for Word, paragraphs and table rows inserted or deleted, and for
    /// PowerPoint, slides copied or deleted. A change names its original cell
    /// (`source_cell`, else `cell`), or for an inserted row its `insertion`,
    /// `offset` and `column`; a change to an inserted slide names the slide.
    #[cfg(test)]
    fn patch(&self, output: &Path, operations: &[Value], changes: &[Value]) -> Result<Value> {
        self.patch_with(output, operations, changes, &BTreeMap::new())
    }

    /// [`TextSource::patch`] with the image `assets` (by asset path) of the
    /// pictures shape operations add.
    fn patch_with(
        &self,
        output: &Path,
        operations: &[Value],
        changes: &[Value],
        assets: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Value> {
        ensure!(
            !matches!(self.backend, TextBackend::Native),
            "text documents use direct editing; edit the original and re-import with the same document ID (export/apply is not supported)"
        );
        ensure!(!output.exists(), "output already exists");
        ensure!(
            operations
                .iter()
                .all(|operation| if is_presentation_operation(operation) {
                    self.format == "pptx"
                } else {
                    word(&self.format) || self.format == "pptx"
                }),
            "row operations apply to Excel, Word and PowerPoint documents, slide and shape operations to PowerPoint"
        );
        let slide_operations = parse_slide_operations(operations, &self.sheets)?;
        let shape_operations = parse_shape_operations(operations)?;
        let mut shapes_by_sheet: BTreeMap<&str, Vec<&ShapeOperation>> = BTreeMap::new();
        for operation in &shape_operations {
            shapes_by_sheet
                .entry(operation.slide.as_str())
                .or_default()
                .push(operation);
        }
        let restructured = match &self.backend {
            TextBackend::Office(_) if !slide_operations.is_empty() => {
                let complete = office_parts(&self.raw, true)?;
                Some(slides::restructure(
                    &complete,
                    &self.sheets,
                    &slide_operations,
                )?)
            }
            _ => None,
        };
        // The pages as the slide operations leave them, with their parts.
        let view = if slide_operations.is_empty() {
            Cow::Borrowed(&self.sheets)
        } else {
            Cow::Owned(slide_view(&self.sheets, &self.layouts, &slide_operations)?)
        };
        let mut pages = vec![];
        for sheet in view.iter() {
            let name = string(&sheet["name"])?;
            let part = match restructured.as_ref().and_then(|r| r.inserted.get(name)) {
                Some(part) => part.clone(),
                None => string(&sheet["part"])?.to_owned(),
            };
            pages.push((name.to_owned(), part));
        }
        let operations = crate::excel::parse_operations(operations, &view)?;
        let mut operations_by_sheet: BTreeMap<&str, Vec<&crate::excel::StructuralOperation>> =
            BTreeMap::new();
        for operation in &operations {
            operations_by_sheet
                .entry(&operation.sheet)
                .or_default()
                .push(operation);
        }
        // Each page and address is looked up for every changed text block.
        let mut sheet_index = BTreeMap::new();
        for sheet in view.iter().chain(self.sheets.iter()) {
            let name = string(&sheet["name"])?;
            if sheet_index.contains_key(name) {
                continue;
            }
            let mut addresses = BTreeMap::new();
            for (index, cell) in array(&sheet["cells"])?.iter().enumerate() {
                addresses.entry(string(&cell["address"])?).or_insert(index);
            }
            sheet_index.insert(name, (sheet, addresses));
        }
        // The replaced texts of each page by name, to check the output against.
        let mut by_page: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        let mut replacements: BTreeMap<String, BTreeMap<usize, String>> = BTreeMap::new();
        let mut inserted: BTreeMap<String, word::InsertedText> = BTreeMap::new();
        for change in changes {
            if change["inserted"] == true {
                let key = (
                    string(&change["insertion"])?.to_owned(),
                    u32::try_from(change["offset"].as_u64().context("inserted offset")?)?,
                    string(&change["column"])?.to_owned(),
                );
                let value = string(&change["after"])
                    .context("text replacement must be a string")?
                    .to_owned();
                inserted
                    .entry(string(&change["sheet"])?.to_owned())
                    .or_default()
                    .insert(key, value);
                continue;
            }
            let name = string(&change["sheet"])?;
            let (sheet, addresses) = sheet_index.get(name).context("missing text container")?;
            let part = match restructured.as_ref().and_then(|r| r.inserted.get(name)) {
                Some(part) => part.as_str(),
                None => string(&sheet["part"])?,
            };
            ensure!(
                !restructured
                    .as_ref()
                    .is_some_and(|r| r.removed.contains(part)),
                "{name} is deleted by a delete_slide operation; its text cannot be edited"
            );
            let address = change.get("source_cell").unwrap_or(&change["cell"]);
            let cell = *addresses
                .get(string(address)?)
                .context("missing text target")?;
            ensure!(
                sheet["cells"][cell]["value"] == change["before"],
                "text baseline mismatch"
            );
            let value = string(&change["after"])
                .context("text replacement must be a string; use an empty string to clear text")?;
            ensure!(
                replacements
                    .entry(part.into())
                    .or_default()
                    .insert(cell, value.into())
                    .is_none(),
                "duplicate text replacement"
            );
            by_page
                .entry(name.to_owned())
                .or_default()
                .insert(cell, value.into());
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
                let (mut patched, removed) = match &restructured {
                    Some(restructured) => {
                        (restructured.patched.clone(), restructured.removed.clone())
                    }
                    None => (BTreeMap::new(), BTreeSet::new()),
                };
                // The image of each picture a shape operation adds, as a media
                // part related from its slide.
                let page_parts: BTreeMap<&str, &str> = pages
                    .iter()
                    .map(|(name, part)| (name.as_str(), part.as_str()))
                    .collect();
                let pictures = self.add_pictures(
                    &shape_operations,
                    &page_parts,
                    restructured.as_ref(),
                    assets,
                    &mut patched,
                )?;
                // Every Word part is read, since a content control bound to
                // edited document data shows it wherever it is; PowerPoint reads
                // the pages whose text or rows change.
                let pages: Vec<(String, String)> = pages
                    .into_iter()
                    .filter(|(name, part)| {
                        word(&self.format)
                            || replacements.contains_key(part)
                            || operations_by_sheet.contains_key(name.as_str())
                            || shapes_by_sheet.contains_key(name.as_str())
                    })
                    .collect();
                let read: Vec<String> = pages.iter().map(|(_, part)| part.clone()).collect();
                let texts = read
                    .iter()
                    .map(|part| {
                        let bytes = match &restructured {
                            Some(restructured) => restructured.part(parts, part),
                            None => parts.get(part).map(Vec::as_slice),
                        };
                        part_text(bytes.context("missing document part")?)
                    })
                    .collect::<Result<Vec<_>>>()?;
                let docs = texts
                    .iter()
                    .map(|(text, _)| Document::parse(text))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut edits: Vec<Vec<(Node<'_, '_>, String)>> = vec![vec![]; read.len()];
                // Text written into empty slide placeholders, by part.
                let mut fills: Vec<Vec<(std::ops::Range<usize>, String)>> =
                    vec![vec![]; read.len()];
                // Paragraphs and table rows inserted or deleted, by part.
                let mut structural = vec![vec![]; read.len()];
                // Shapes changed, added or deleted, by part, and the IDs of the added.
                let mut shaped: Vec<Vec<(std::ops::Range<usize>, String)>> =
                    vec![vec![]; read.len()];
                let mut added_shapes: BTreeMap<String, (String, u64)> = BTreeMap::new();
                let no_values = word::InsertedText::new();
                for (index, part) in read.iter().enumerate() {
                    let sheet_name = pages[index].0.as_str();
                    let part_operations = operations_by_sheet
                        .get(sheet_name)
                        .cloned()
                        .unwrap_or_default();
                    let changes = replacements.get(part);
                    let shape_operations = shapes_by_sheet.get(sheet_name);
                    if changes.is_none() && part_operations.is_empty() && shape_operations.is_none()
                    {
                        continue;
                    }
                    let xml = &docs[index];
                    if word(&self.format) {
                        let fields = word_field_results(xml);
                        let layout = word::layout(xml)?;
                        structural[index] = layout
                            .restructure(
                                &texts[index].0,
                                &part_operations,
                                inserted.get(sheet_name).unwrap_or(&no_values),
                            )
                            .with_context(|| format!("{sheet_name} ({part})"))?;
                        for (i, text) in changes.into_iter().flatten() {
                            let block = layout.blocks.get(*i).context("text block disappeared")?;
                            for (node, new) in word::edit(block, text)? {
                                ensure!(
                                    !fields.contains(&node.range().start),
                                    "Word field result text cannot be edited because Word recalculates it (date, page number, table of contents, cross-reference, etc.): {part} {}; edit the field in Word",
                                    block.address
                                );
                                edits[index].push((node, new));
                            }
                        }
                    } else {
                        let layout = slide_text::layout(xml)?;
                        structural[index] = layout
                            .restructure(
                                &texts[index].0,
                                &part_operations,
                                inserted.get(sheet_name).unwrap_or(&no_values),
                            )
                            .with_context(|| format!("{sheet_name} ({part})"))?;
                        for (i, text) in changes.into_iter().flatten() {
                            let block = layout.blocks.get(*i).context("text block disappeared")?;
                            if let Some(paragraph) = block.empty {
                                if !text.is_empty() {
                                    fills[index].push(slide_text::fill_empty(
                                        &texts[index].0,
                                        paragraph,
                                        text,
                                    )?);
                                }
                                continue;
                            }
                            for (node, new) in word::edit(block, text)? {
                                // Slide numbers and dates are fields PowerPoint fills in again.
                                ensure!(
                                    !node.ancestors().any(|a| a.has_tag_name((DRAWING, "fld"))),
                                    "PowerPoint field text (slide number, date, etc.) cannot be edited because PowerPoint recalculates it: {sheet_name} {}; edit the field in PowerPoint",
                                    block.address
                                );
                                edits[index].push((node, new));
                            }
                        }
                        if let Some(shape_operations) = shape_operations {
                            // A copied slide's shapes keep the IDs of the slide it copies.
                            let origin = view
                                .iter()
                                .find(|sheet| sheet["name"] == sheet_name)
                                .and_then(|sheet| sheet["copy_of"].as_str())
                                .and_then(|origin| self.sheets.iter().find(|s| s["name"] == origin))
                                .and_then(|sheet| sheet["part"].as_str())
                                .unwrap_or(part);
                            let (edits, added) = slide_shapes::slide_edits(
                                &texts[index].0,
                                xml,
                                part,
                                origin,
                                shape_operations,
                                &layout.blocks,
                                pictures.get(part.as_str()).unwrap_or(&BTreeMap::new()),
                            )
                            .with_context(|| sheet_name.to_owned())?;
                            shaped[index] = edits;
                            for (operation, id) in added {
                                added_shapes.insert(operation, (sheet_name.to_owned(), id));
                            }
                        }
                    }
                }
                let bound = if word(&self.format) {
                    bound_data_edits(&read, &docs, &mut edits)?
                } else {
                    BTreeMap::new()
                };
                for ((index, edits), part) in edits.into_iter().enumerate().zip(&read) {
                    if edits.is_empty()
                        && structural[index].is_empty()
                        && fills[index].is_empty()
                        && shaped[index].is_empty()
                    {
                        continue;
                    }
                    let (original, encoding) = &texts[index];
                    let original = original.as_ref();
                    let encoding = *encoding;
                    let part = part.clone();
                    let mut targets = vec![];
                    let mut hidden_cache = BTreeMap::new();
                    for (node, text) in edits {
                        for copy in hidden_copies(node, &self.format, &mut hidden_cache)? {
                            targets.push((copy, text.clone()));
                        }
                        targets.push((node, text));
                    }
                    // Insertions sort ahead of an element removed at the same place.
                    let mut text_edits = std::mem::take(&mut fills[index]);
                    text_edits.extend(std::mem::take(&mut structural[index]));
                    text_edits.extend(std::mem::take(&mut shaped[index]));
                    for (node, text) in targets {
                        if !word(&self.format) {
                            text_edits.push(slide_text::replacement(original, node, &text)?);
                            continue;
                        }
                        let raw = &original[node.range()];
                        let end = raw.find('>').context("invalid text element")?;
                        let opening = &raw[..end];
                        let name = opening
                            .trim_start_matches('<')
                            .split(|c: char| c.is_whitespace() || c == '/')
                            .next()
                            .context("missing text tag")?;
                        if word::text_break(node) {
                            // A tab or line break of the run becomes the new text.
                            let prefix = element_prefix(name);
                            let replacement = word_run_content(
                                &text,
                                &format!("<{prefix}t xml:space=\"preserve\""),
                                &format!("{prefix}t"),
                                &prefix,
                            )?;
                            text_edits.push((node.range(), replacement));
                            continue;
                        }
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
                        let replacement = if word(&self.format) && text.contains(['\n', '\t']) {
                            word_run_content(&text, &opening, name, &element_prefix(name))?
                        } else {
                            format!("{opening}>{}</{name}>", xml_text(&text)?)
                        };
                        text_edits.push((node.range(), replacement));
                    }
                    let result = splice(original, text_edits, "text")?;
                    Document::parse(&result)?;
                    patched.insert(part, encode_part(&result, encoding));
                }
                // The data bound controls show takes their new text.
                let mut by_part: BTreeMap<String, Vec<(&word::Binding, &String)>> = BTreeMap::new();
                let mut stores: BTreeMap<&str, String> = BTreeMap::new();
                for (binding, value) in bound.values() {
                    let part = match stores.get(binding.store.as_str()) {
                        Some(part) => part.clone(),
                        None => {
                            let part = bound_data_part(parts, &binding.store)?;
                            stores.insert(binding.store.as_str(), part.clone());
                            part
                        }
                    };
                    by_part.entry(part).or_default().push((binding, value));
                }
                for (part, values) in by_part {
                    let (text, encoding) = part_text(&parts[&part])?;
                    let updated = set_bound_values(&text, &values)?;
                    patched.insert(part, encode_part(&updated, encoding));
                }
                if restructured.is_none() {
                    validate_office_binary(&self.raw)?;
                }
                crate::excel::write_archive_without(&self.raw, output, &patched, &removed)?;
                // The text of an added shape follows the slide's other text.
                let mut appended: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for operation in &shape_operations {
                    if let ShapeEdit::Add {
                        text: Some(text), ..
                    } = &operation.edit
                    {
                        appended.entry(operation.slide.clone()).or_default().extend(
                            text.split('\n')
                                .filter(|l| !l.is_empty())
                                .map(str::to_owned),
                        );
                    }
                }
                if let Some(restructured) = &restructured {
                    let rows_changed: BTreeSet<&str> =
                        operations.iter().map(|o| o.sheet.as_str()).collect();
                    self.ensure_slides_read_back(
                        output,
                        restructured,
                        &slide_operations,
                        &by_page,
                        &rows_changed,
                        &appended,
                    )
                    .inspect_err(|_| {
                        let _ = fs::remove_file(output);
                    })?;
                }
                if !shape_operations.is_empty() {
                    let order = match &restructured {
                        Some(restructured) => restructured
                            .order
                            .iter()
                            .map(|(slide, _)| slide.clone())
                            .collect(),
                        None => self
                            .sheets
                            .iter()
                            .filter_map(|s| s["name"].as_str())
                            .filter(|name| name.starts_with("slide-"))
                            .map(str::to_owned)
                            .collect::<Vec<_>>(),
                    };
                    slide_shapes::ensure_read_back(
                        output,
                        &order,
                        &shape_operations,
                        &added_shapes,
                        assets,
                    )
                    .inspect_err(|_| {
                        let _ = fs::remove_file(output);
                    })?;
                }
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
                            let raw = original.get_page_content_with_limit(id, MAX_PAGE)?;
                            let mut content = Content::decode_strict(&raw)?;
                            pdf_text(original, id, &mut content, changes, Some(&glyphs))?;
                            // Always allocate a new stream: an original stream may be shared by pages.
                            let stream = doc.add_object(Stream::new(
                                lopdf::Dictionary::new(),
                                encode_content(&raw, &content)
                                    .with_context(|| format!("PDF page {page}"))?,
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
        let mut report = json!({"format":self.format,"text_changes":changes.len(),"layout_review_required":true});
        if let Some(restructured) = &restructured {
            report["slide_order"] = json!(
                restructured
                    .order
                    .iter()
                    .map(|(slide, _)| slide)
                    .collect::<Vec<_>>()
            );
        }
        Ok(report)
    }

    /// The relationship ID of each picture asset in the slide parts of the
    /// pictures `operations` add or replace: a new media part related from the
    /// slide, written into `patched`.
    fn add_pictures(
        &self,
        operations: &[ShapeOperation],
        pages: &BTreeMap<&str, &str>,
        restructured: Option<&slides::Restructured>,
        assets: &BTreeMap<String, Vec<u8>>,
        patched: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
        let mut pictures: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        if !operations.iter().any(|o| o.asset().is_some()) {
            return Ok(pictures);
        }
        let TextBackend::Office(parts) = &self.backend else {
            bail!("pictures are added to PowerPoint slides only");
        };
        let complete = office_parts(&self.raw, true)?;
        let mut taken: BTreeSet<String> = complete
            .keys()
            .chain(patched.keys())
            .map(|name| name.to_lowercase())
            .collect();
        let mut media: BTreeMap<String, String> = BTreeMap::new();
        for operation in operations {
            let Some(asset) = operation.asset() else {
                continue;
            };
            let bytes = assets
                .get(asset)
                .with_context(|| format!("image asset missing: {asset}"))?;
            let slide = *pages
                .get(operation.slide.as_str())
                .with_context(|| format!("{} is not a slide here", operation.slide))?;
            let part = match media.get(asset) {
                Some(part) => part.clone(),
                None => {
                    let part = (1..)
                        .map(|n| format!("ppt/media/arp-image{n}.png"))
                        .find(|name| !taken.contains(&name.to_lowercase()))
                        .unwrap();
                    taken.insert(part.to_lowercase());
                    patched.insert(part.clone(), bytes.clone());
                    media.insert(asset.to_owned(), part.clone());
                    part
                }
            };
            if pictures.get(slide).is_some_and(|p| p.contains_key(asset)) {
                continue;
            }
            let (directory, file) = slide
                .rsplit_once('/')
                .context("slide part without folder")?;
            let rels = format!("{directory}/_rels/{file}.rels");
            let text = match patched.get(&rels) {
                Some(bytes) => part_text(bytes)?.0.into_owned(),
                None => match restructured
                    .and_then(|r| r.part(parts, &rels))
                    .or_else(|| parts.get(&rels).map(Vec::as_slice))
                {
                    Some(bytes) => part_text(bytes)?.0.into_owned(),
                    None => format!(r#"<Relationships xmlns="{PACKAGE_REL}"></Relationships>"#),
                },
            };
            let xml = Document::parse(&text)?;
            let used: BTreeSet<&str> = xml
                .descendants()
                .filter_map(|n| n.attribute("Id"))
                .collect();
            let id = (1..)
                .map(|n| format!("rIdArp{n}"))
                .find(|id| !used.contains(id.as_str()))
                .unwrap();
            let target = format!("../media/{}", part.rsplit('/').next().unwrap_or(&part));
            let end = text
                .rfind("</")
                .context("relationships part without closing tag")?;
            let mut updated = text.clone();
            updated.insert_str(
                end,
                &format!(
                    r#"<Relationship Id="{id}" Type="{}" Target="{target}"/>"#,
                    slide_shapes::IMAGE
                ),
            );
            patched.insert(rels, updated.into_bytes());
            pictures
                .entry(slide.to_owned())
                .or_default()
                .insert(asset.to_owned(), id);
        }
        // PNG parts need their content type.
        let types = "[Content_Types].xml";
        let text = match patched.get(types).or_else(|| parts.get(types)) {
            Some(bytes) => part_text(bytes)?.0.into_owned(),
            None => bail!("the presentation has no content types"),
        };
        let xml = Document::parse(&text)?;
        if !xml.descendants().any(|n| {
            n.tag_name().name() == "Default"
                && n.attribute("Extension")
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        }) {
            let end = text
                .rfind("</")
                .context("content types without closing tag")?;
            let mut updated = text.clone();
            updated.insert_str(end, r#"<Default Extension="png" ContentType="image/png"/>"#);
            patched.insert(types.to_owned(), updated.into_bytes());
        }
        Ok(pictures)
    }

    /// Reads the written presentation back and compares its slides and notes
    /// pages, in order, with the pages the operations and `edits` should make.
    /// A page whose rows change (`rows_changed`) is compared by its place only.
    fn ensure_slides_read_back(
        &self,
        output: &Path,
        restructured: &slides::Restructured,
        operations: &[SlideOperation],
        edits: &BTreeMap<String, BTreeMap<usize, String>>,
        rows_changed: &BTreeSet<&str>,
        appended: &BTreeMap<String, Vec<String>>,
    ) -> Result<()> {
        // Every page as the operations leave it: kept, copied or made from a layout.
        let view = slide_view(&self.sheets, &self.layouts, operations)?;
        let sheets: BTreeMap<&str, &Value> = view
            .iter()
            .map(|sheet| Ok((string(&sheet["name"])?, sheet)))
            .collect::<Result<_>>()?;
        let expected_page = |name: &str| -> Result<Option<Vec<String>>> {
            if rows_changed.contains(name) {
                return Ok(None);
            }
            let sheet = sheets.get(name).context("missing text container")?;
            let mut values = array(&sheet["cells"])?
                .iter()
                .map(|cell| cell["value"].as_str().unwrap_or("").to_owned())
                .collect::<Vec<_>>();
            for (index, text) in edits.get(name).into_iter().flatten() {
                values[*index] = text.clone();
            }
            values.extend(appended.get(name).into_iter().flatten().cloned());
            Ok(Some(values))
        };
        let mut expected = vec![];
        for (slide, _) in &restructured.order {
            expected.push(expected_page(slide)?);
        }
        for (_, notes) in &restructured.order {
            if let Some(notes) = notes {
                expected.push(expected_page(notes)?);
            }
        }
        let Source::Text(written) = Source::open(output)? else {
            bail!("the written presentation is not read as a presentation");
        };
        let actual = written
            .sheets
            .iter()
            .map(|sheet| {
                Ok(array(&sheet["cells"])?
                    .iter()
                    .map(|cell| cell["value"].as_str().unwrap_or("").to_owned())
                    .collect::<Vec<_>>())
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(&expected)
                    .all(|(actual, expected)| expected.as_ref().is_none_or(|e| e == actual)),
            "the written presentation does not read back as the slide operations and edits make it; nothing is written"
        );
        for ((slide, _), (sheet, hidden)) in restructured
            .order
            .iter()
            .zip(written.sheets.iter().zip(&restructured.hidden))
        {
            ensure!(
                (sheet["state"] == "hidden") == *hidden,
                "{slide} does not read back {} in the slide show; nothing is written",
                if *hidden { "hidden" } else { "shown" }
            );
        }
        Ok(())
    }
}

/// The byte ranges of the inline images (`BI` ... `ID` data `EI`) of a page's
/// content stream, outside strings and comments. `lengths` gives, in order,
/// the data length of each image lopdf could read, which marks its end
/// exactly; otherwise the image ends at the first `EI` set off by whitespace,
/// as PDF readers find it.
fn inline_images(raw: &[u8], lengths: &[Option<usize>]) -> Result<Vec<std::ops::Range<usize>>> {
    let space = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b'\x0c' | b'\0');
    let delimiter = |b: u8| space(b) || b"()<>[]{}/%".contains(&b);
    let token = |at: usize, word: &[u8]| {
        raw[at..].starts_with(word)
            && (at == 0 || delimiter(raw[at - 1]))
            && raw.get(at + word.len()).is_none_or(|b| delimiter(*b))
    };
    // The end of the name, string, comment or hex string starting at `at`.
    let skip = |at: usize| -> Option<usize> {
        match raw[at] {
            b'/' => Some(
                raw[at + 1..]
                    .iter()
                    .position(|b| delimiter(*b))
                    .map_or(raw.len(), |p| at + 1 + p),
            ),
            b'%' => Some(
                raw[at..]
                    .iter()
                    .position(|b| matches!(b, b'\r' | b'\n'))
                    .map_or(raw.len(), |p| at + p),
            ),
            b'(' => {
                let (mut depth, mut i) = (0usize, at);
                while i < raw.len() {
                    match raw[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1);
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                Some(raw.len())
            }
            b'<' if raw.get(at + 1) != Some(&b'<') => Some(
                raw[at..]
                    .iter()
                    .position(|b| *b == b'>')
                    .map_or(raw.len(), |p| at + p + 1),
            ),
            _ => None,
        }
    };
    let mut images = vec![];
    let mut i = 0;
    while i < raw.len() {
        if let Some(end) = skip(i) {
            i = end;
            continue;
        }
        if !token(i, b"BI") {
            i += 1;
            continue;
        }
        let start = i;
        i += 2;
        while i < raw.len() && !token(i, b"ID") {
            i = skip(i).unwrap_or(i + 1);
        }
        ensure!(i < raw.len(), "a PDF inline image has no ID");
        let mut data = i + 2;
        let exact = lengths
            .get(images.len())
            .copied()
            .flatten()
            .and_then(|length| {
                while data < raw.len() && space(raw[data]) {
                    data += 1;
                }
                let mut end = data.checked_add(length)?;
                while end < raw.len() && space(raw[end]) {
                    end += 1;
                }
                token(end, b"EI").then_some(end + 2)
            });
        let end = match exact {
            Some(end) => end,
            None => (i + 3..raw.len().saturating_sub(1))
                .find(|&at| space(raw[at - 1]) && token(at, b"EI"))
                .map(|at| at + 2)
                .context("a PDF inline image has no EI")?,
        };
        images.push(start..end);
        i = end;
    }
    Ok(images)
}

/// Encodes `content` decoded from `raw`, copying its inline images from `raw`
/// as they were: lopdf would write one as a stream object, and drops those it
/// cannot read.
fn encode_content(raw: &[u8], content: &Content) -> Result<Vec<u8>> {
    let lengths: Vec<_> = content
        .operations
        .iter()
        .filter(|o| o.operator == "BI")
        .map(|o| match o.operands.first() {
            Some(Object::Stream(stream)) => Some(stream.content.len()),
            _ => None,
        })
        .collect();
    let images = inline_images(raw, &lengths)?;
    ensure!(
        images.len() == lengths.len(),
        "the inline images of a PDF page could not be told apart from its other drawing commands; edit the text in a PDF editor"
    );
    let mut output = vec![];
    let mut pending = vec![];
    let mut images = images.into_iter();
    let flush =
        |output: &mut Vec<u8>, pending: &mut Vec<lopdf::content::Operation>| -> Result<()> {
            if !pending.is_empty() {
                output.extend(
                    Content {
                        operations: std::mem::take(pending),
                    }
                    .encode()?,
                );
                output.push(b'\n');
            }
            Ok(())
        };
    for operation in &content.operations {
        if operation.operator == "BI" {
            flush(&mut output, &mut pending)?;
            output.extend_from_slice(&raw[images.next().context("inline image missing")?]);
            output.push(b'\n');
        } else {
            pending.push(operation.clone());
        }
    }
    flush(&mut output, &mut pending)?;
    Ok(output)
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

/// The graphics state of a PDF page that decides how text looks: the
/// transformation (`cm`), fill and stroke colours and the text font and size.
#[derive(Clone)]
struct PdfLook {
    matrix: [f64; 6],
    fill: Option<String>,
    stroke: Option<String>,
    fill_space: Vec<u8>,
    stroke_space: Vec<u8>,
    size: f64,
}

impl Default for PdfLook {
    fn default() -> Self {
        // Colours start black in DeviceGray.
        Self {
            matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            fill: Some("000000".into()),
            stroke: Some("000000".into()),
            fill_space: b"DeviceGray".to_vec(),
            stroke_space: b"DeviceGray".to_vec(),
            size: 0.0,
        }
    }
}

/// `a` then `b`, as PDF multiplies matrices.
fn pdf_multiply(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn pdf_numbers(operands: &[Object]) -> Vec<f64> {
    operands
        .iter()
        .filter_map(|o| o.as_float().ok().map(f64::from))
        .collect()
}

/// A colour of a device colour space (Gray, RGB or CMYK components) as
/// `RRGGBB`; colours of other spaces are not resolved.
fn pdf_color(space: &[u8], components: &[f64]) -> Option<String> {
    let channel = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let hex =
        |r: f64, g: f64, b: f64| format!("{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b));
    match (space, components) {
        (b"DeviceGray" | b"G" | b"CalGray", [gray]) => Some(hex(*gray, *gray, *gray)),
        (b"DeviceRGB" | b"RGB" | b"CalRGB", [r, g, b]) => Some(hex(*r, *g, *b)),
        (b"DeviceCMYK" | b"CMYK", [c, m, y, k]) => Some(hex(
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        )),
        _ => None,
    }
}

/// The name of a PDF font (`BaseFont`) without the prefix of an embedded
/// subset (`ABCDEF+`); a name that is not UTF-8 is not given.
fn pdf_font_name(font: &lopdf::Dictionary) -> Option<String> {
    let name = font.get(b"BaseFont").and_then(Object::as_name).ok()?;
    let name = if subset_font(font) { &name[7..] } else { name };
    std::str::from_utf8(name)
        .ok()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// Decodes the page's text strings with the font each is drawn in, and with
/// `changes` replaces the strings at those indexes. `glyphs` (from
/// [`subset_glyphs`]) limits a replacement drawn with an embedded subset font
/// to the codes that subset is known to hold.
fn pdf_text(
    doc: &Pdf,
    page: lopdf::ObjectId,
    content: &mut Content<Vec<lopdf::content::Operation>>,
    changes: &BTreeMap<usize, String>,
    glyphs: Option<&BTreeMap<usize, BTreeSet<Vec<u8>>>>,
) -> Result<Vec<(String, Value)>> {
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
    let mut look = PdfLook::default();
    let mut text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut stack = vec![];
    let mut values = vec![];
    for operation in &mut content.operations {
        let numbers = pdf_numbers(&operation.operands);
        match operation.operator.as_str() {
            "cm" if numbers.len() == 6 => {
                let m = [
                    numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                ];
                look.matrix = pdf_multiply(m, look.matrix);
            }
            "BT" => text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            "Tm" if numbers.len() == 6 => {
                text_matrix = [
                    numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
                ];
            }
            "g" | "rg" | "k" => {
                look.fill_space = match operation.operator.as_str() {
                    "g" => b"DeviceGray".to_vec(),
                    "rg" => b"DeviceRGB".to_vec(),
                    _ => b"DeviceCMYK".to_vec(),
                };
                look.fill = pdf_color(&look.fill_space, &numbers);
            }
            "G" | "RG" | "K" => {
                look.stroke_space = match operation.operator.as_str() {
                    "G" => b"DeviceGray".to_vec(),
                    "RG" => b"DeviceRGB".to_vec(),
                    _ => b"DeviceCMYK".to_vec(),
                };
                look.stroke = pdf_color(&look.stroke_space, &numbers);
            }
            "cs" | "CS" => {
                let space = operation
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .unwrap_or(b"")
                    .to_vec();
                // A new colour space starts at its initial colour; device spaces start black.
                let black = match space.as_slice() {
                    b"DeviceGray" | b"G" | b"CalGray" => pdf_color(&space, &[0.0]),
                    b"DeviceRGB" | b"RGB" | b"CalRGB" => pdf_color(&space, &[0.0, 0.0, 0.0]),
                    b"DeviceCMYK" | b"CMYK" => pdf_color(&space, &[0.0, 0.0, 0.0, 1.0]),
                    _ => None,
                };
                if operation.operator == "cs" {
                    look.fill_space = space;
                    look.fill = black;
                } else {
                    look.stroke_space = space;
                    look.stroke = black;
                }
            }
            "sc" | "scn" => look.fill = pdf_color(&look.fill_space, &numbers),
            "SC" | "SCN" => look.stroke = pdf_color(&look.stroke_space, &numbers),
            "Tf" => {
                font = operation
                    .operands
                    .first()
                    .context("missing PDF font")?
                    .as_name()?
                    .to_vec();
                look.size = numbers.last().copied().unwrap_or(0.0);
            }
            "Tr" => {
                render = operation
                    .operands
                    .first()
                    .and_then(|mode| mode.as_i64().ok())
                    .unwrap_or(0);
            }
            "q" => stack.push((font.clone(), render, look.clone())),
            "Q" => {
                (font, render, look) = stack.pop().context("unbalanced PDF graphics state")?;
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
                // The size on the page: the font size scaled by the text and
                // transformation matrices; the colour of what the render mode draws.
                let shown = pdf_multiply(text_matrix, look.matrix);
                let size = look.size.abs() * (shown[2] * shown[2] + shown[3] * shown[3]).sqrt();
                let name = pdf_font_name(font_dictionary);
                let color = match render {
                    1 | 5 => look.stroke.clone(),
                    3 | 7 => None,
                    _ => look.fill.clone(),
                };
                let look_of = crate::fonts::element_font(
                    &[crate::fonts::RunFont {
                        latin: name.clone(),
                        east_asian: name,
                        size: (size > 0.0).then_some(size),
                        color: color.map(crate::fonts::Color::rgb),
                    }],
                    &crate::fonts::RunFont::default(),
                );
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
                    values.push((value, look_of.clone()));
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
    use std::io::{Cursor, Read, Write};
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    fn office_parts_keep_binary_names_without_retaining_bytes() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        writer
            .start_file("word/media/image1.png", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"image bytes").unwrap();
        let raw = writer.finish().unwrap().into_inner();
        let selected = office_parts(&raw, false).unwrap();
        assert_eq!(selected["word/document.xml"], b"<document/>");
        assert!(selected["word/media/image1.png"].is_empty());
        assert_eq!(
            office_parts(&raw, true).unwrap()["word/media/image1.png"],
            b"image bytes"
        );
    }

    #[test]
    fn skipped_binary_crc_is_checked_before_writeback() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/media/image1.png",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"image bytes").unwrap();
        let mut raw = writer.finish().unwrap().into_inner();
        let offset = raw
            .windows(b"image bytes".len())
            .position(|window| window == b"image bytes")
            .unwrap();
        raw[offset] ^= 1;
        assert!(office_parts(&raw, false).is_ok());
        assert!(validate_office_binary(&raw).is_err());
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_office_parts() {
        use std::time::Instant;

        let media = vec![0x5a; 32 * 1024 * 1024];
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        writer
            .start_file("word/media/image1.png", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&media).unwrap();
        let raw = writer.finish().unwrap().into_inner();
        for include_binary in [true, false] {
            let start = Instant::now();
            let parts = office_parts(&raw, include_binary).unwrap();
            let retained: usize = parts.values().map(Vec::len).sum();
            eprintln!(
                "office_parts include_binary={include_binary} elapsed_ms={} retained_bytes={retained}",
                start.elapsed().as_millis()
            );
        }
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_bound_controls() {
        use std::time::Instant;

        let body = (0..500)
            .map(|index| format!("<w:sdt><w:sdtContent><w:p><w:r><w:t>{index}</w:t></w:r></w:p></w:sdtContent></w:sdt>"))
            .collect::<String>();
        let xml = format!("<w:document xmlns:w=\"{WORD}\"><w:body>{body}</w:body></w:document>");
        let doc = Document::parse(&xml).unwrap();
        let edits: Vec<_> = doc
            .descendants()
            .filter(|node| node.has_tag_name((WORD, "t")))
            .map(|node| (node, "replacement".to_owned()))
            .collect();
        let controls: Vec<_> = doc
            .descendants()
            .filter(|node| node.has_tag_name((WORD, "sdt")))
            .collect();
        let start = Instant::now();
        let old: Vec<_> = controls
            .iter()
            .map(|control| {
                let mut indexed = BTreeMap::new();
                for (node, value) in &edits {
                    indexed.entry(node.range().start).or_insert(value.as_str());
                }
                word::control_text(*control, &indexed)
            })
            .collect();
        let old_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let mut indexed = BTreeMap::new();
        for (node, value) in &edits {
            indexed.entry(node.range().start).or_insert(value.as_str());
        }
        let new: Vec<_> = controls
            .iter()
            .map(|control| word::control_text(*control, &indexed))
            .collect();
        let new_ms = start.elapsed().as_millis();
        assert_eq!(old, new);
        eprintln!(
            "bound_controls old_ms={old_ms} new_ms={new_ms} controls={} edits={}",
            controls.len(),
            edits.len()
        );
    }

    const BODY: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
        <w:p><w:r><w:t>plain</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>DATE</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>2026/09/25</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p>
        <w:p><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>TOC</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r></w:p>
        <w:p><w:r><w:t>entry</w:t></w:r></w:p>
        <w:p><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>tail</w:t></w:r></w:p>
        </w:body></w:document>"#;

    /// Names, strings and comments holding `BI` are not images; a known data
    /// length ends an image even when its data holds ` EI `.
    #[test]
    fn inline_images_are_found_outside_names_strings_and_comments() {
        let raw = b"/BI 1 gs % BI\n(BI ID EI) Tj BI /W 4 /H 1 /BPC 8 /CS /G ID  EI  EI Q BI /F /AHx ID 00> EI";
        let text = |ranges: Vec<std::ops::Range<usize>>| -> Vec<String> {
            ranges
                .into_iter()
                .map(|r| String::from_utf8_lossy(&raw[r]).into_owned())
                .collect()
        };
        assert_eq!(
            text(inline_images(raw, &[Some(4), None]).unwrap()),
            [
                "BI /W 4 /H 1 /BPC 8 /CS /G ID  EI  EI",
                "BI /F /AHx ID 00> EI"
            ]
        );
        // Without the length, the first EI set off by whitespace ends the data.
        assert_eq!(
            text(inline_images(raw, &[None, None]).unwrap())[0],
            "BI /W 4 /H 1 /BPC 8 /CS /G ID  EI"
        );
    }

    #[test]
    fn word_field_results_cover_complex_simple_and_multi_paragraph_fields() {
        let xml = Document::parse(BODY).unwrap();
        let results = word_field_results(&xml);
        let flags: Vec<_> = xml
            .descendants()
            .filter(|n| n.has_tag_name((WORD, "t")))
            .map(|n| results.contains(&n.range().start))
            .collect();
        assert_eq!(flags, [false, true, false, true, true, false]);
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
    fn word_images_and_field_codes_are_importable_without_changing_body_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("illustrated.docx");
        word_package(
            &path,
            BODY,
            &[("word/media/image1.png", b"image bytes".to_vec())],
        );
        let source = Source::open(&path).unwrap();
        assert_eq!(
            source.word_images().unwrap()["image-001.png"],
            b"image bytes"
        );
        assert_eq!(
            source.word_field_codes().unwrap(),
            ["DATE", "PAGE", "TOC"]
                .map(|instruction| json!({"part":"word/document.xml","instruction":instruction}))
        );
        assert_eq!(source.sheets()[0]["cells"][0]["value"], "plain");
    }

    #[test]
    fn word_section_print_settings_are_recorded_with_source_part() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("print.docx");
        let body = format!(
            r#"<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>"#
        );
        word_package(&path, &body, &[]);
        let settings = Source::open(&path).unwrap().print_settings().unwrap();
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0]["part"], "word/document.xml");
        assert_eq!(settings[0]["component"], "sectPr");
        assert!(settings[0]["xml"].as_str().unwrap().contains("11906"));
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
                .patch(&rejected, &[], &[change(cell, before, after)])
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
            &[],
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

    fn compound_file(streams: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut file = cfb::CompoundFile::create(std::io::Cursor::new(vec![])).unwrap();
        for name in streams {
            file.create_stream(name)
                .unwrap()
                .write_all(b"data")
                .unwrap();
        }
        file.flush().unwrap();
        file.into_inner().into_inner()
    }

    #[test]
    fn ole_compound_files_are_reported_as_encrypted_or_binary() {
        let password = compound_file(&["/EncryptionInfo", "/EncryptedPackage"]);
        let error = office_package(&password).unwrap_err().to_string();
        assert!(error.contains("--password-stdin"), "{error}");
        let rights = compound_file(&["/EncryptedPackage"]);
        let error = office_package(&rights).unwrap_err().to_string();
        assert!(error.contains("sensitivity label"), "{error}");
        let binary = compound_file(&["/WordDocument"]);
        let error = office_package(&binary).unwrap_err().to_string();
        assert!(error.contains("binary Office document"), "{error}");
        office_package(b"PK").unwrap();
        let dir = tempfile::tempdir().unwrap();
        for extension in ["docx", "xlsm"] {
            let source = dir.path().join(format!("protected.{extension}"));
            fs::write(&source, &password).unwrap();
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
                &[],
                &[change("A2", "2026/09/25after")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("Word field result"), "{error}");
        let written = dir.path().join("written.docx");
        doc.patch(&written, &[], &[change("A1", "note")]).unwrap();
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
                &[],
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
            &[],
            &[json!({"sheet":"document","cell":"A1","before":"本文","after":"更新"})],
        )
        .unwrap();
        assert_eq!(
            written_part(&written, "word/document.xml"),
            encoded(&body.replace("<w:t>本文", r#"<w:t xml:space="preserve">更新"#))
        );
        assert_eq!(values(&open_word(&written), 0), ["更新"]);
    }

    #[test]
    fn word_line_breaks_and_tabs_are_written_as_run_elements() {
        let text_box = || r#"<w:txbxContent><w:p><w:r><w:t>注</w:t><w:br/><w:t>記</w:t></w:r></w:p></w:txbxContent>"#;
        let body = format!(
            r#"<w:document xmlns:w="{W}" xmlns:mc="{MARKUP_COMPATIBILITY}"><w:body><w:p><w:r><w:t>項目</w:t><w:tab/><w:t>値</w:t></w:r></w:p><w:p><w:r><w:t>一行目</w:t><w:br/><w:t>二行目</w:t></w:r></w:p><w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{}</w:drawing></mc:Choice><mc:Fallback><w:pict>{}</w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p><w:p><w:fldSimple w:instr="PAGE"><w:r><w:t>1</w:t><w:tab/><w:t>2</w:t></w:r></w:fldSimple></w:p></w:body></w:document>"#,
            text_box(),
            text_box()
        );
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("breaks.docx");
        word_package(&source, &body, &[]);
        let doc = open_word(&source);
        assert_eq!(
            values(&doc, 0),
            ["項目\t値", "一行目\n二行目", "注\n記", "1\t2"]
        );
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        // A tab or break in a field result is recalculated like its text.
        let error = doc
            .patch(
                &dir.path().join("field.docx"),
                &[],
                &[change("A4", "1\t2", "12")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("Word field result"), "{error}");
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[],
            &[
                change("A1", "項目\t値", "項目\n値\t（単位）"),
                change("A2", "一行目\n二行目", "一行目二行目"),
                change("A3", "注\n記", "注\t記"),
            ],
        )
        .unwrap();
        assert_eq!(
            values(&open_word(&written), 0),
            ["項目\n値\t（単位）", "一行目二行目", "注\t記", "1\t2"]
        );
        let xml = String::from_utf8(written_part(&written, "word/document.xml")).unwrap();
        assert!(
            xml.contains(r#"<w:t>項目</w:t><w:br/><w:t xml:space="preserve">値</w:t><w:tab/><w:t xml:space="preserve">（単位）</w:t>"#),
            "{xml}"
        );
        assert!(xml.contains("<w:t>一行目</w:t><w:t>二行目</w:t>"), "{xml}");
        // Both renderings of the text box change.
        assert_eq!(
            xml.matches("<w:t>注</w:t><w:tab/><w:t>記</w:t>").count(),
            2,
            "{xml}"
        );
    }

    /// A Word package whose title shows the core properties' title twice (a
    /// block control and an inline one split over two runs) and whose cover
    /// page date and abstract show a custom XML part.
    fn bound_package(path: &Path) {
        const DC: &str = "http://purl.org/dc/elements/1.1/";
        const CP: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
        const COVER: &str = "http://schemas.microsoft.com/office/2006/coverPageProps";
        let binding = |store: &str, prefixes: &str, xpath: &str, kind: &str| {
            format!(
                r#"<w:sdtPr><w:dataBinding w:prefixMappings="{prefixes}" w:xpath="{xpath}" w:storeItemID="{store}"/>{kind}</w:sdtPr>"#
            )
        };
        let title = binding(
            "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}",
            &format!("xmlns:ns0='{DC}' xmlns:ns1='{CP}'"),
            "/ns1:coreProperties[1]/ns0:title[1]",
            "<w:text/>",
        );
        let cover = |field: &str, kind: &str| {
            binding(
                "{55AF091B-3C7A-41E3-B477-F2FDAA23CFDA}",
                &format!("xmlns:ns0='{COVER}'"),
                &format!("/ns0:CoverPageProperties[1]/ns0:{field}[1]"),
                kind,
            )
        };
        let body = format!(
            r#"<w:document xmlns:w="{W}"><w:body><w:sdt>{title}<w:sdtContent><w:p><w:r><w:t>旧題</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>題名：</w:t></w:r><w:sdt>{title}<w:sdtContent><w:r><w:t>旧</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>題</w:t></w:r></w:sdtContent></w:sdt></w:p><w:sdt>{}<w:sdtContent><w:p><w:r><w:t>2026/9/1</w:t></w:r></w:p></w:sdtContent></w:sdt><w:sdt>{}<w:sdtContent><w:p><w:r><w:t>概要</w:t></w:r></w:p></w:sdtContent></w:sdt></w:body></w:document>"#,
            cover("PublishDate", "<w:date/>"),
            cover("Abstract", "<w:text/>")
        );
        package(
            path,
            &[
                (
                    "_rels/.rels",
                    format!(
                        r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="word/document.xml"/><Relationship Id="r2" Type="{PACKAGE_REL}/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#
                    )
                    .into_bytes(),
                ),
                ("word/document.xml", body.into_bytes()),
                (
                    "docProps/core.xml",
                    format!(r#"<cp:coreProperties xmlns:cp="{CP}" xmlns:dc="{DC}"><dc:title>旧題</dc:title><dc:creator/></cp:coreProperties>"#).into_bytes(),
                ),
                (
                    "customXml/item1.xml",
                    format!(r#"<CoverPageProperties xmlns="{COVER}"><PublishDate>2026-09-01</PublishDate><Abstract>概要</Abstract></CoverPageProperties>"#).into_bytes(),
                ),
                (
                    "customXml/_rels/item1.xml.rels",
                    format!(r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="p" Type="{REL}/customXmlProps" Target="itemProps1.xml"/></Relationships>"#).into_bytes(),
                ),
                (
                    "customXml/itemProps1.xml",
                    br#"<ds:datastoreItem ds:itemID="{55af091b-3c7a-41e3-b477-f2fdaa23cfda}" xmlns:ds="http://schemas.openxmlformats.org/officeDocument/2006/customXml"/>"#.to_vec(),
                ),
            ],
        );
    }

    #[test]
    fn edits_to_bound_content_controls_update_their_data_and_twins() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bound.docx");
        bound_package(&source);
        let doc = open_word(&source);
        assert_eq!(values(&doc, 0), ["旧題", "題名：旧題", "2026/9/1", "概要"]);
        let change = |cell: &str, before: &str, after: &str| json!({"sheet":"document","cell":cell,"before":before,"after":after});
        let written = dir.path().join("written.docx");
        doc.patch(
            &written,
            &[],
            &[
                change("A1", "旧題", "新題"),
                change("A4", "概要", "新しい概要"),
            ],
        )
        .unwrap();
        // The inline control bound to the same title shows it too.
        assert_eq!(
            values(&open_word(&written), 0),
            ["新題", "題名：新題", "2026/9/1", "新しい概要"]
        );
        let core = String::from_utf8(written_part(&written, "docProps/core.xml")).unwrap();
        assert!(
            core.contains("<dc:title>新題</dc:title><dc:creator/>"),
            "{core}"
        );
        let cover = String::from_utf8(written_part(&written, "customXml/item1.xml")).unwrap();
        assert!(cover.contains("<Abstract>新しい概要</Abstract>"), "{cover}");

        for (changes, message) in [
            (vec![change("A3", "2026/9/1", "2026/10/1")], "date, list"),
            (vec![change("A1", "旧題", "新\n題")], "single-line"),
            (
                vec![
                    change("A1", "旧題", "新題"),
                    change("A2", "題名：旧題", "題名：別題"),
                ],
                "edited to different text",
            ),
        ] {
            let error = doc
                .patch(&dir.path().join("rejected.docx"), &[], &changes)
                .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
        }
    }

    /// A one-slide presentation whose slide holds `slide` text and, with
    /// `notes`, a notes page with that text, its slide image and slide number.
    fn pptx_package(path: &Path, slide: &str, notes: Option<&str>) {
        let mut parts = vec![
            (
                "_rels/.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="r1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/presentation.xml",
                format!(
                    r#"<p:presentation xmlns:p="{PRESENTATION}" xmlns:r="{REL}"><p:sldIdLst><p:sldId id="256" r:id="s1"/></p:sldIdLst></p:presentation>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="s1" Type="{REL}/slide" Target="slides/slide1.xml"/></Relationships>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/slides/slide1.xml",
                format!(
                    "<p:sld xmlns:p=\"{PRESENTATION}\" xmlns:a=\"{DRAWING}\"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:rPr lang=\"ja-JP\" b=\"1\"/><a:t>{slide}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                )
                .into_bytes(),
            ),
        ];
        if let Some(notes) = notes {
            let placeholder = |kind: &str, body: &str| {
                format!(
                    r#"<p:sp><p:nvSpPr><p:cNvPr id="1" name="{kind}"/><p:cNvSpPr/><p:nvPr><p:ph type="{kind}"/></p:nvPr></p:nvSpPr>{body}</p:sp>"#
                )
            };
            parts.push((
                "ppt/slides/_rels/slide1.xml.rels",
                format!(
                    r#"<Relationships xmlns="{PACKAGE_REL}"><Relationship Id="n1" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#
                )
                .into_bytes(),
            ));
            parts.push((
                "ppt/notesSlides/notesSlide1.xml",
                format!(
                    r#"<p:notes xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld><p:spTree>{}{}{}</p:spTree></p:cSld></p:notes>"#,
                    placeholder("sldImg", ""),
                    placeholder("body", &format!("<p:txBody><a:p><a:r><a:t>{notes}</a:t></a:r></a:p></p:txBody>")),
                    placeholder("sldNum", r#"<p:txBody><a:p><a:fld id="{1}" type="slidenum"><a:t>1</a:t></a:fld></a:p></p:txBody>"#),
                )
                .into_bytes(),
            ));
        }
        package(path, &parts);
    }

    fn open_text(path: &Path) -> TextSource {
        let Source::Text(doc) = Source::open(path).unwrap() else {
            panic!("text source expected")
        };
        doc
    }

    #[test]
    fn powerpoint_paragraphs_take_tabs_and_line_breaks_in_the_run_format() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("tabs.pptx");
        pptx_package(&source, "項目\t値", None);
        let doc = open_text(&source);
        assert_eq!(values(&doc, 0), ["項目\t値"]);
        let written = dir.path().join("written.pptx");
        doc.patch(
            &written,
            &[],
            &[json!({"sheet":"slide-1","cell":"A1","before":"項目\t値","after":"項目\n\t値"})],
        )
        .unwrap();
        let doc = open_text(&written);
        assert_eq!(values(&doc, 0), ["項目\n\t値"]);
        let slide = String::from_utf8(written_part(&written, "ppt/slides/slide1.xml")).unwrap();
        let format = r#"<a:rPr lang="ja-JP" b="1"/>"#;
        assert!(
            slide.contains(&format!(
                "<a:r>{format}<a:t>項目</a:t></a:r><a:br>{format}</a:br><a:r>{format}<a:t>\t値</a:t></a:r>"
            )),
            "{slide}"
        );
        // Taking the line break out joins the lines again.
        let joined = dir.path().join("joined.pptx");
        doc.patch(
            &joined,
            &[],
            &[json!({"sheet":"slide-1","cell":"A1","before":"項目\n\t値","after":"項目、値"})],
        )
        .unwrap();
        assert_eq!(values(&open_text(&joined), 0), ["項目、値"]);
    }

    /// Notes pages follow the slides; the slide number they show is left out.
    #[test]
    fn powerpoint_notes_follow_the_slides_and_are_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("notes.pptx");
        pptx_package(&source, "表題", Some("話す内容"));
        let doc = open_text(&source);
        let names: Vec<_> = doc
            .sheets
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["slide-1", "notes-1"]);
        assert_eq!(doc.sheets[1]["part"], "ppt/notesSlides/notesSlide1.xml");
        assert_eq!(values(&doc, 1), ["話す内容"]);
        let written = dir.path().join("written.pptx");
        doc.patch(
            &written,
            &[], &[json!({"sheet":"notes-1","cell":"A1","before":"話す内容","after":"話す内容（改訂）"})],
        )
        .unwrap();
        let doc = open_text(&written);
        assert_eq!(values(&doc, 0), ["表題"]);
        assert_eq!(values(&doc, 1), ["話す内容（改訂）"]);
        let notes =
            String::from_utf8(written_part(&written, "ppt/notesSlides/notesSlide1.xml")).unwrap();
        assert!(notes.contains("<a:t>1</a:t></a:fld>"), "{notes}");
    }

    /// A two-slide presentation. Slide 1 has a notes page, a chart with its
    /// workbook and a comment; with `link`, slide 2 links to slide 1. Both
    /// slides are in one section, and custom shows list `shows` (slide numbers).
    fn deck_package(path: &Path, link: bool, shows: &[&[usize]]) {
        const P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
        const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
        let slide = |text: &str| {
            format!(
                r#"<p:sld xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}" xmlns:r="{REL}"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
            )
            .into_bytes()
        };
        let rels = |body: &str| {
            format!(r#"<Relationships xmlns="{PACKAGE_REL}">{body}</Relationships>"#).into_bytes()
        };
        let override_type = |part: &str, kind: &str| {
            format!(
                r#"<Override PartName="/{part}" ContentType="application/vnd.openxmlformats-officedocument.{kind}+xml"/>"#
            )
        };
        let shows: String = shows
            .iter()
            .enumerate()
            .map(|(i, slides)| {
                let slides: String = slides
                    .iter()
                    .map(|n| format!(r#"<p:sld r:id="rId{}"/>"#, n + 1))
                    .collect();
                format!(r#"<p:custShow name="show{i}" id="{i}"><p:sldLst>{slides}</p:sldLst></p:custShow>"#)
            })
            .collect();
        let parts = vec![
            (
                "[Content_Types].xml",
                format!(
                    r#"<Types xmlns="{TYPES}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="xlsx" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"/>{}{}{}{}{}</Types>"#,
                    override_type("ppt/presentation.xml", "presentationml.presentation.main"),
                    override_type("ppt/slides/slide1.xml", "presentationml.slide"),
                    override_type("ppt/slides/slide2.xml", "presentationml.slide"),
                    override_type("ppt/notesSlides/notesSlide1.xml", "presentationml.notesSlide"),
                    override_type("ppt/charts/chart1.xml", "drawingml.chart"),
                )
                .into_bytes(),
            ),
            (
                "_rels/.rels",
                rels(&format!(
                    r#"<Relationship Id="r1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="r2" Type="{REL}/extended-properties" Target="docProps/app.xml"/>"#
                )),
            ),
            (
                "docProps/app.xml",
                b"<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Slides>2</Slides><Notes>1</Notes></Properties>".to_vec(),
            ),
            (
                "ppt/presentation.xml",
                format!(
                    r#"<p:presentation xmlns:p="{PRESENTATION}" xmlns:r="{REL}"><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId3"/></p:sldIdLst><p:custShowLst>{shows}</p:custShowLst><p:extLst><p:ext uri="{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}"><p14:sectionLst xmlns:p14="{P14}"><p14:section name="All" id="{{1}}"><p14:sldIdLst><p14:sldId id="256"/><p14:sldId id="257"/></p14:sldIdLst></p14:section></p14:sectionLst></p:ext></p:extLst></p:presentation>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId2" Type="{REL}/slide" Target="slides/slide1.xml"/><Relationship Id="rId3" Type="{REL}/slide" Target="slides/slide2.xml"/>"#
                )),
            ),
            ("ppt/slides/slide1.xml", slide("表題")),
            (
                "ppt/slides/_rels/slide1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/notesSlide" Target="../notesSlides/notesSlide1.xml"/><Relationship Id="rId2" Type="{REL}/chart" Target="../charts/chart1.xml"/><Relationship Id="rId3" Type="{REL}/comments" Target="../comments/comment1.xml"/>"#
                )),
            ),
            (
                "ppt/notesSlides/notesSlide1.xml",
                format!(
                    r#"<p:notes xmlns:p="{PRESENTATION}" xmlns:a="{DRAWING}"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="1" name="body"/><p:cNvSpPr/><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>話す内容</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:notes>"#
                )
                .into_bytes(),
            ),
            (
                "ppt/notesSlides/_rels/notesSlide1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/slide" Target="../slides/slide1.xml"/>"#
                )),
            ),
            ("ppt/charts/chart1.xml", b"<chartSpace/>".to_vec()),
            (
                "ppt/charts/_rels/chart1.xml.rels",
                rels(&format!(
                    r#"<Relationship Id="rId1" Type="{REL}/package" Target="../embeddings/Book.xlsx"/>"#
                )),
            ),
            ("ppt/embeddings/Book.xlsx", b"workbook".to_vec()),
            ("ppt/comments/comment1.xml", b"<cmLst/>".to_vec()),
            ("ppt/slides/slide2.xml", slide("二枚目")),
            (
                "ppt/slides/_rels/slide2.xml.rels",
                rels(&if link {
                    format!(r#"<Relationship Id="rId1" Type="{REL}/slide" Target="slide1.xml"/>"#)
                } else {
                    String::new()
                }),
            ),
        ];
        package(path, &parts);
    }

    fn slide_operation(kind: &str, fields: Value) -> Value {
        let mut operation = json!({"id":"op","kind":kind,"reason":"test"});
        for (key, value) in fields.as_object().unwrap() {
            operation[key] = value.clone();
        }
        operation
    }

    #[test]
    fn powerpoint_slides_are_copied_with_their_own_parts_and_deleted_with_theirs() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deck.pptx");
        deck_package(&source, false, &[&[1, 2]]);
        let doc = open_text(&source);
        let names: Vec<_> = doc
            .sheets
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["slide-1", "slide-2", "notes-1"]);
        let operations = [
            json!({"id":"copy","kind":"insert_slide","from":"slide-1","after":"slide-2","reason":"copy"}),
            json!({"id":"drop","kind":"delete_slide","slide":"slide-2","reason":"drop"}),
        ];
        let written = dir.path().join("written.pptx");
        let report = doc
            .patch(
                &written,
                &operations,
                &[
                    json!({"sheet":"copy","cell":"A1","before":"表題","after":"複製"}),
                    json!({"sheet":"notes-copy","cell":"A1","before":"話す内容","after":"複製のノート"}),
                ],
            )
            .unwrap();
        assert_eq!(report["slide_order"], json!(["slide-1", "copy"]));
        let result = open_text(&written);
        let pages: Vec<_> = (0..result.sheets.len())
            .map(|i| values(&result, i))
            .collect();
        assert_eq!(pages, [["表題"], ["複製"], ["話す内容"], ["複製のノート"]]);

        let part = |name: &str| String::from_utf8(written_part(&written, name)).unwrap();
        let mut zip = zip::ZipArchive::new(fs::File::open(&written).unwrap()).unwrap();
        assert!(zip.by_name("ppt/slides/slide2.xml").is_err());
        assert!(zip.by_name("ppt/slides/_rels/slide2.xml.rels").is_err());
        // The copy has its own chart and workbook, and no comments.
        let copy_rels = part("ppt/slides/_rels/slide3.xml.rels");
        assert!(
            copy_rels.contains(r#"Target="../charts/chart2.xml""#),
            "{copy_rels}"
        );
        assert!(
            copy_rels.contains(r#"Target="../notesSlides/notesSlide2.xml""#),
            "{copy_rels}"
        );
        assert!(!copy_rels.contains("comments"), "{copy_rels}");
        assert!(
            part("ppt/charts/_rels/chart2.xml.rels")
                .contains(r#"Target="../embeddings/Book1.xlsx""#)
        );
        assert_eq!(
            written_part(&written, "ppt/embeddings/Book1.xlsx"),
            b"workbook"
        );
        assert!(
            part("ppt/notesSlides/_rels/notesSlide2.xml.rels")
                .contains(r#"Target="../slides/slide3.xml""#)
        );
        assert!(part("ppt/slides/_rels/slide1.xml.rels").contains("comment1.xml"));

        let presentation = part("ppt/presentation.xml");
        assert!(presentation.contains(r#"<p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="258" r:id="rId1"/></p:sldIdLst>"#), "{presentation}");
        assert!(
            presentation.contains(r#"<p:sldLst><p:sld r:id="rId2"/></p:sldLst>"#),
            "{presentation}"
        );
        assert!(
            presentation.contains(
                r#"<p14:sldIdLst><p14:sldId id="256"/><p14:sldId id="258"/></p14:sldIdLst>"#
            ),
            "{presentation}"
        );
        let types = part("[Content_Types].xml");
        for present in [
            "/ppt/slides/slide3.xml",
            "/ppt/notesSlides/notesSlide2.xml",
            "/ppt/charts/chart2.xml",
        ] {
            assert!(types.contains(present), "{present}: {types}");
        }
        assert!(!types.contains("/ppt/slides/slide2.xml"), "{types}");
        assert!(part("docProps/app.xml").contains("<Slides>2</Slides><Notes>2</Notes>"));
    }

    #[test]
    fn a_linked_slide_or_the_last_of_a_custom_show_is_not_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let delete = |source: &Path, slide: &str| {
            open_text(source)
                .patch(
                    &dir.path().join(format!("out-{slide}.pptx")),
                    &[slide_operation("delete_slide", json!({"slide":slide}))],
                    &[],
                )
                .map(|_| String::new())
                .unwrap_or_else(|error| format!("{error:#}"))
        };
        let linked = dir.path().join("linked.pptx");
        deck_package(&linked, true, &[]);
        let error = delete(&linked, "slide-1");
        assert!(error.contains("linked from slide-2"), "{error}");
        let shown = dir.path().join("shown.pptx");
        deck_package(&shown, false, &[&[2]]);
        let error = delete(&shown, "slide-2");
        assert!(
            error.contains("custom show show0 would have no slides left"),
            "{error}"
        );
        assert!(!dir.path().join("out-slide-2.pptx").exists());
    }

    #[test]
    fn powerpoint_slides_are_moved_with_their_sections_and_hidden_or_shown() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("deck.pptx");
        deck_package(&source, false, &[&[1, 2]]);
        let doc = open_text(&source);
        let operations = [
            json!({"id":"copy","kind":"insert_slide","from":"slide-1","after":"slide-2","reason":"copy"}),
            json!({"id":"first","kind":"move_slide","slide":"slide-2","before":"slide-1","reason":"order"}),
            json!({"id":"last","kind":"move_slide","slide":"slide-1","after":"copy","reason":"order"}),
            json!({"id":"hide","kind":"set_slide_visibility","slide":"copy","hidden":true,"reason":"draft"}),
        ];
        let written = dir.path().join("written.pptx");
        let report = doc.patch(&written, &operations, &[]).unwrap();
        assert_eq!(report["slide_order"], json!(["slide-2", "copy", "slide-1"]));
        let result = open_text(&written);
        assert_eq!(result.sheets.len(), 5, "three slides and two notes pages");
        let states: Vec<_> = result.sheets[..3]
            .iter()
            .map(|s| s["state"].clone())
            .collect();
        assert_eq!(
            states,
            [json!("visible"), json!("hidden"), json!("visible")]
        );
        let texts: Vec<_> = (0..3).map(|i| values(&result, i)).collect();
        assert_eq!(texts, [vec!["二枚目"], vec!["表題"], vec!["表題"]]);
        // The section lists the slides in their new order, as the show does.
        let mut zip = zip::ZipArchive::new(fs::File::open(&written).unwrap()).unwrap();
        let mut presentation = String::new();
        zip.by_name("ppt/presentation.xml")
            .unwrap()
            .read_to_string(&mut presentation)
            .unwrap();
        let ids = |list: &str| -> Vec<String> {
            let start = presentation.find(list).unwrap();
            let end = start
                + presentation[start..]
                    .find(&list.replace('<', "</"))
                    .unwrap();
            regex::Regex::new(r#"id="(\d+)""#)
                .unwrap()
                .captures_iter(&presentation[start..end])
                .map(|c| c[1].to_owned())
                .collect()
        };
        assert_eq!(ids("<p:sldIdLst>"), ids("<p14:sldIdLst>"));
        assert_eq!(ids("<p:sldIdLst>")[0], "257");
        // Showing the hidden slide again removes the mark.
        let shown = dir.path().join("shown.pptx");
        let hidden = open_text(&written);
        hidden
            .patch(
                &shown,
                &[json!({"id":"show","kind":"set_slide_visibility","slide":"slide-2","hidden":false,"reason":"final"})],
                &[],
            )
            .unwrap();
        assert!(
            open_text(&shown)
                .sheets
                .iter()
                .all(|s| s["state"] == "visible")
        );
        let error = |operations: &[Value]| {
            format!(
                "{:#}",
                parse_slide_operations(operations, &doc.sheets).unwrap_err()
            )
        };
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-1","after":"slide-1","reason":"r"})])
                .contains("next to itself")
        );
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-9","after":"slide-1","reason":"r"})])
                .contains("slide-9 is not a slide here")
        );
        assert!(
            error(&[json!({"id":"m","kind":"move_slide","slide":"slide-1","reason":"r"})])
                .contains("one of after and before")
        );
    }

    #[test]
    fn slide_operations_name_existing_slides_and_new_pages() {
        let sheets = [
            json!({"name":"slide-1","part":"a"}),
            json!({"name":"slide-2","part":"b"}),
            json!({"name":"notes-1","part":"c"}),
        ];
        let error = |operations: &[Value]| {
            format!(
                "{:#}",
                parse_slide_operations(operations, &sheets).unwrap_err()
            )
        };
        let insert = |id: &str, from: &str| json!({"id":id,"kind":"insert_slide","from":from,"after":"slide-1","reason":"r"});
        assert!(error(&[insert("slide-2", "slide-1")]).contains("choose another ID"));
        assert!(error(&[insert("1", "slide-1")]).contains("is a number"));
        assert!(error(&[insert("x", "slide-9")]).contains("slide-9 is not a slide here"));
        let delete_copy = json!({"id":"d","kind":"delete_slide","slide":"x","reason":"r"});
        assert!(error(&[insert("x", "slide-1"), delete_copy]).contains("remove that operation"));
        let delete = |id: &str, slide: &str| json!({"id":id,"kind":"delete_slide","slide":slide,"reason":"r"});
        assert!(error(&[delete("a", "slide-1"), delete("b", "slide-2")]).contains("last slide"));
        assert!(
            error(&[delete("a", "slide-1"), delete("b", "slide-1")]).contains("not a slide here")
        );

        let operations =
            parse_slide_operations(&[insert("x", "slide-1"), delete("d", "slide-1")], &sheets)
                .unwrap();
        let view = slide_view(&sheets, &[], &operations).unwrap();
        let pages: Vec<_> = view
            .iter()
            .map(|s| (s["name"].as_str().unwrap(), s["page"].as_str().unwrap()))
            .collect();
        assert_eq!(
            pages,
            [
                ("slide-2", "sheet-2"),
                ("x", "sheet-x"),
                ("notes-x", "sheet-notes-x")
            ]
        );
        assert_eq!(view[2]["copy_of"], "notes-1");
    }

    #[test]
    fn root_level_presentation_relationships_are_found() {
        let mut parts = BTreeMap::new();
        parts.insert(
            "_rels/.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"r0\" Type=\"{REL}/officeDocument\" Target=\"presentation.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert(
            "presentation.xml".into(),
            format!("<p:presentation xmlns:p=\"{PRESENTATION}\" xmlns:r=\"{REL}\"><p:sldId r:id=\"r1\"/></p:presentation>").into_bytes(),
        );
        parts.insert(
            "_rels/presentation.xml.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"r1\" Type=\"{REL}/slide\" Target=\"slide.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert("slide.xml".into(), Vec::new());
        parts.insert(
            "_rels/slide.xml.rels".into(),
            format!("<Relationships xmlns=\"{PACKAGE_REL}\"><Relationship Id=\"n1\" Type=\"{REL}/notesSlide\" Target=\"notes.xml\"/></Relationships>").into_bytes(),
        );
        parts.insert("notes.xml".into(), Vec::new());
        assert_eq!(
            office_containers(&parts, "pptx").unwrap(),
            vec![
                ("slide-1".into(), "slide.xml".into()),
                ("notes-1".into(), "notes.xml".into())
            ]
        );
    }
}
