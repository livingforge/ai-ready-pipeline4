//! HTML text laid out on a grid as Word's is (see [`super::word`]): the
//! document title and each block outside tables take one row in column A,
//! and a table keeps its rows and grid columns, reports cells spanning rows
//! or columns as merges and its range as an explicit table. A block is the
//! text a block element (paragraph, heading, list item, table cell...) shows
//! between the blocks it holds, with white space collapsed as browsers show
//! it. Pieces map that text back to the places in the original it comes from,
//! so that an edit replaces only the characters it changes and every other
//! byte of the original stays as it is.
use super::html_parse::{self, Decoded, Kind, Tree};
use super::word::{InsertedText, Placed};
use super::*;
use crate::excel::{StructuralOperation, column_name};
use std::ops::Range;

/// Elements that start and end a block of their own.
const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "html",
    "legend",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "plaintext",
    "pre",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
    "xmp",
];
/// Elements whose content is not shown as text of the document.
const HIDDEN: &[&str] = &[
    "head", "title", "script", "style", "template", "noscript", "svg", "math", "iframe", "object",
    "embed", "canvas", "audio", "video", "select", "textarea", "input", "map", "noembed",
    "noframes", "datalist", "meta", "link", "base",
];
/// Block elements whose text, when it is all they hold, is a row that row
/// operations copy and delete.
const ROW_ELEMENTS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "legend",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "p",
    "pre",
    "search",
    "section",
    "summary",
];
const LISTS: &[&str] = &["ul", "ol", "dl", "menu", "dir"];

/// A run of a block's text and the places in the original it comes from.
#[derive(Clone, Debug)]
struct Piece {
    /// The bytes of the block's text.
    text: Range<usize>,
    /// The places of those characters in the original: the characters
    /// themselves when `literal`, else the character reference, white space
    /// or line break they stand for. A space HTML collapses from white space
    /// in several text nodes has several. None for the line break joining
    /// two paragraphs of a table cell, which no edit can change.
    sources: Vec<Range<usize>>,
    literal: bool,
    /// Inside `pre`, which shows white space as written.
    pre: bool,
}

pub(super) struct Block {
    pub address: String,
    pub text: String,
    pieces: Vec<Piece>,
    /// Whether an edit can write line breaks, as `<br>`; the title cannot.
    breaks: bool,
    bold: bool,
    /// The size browsers give the text by default, in points.
    size: u32,
    in_table: bool,
    /// What the block is (see [`Inherited::kind`]) and the headings it is
    /// under, its own first when it is one.
    kind: &'static str,
    headings: Vec<String>,
    /// How browsers set the block off beyond its font (see [`BlockShape`]).
    indent: usize,
    marker: Option<String>,
}

/// How browsers set a block off from the text around it beyond its font,
/// for a page that shows the blocks as a browser does.
pub struct BlockShape {
    /// What the block is, as its position names it.
    pub kind: &'static str,
    /// How many lists, definitions, quotes and figures hold it, each of
    /// which a browser indents.
    pub indent: usize,
    /// The bullet or number of the list item it starts.
    pub marker: Option<String>,
}

enum RowKind {
    /// A block element whose text the row is.
    Element(usize),
    /// A table row, with its cells and their grid columns; `spanned` when a
    /// cell spanning rows starts in or crosses it.
    TableRow {
        row: usize,
        table: usize,
        cells: Vec<(usize, usize)>,
        spanned: bool,
    },
    Caption(usize),
    /// Text that is not the whole of an element, which row operations
    /// cannot copy or delete, and why.
    Fixed(&'static str),
}

struct Row {
    kind: RowKind,
    /// A node of the row, to place new rows next to it.
    anchor: usize,
    /// The blocks of the row in column order.
    blocks: Vec<usize>,
}

pub(super) struct Layout {
    pub blocks: Vec<Block>,
    merges: Vec<String>,
    tables: Vec<Value>,
    rows: BTreeMap<usize, Row>,
    /// The `<a href>` of the blocks: the cell, `url` and the `text` shown.
    links: Vec<Value>,
    /// The names links point to (`id`, and `name` of `a`) and the cells at them.
    anchors: Vec<Value>,
    /// The `<img>` of the page: the cell of its block, `src`, `alt` and,
    /// once [`Layout::assets`] names it, the `asset` of a picture it reads.
    images: Vec<Value>,
}

/// The extensions of pictures a page shows that are kept as assets.
const PICTURES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "svg", "ico",
];

/// The pictures of a page kept as assets: the bytes of those it holds as
/// data URLs and the files of those it links, by asset name.
#[derive(Default)]
pub(super) struct Pictures {
    pub embedded: BTreeMap<String, Vec<u8>>,
    pub files: BTreeMap<String, std::path::PathBuf>,
}

/// The bytes of a base64 `data:image/...` URL.
fn data_picture(src: &str) -> Option<(Vec<u8>, String)> {
    use base64::Engine;
    let rest = src.trim().strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    let media = header.strip_suffix(";base64")?;
    let kind = media.strip_prefix("image/")?.split(';').next()?;
    let extension = match kind {
        "jpeg" => "jpg",
        "svg+xml" => "svg",
        "x-icon" | "vnd.microsoft.icon" => "ico",
        kind if PICTURES.contains(&kind) => kind,
        _ => "bin",
    };
    let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()?;
    Some((bytes, extension.to_owned()))
}

impl Layout {
    /// How browsers set off each block beyond its font, by its address.
    pub(super) fn shapes(self) -> BTreeMap<String, BlockShape> {
        self.blocks
            .into_iter()
            .map(|block| {
                let shape = BlockShape {
                    kind: block.kind,
                    indent: block.indent,
                    marker: block.marker,
                };
                (block.address, shape)
            })
            .collect()
    }

    /// Names the pictures of the page at `page` as assets, in the order the
    /// page first shows them: those it holds as data URLs, and the picture
    /// files its relative `src` names that lie in one of `folders`
    /// (canonical paths). Pictures on other sites are not read.
    pub(super) fn assets(&mut self, page: &Path, folders: &[std::path::PathBuf]) -> Pictures {
        let mut pictures = Pictures::default();
        // The asset of each picture already named, by file or by its bytes.
        let mut named: BTreeMap<String, String> = BTreeMap::new();
        let directory = page.parent().unwrap_or(Path::new(""));
        for image in &mut self.images {
            let src = image["src"].as_str().unwrap_or("").to_owned();
            let next = format!("image-{:03}", named.len() + 1);
            let asset = if let Some((bytes, extension)) = data_picture(&src) {
                let key = format!("data:{}", crate::data::hash(&bytes));
                named.get(&key).cloned().unwrap_or_else(|| {
                    let name = format!("{next}.{extension}");
                    pictures.embedded.insert(name.clone(), bytes);
                    named.insert(key, name.clone());
                    name
                })
            } else {
                let path = src.split(['?', '#']).next().unwrap_or("");
                let scheme = path.split_once(':').is_some_and(|(name, _)| {
                    name.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                });
                if path.is_empty() || scheme || path.starts_with('/') || path.starts_with('\\') {
                    continue;
                }
                let file = directory.join(super::percent_decode_lenient(path));
                let extension = file
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .filter(|e| PICTURES.contains(&e.as_str()));
                // The path as it resolves past `..` and links, so that it
                // cannot leave the folders by them.
                let canonical = crate::data::canonicalize(&file).ok().filter(|file| {
                    file.is_file() && folders.iter().any(|folder| file.starts_with(folder))
                });
                let (Some(extension), Some(canonical)) = (extension, canonical) else {
                    continue;
                };
                let key = canonical.to_string_lossy().to_lowercase();
                named.get(&key).cloned().unwrap_or_else(|| {
                    let name = format!("{next}.{extension}");
                    pictures.files.insert(name.clone(), canonical);
                    named.insert(key, name.clone());
                    name
                })
            };
            image["asset"] = json!(asset);
        }
        pictures
    }
}

/// Whether element `node` shows no text of the document.
fn hidden(tree: &Tree, node: usize) -> bool {
    tree.element(node).is_some_and(|e| {
        e.foreign
            || HIDDEN.contains(&&*e.name)
            // A tab panel not chosen is hidden until its tab is, as the
            // code of each language is; its text is the page's all the same.
            || (e.attribute("hidden").is_some() && role(e).as_deref() != Some("tabpanel"))
            || e.name == "nav"
            || role(e).is_some_and(|role| CHROME.contains(&role.as_str()))
    })
}

/// The landmark roles of a site's frame around the body text: its
/// navigation, banner, footer and search.
const CHROME: &[&str] = &["navigation", "banner", "contentinfo", "search"];

/// The role an element names, in lower case.
fn role(element: &html_parse::Element) -> Option<String> {
    element
        .attribute("role")
        .and_then(|role| role.split_ascii_whitespace().next())
        .map(str::to_ascii_lowercase)
}

/// The first `<main>` or element of role `main` of `tree` not `excluded`
/// (hidden), which holds the body text when a page marks it.
fn main_of(tree: &Tree, excluded: &[bool]) -> Option<usize> {
    (0..tree.nodes.len()).find(|node| {
        !excluded[*node]
            && tree.element(*node).is_some_and(|e| {
                !e.foreign && (e.name == "main" || role(e).as_deref() == Some("main"))
            })
    })
}

impl Layout {
    /// Finds the links and pictures of the elements of `tree` that are not
    /// `excluded` (hidden, or outside the main content it marks), and the
    /// anchors of those not `inert`: a link to the site's navigation or a
    /// hidden element finds it too, at the block that follows it.
    fn link(&mut self, tree: &Tree, excluded: &[bool], inert: &[bool]) {
        // The blocks by where their text starts and ends in the original.
        let mut spans: Vec<(usize, usize, usize)> = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let sources = || block.pieces.iter().flat_map(|p| &p.sources);
                Some((
                    sources().map(|s| s.start).min()?,
                    sources().map(|s| s.end).max()?,
                    index,
                ))
            })
            .collect();
        spans.sort_unstable();
        // The block holding `at`, else the first after it.
        let block_at = |at: usize| {
            let next = spans.partition_point(|(start, _, _)| *start <= at);
            match next.checked_sub(1).map(|k| spans[k]) {
                Some((_, end, index)) if at < end => Some(index),
                _ => spans.get(next).map(|(_, _, index)| *index),
            }
        };
        let last = spans.last().map(|(_, _, index)| *index);
        let mut names = BTreeSet::new();
        for (node, excluded) in excluded.iter().enumerate() {
            let Some(element) = tree.element(node) else {
                continue;
            };
            let range = tree.nodes[node].range.clone();
            let name = element
                .attribute("id")
                .or_else(|| {
                    (element.name == "a")
                        .then(|| element.attribute("name"))
                        .flatten()
                })
                .filter(|name| !name.is_empty() && !inert[node]);
            // An element after the last block, as a footer, is at the last.
            if let Some(name) = name
                && let Some(index) = block_at(range.start).or(last)
                && names.insert(name.to_owned())
            {
                self.anchors
                    .push(json!({"name":name,"range":self.blocks[index].address}));
            }
            if *excluded {
                continue;
            }
            let linking = element.name == "a" && element.attribute("href").is_some();
            if element.name != "img" && !linking {
                continue;
            }
            // The nearest block of the block element holding the element.
            let container = tree
                .ancestors(node)
                .find(|a| BLOCK.contains(&tree.name(*a)))
                .map_or(0..usize::MAX, |a| tree.nodes[a].range.clone());
            let next = spans.partition_point(|(start, _, _)| *start <= range.start);
            let around = next
                .checked_sub(1)
                .map(|k| spans[k])
                .filter(|(start, _, _)| container.contains(start))
                .or_else(|| {
                    spans
                        .get(next)
                        .copied()
                        .filter(|(start, _, _)| container.contains(start))
                });
            if element.name == "img" {
                // A picture by itself is the next block's, or the last one's.
                let index = around
                    .map(|(_, _, index)| index)
                    .or_else(|| block_at(range.start))
                    .or(last);
                if let (Some(index), Some(src)) = (index, element.attribute("src")) {
                    let mut image = json!({"address":self.blocks[index].address,"src":src});
                    if let Some(alt) = element.attribute("alt") {
                        image["alt"] = json!(alt);
                    }
                    self.images.push(image);
                }
                continue;
            }
            let Some(url) = element.attribute("href").filter(|_| element.name == "a") else {
                continue;
            };
            // The block of the link's text, else, for a link showing none
            // (a picture's), the nearest block of the block element holding it.
            let Some(index) = block_at(element.start_tag.end)
                .filter(|index| {
                    self.blocks[*index]
                        .pieces
                        .iter()
                        .any(|p| p.sources.first().is_some_and(|s| range.contains(&s.start)))
                })
                .or(around.map(|(_, _, index)| index))
            else {
                continue;
            };
            let block = &self.blocks[index];
            // The characters of the block that the link shows.
            let shown: Vec<&Range<usize>> = block
                .pieces
                .iter()
                .filter(|p| p.sources.first().is_some_and(|s| range.contains(&s.start)))
                .map(|p| &p.text)
                .collect();
            let mut link = json!({"range":block.address,"url":url});
            if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
                link["text"] = json!(block.text[first.start..last.end]);
            }
            self.links.push(link);
        }
    }
}

/// What the text being read inherits from its elements.
#[derive(Clone, Copy)]
struct Inherited {
    pre: bool,
    /// Where the content of the `pre` the text is in starts, whose first line
    /// break HTML drops.
    pre_start: Option<usize>,
    bold: bool,
    size: u32,
    /// What the nearest block element makes the text: a heading, a list
    /// item, code... as the position of a block names it.
    kind: &'static str,
    /// The level of the heading the text is in.
    level: Option<usize>,
    /// How many indenting elements hold the text (see [`BlockShape::indent`]).
    indent: usize,
}

/// The text of a block being read.
#[derive(Default)]
struct Run {
    text: String,
    pieces: Vec<Piece>,
    /// A space collapsed from the white space read since the last character.
    pending: Option<Piece>,
    /// Whether the next white space starts a line, which HTML drops.
    line_start: bool,
    /// Whether the last piece is a line break, which white space dropped at
    /// the start of the next line goes with.
    after_break: bool,
    /// Whether a block element inside `pre` started or ended since the last
    /// character, so that the next starts a line.
    block_break: bool,
    bold: bool,
    size: Option<u32>,
    anchor: Option<usize>,
    /// The kind and heading level of the first character.
    kind: Option<(&'static str, Option<usize>)>,
    /// The indent of the first character.
    indent: Option<usize>,
}

impl Run {
    fn new() -> Self {
        Self {
            line_start: true,
            bold: true,
            ..Self::default()
        }
    }

    fn shown(&mut self, node: usize, context: &Inherited) {
        self.anchor.get_or_insert(node);
        self.size.get_or_insert(context.size);
        self.kind.get_or_insert((context.kind, context.level));
        self.indent.get_or_insert(context.indent);
        self.bold &= context.bold;
        self.line_start = false;
        self.after_break = false;
    }

    /// Adds a character, or the characters a reference stands for, read from
    /// `source` of text node `node`.
    fn unit(
        &mut self,
        node: usize,
        chars: &str,
        source: Range<usize>,
        literal: bool,
        context: &Inherited,
    ) {
        let white = !context.pre && matches!(chars, " " | "\t" | "\n" | "\x0c");
        if white {
            if self.line_start {
                if self.after_break
                    && let Some(last) = self.pieces.last_mut()
                {
                    extend(&mut last.sources, source);
                }
                return;
            }
            match &mut self.pending {
                Some(space) => {
                    extend(&mut space.sources, source);
                    space.literal = false;
                }
                // One space as written is the space shown, part of the text
                // around it: a piece for each space between words took most
                // of the memory of a page's prose.
                None => {
                    self.pending = Some(Piece {
                        text: 0..0,
                        sources: vec![source],
                        literal: literal && chars == " ",
                        pre: false,
                    });
                }
            }
            return;
        }
        self.open_line();
        if let Some(space) = self.pending.take() {
            let start = self.text.len();
            self.text.push(' ');
            self.push(Piece {
                text: start..start + 1,
                ..space
            });
        }
        self.shown(node, context);
        let start = self.text.len();
        self.text.push_str(chars);
        self.push(Piece {
            text: start..self.text.len(),
            sources: vec![source],
            literal,
            pre: context.pre,
        });
    }

    /// Adds `piece`, joining a literal one to the literal piece before it
    /// when it follows it in the text and in the original.
    fn push(&mut self, piece: Piece) {
        if piece.literal
            && let Some(last) = self.pieces.last_mut()
            && last.literal
            && last.pre == piece.pre
            && last.text.end == piece.text.start
            && last.sources[0].end == piece.sources[0].start
        {
            last.text.end = piece.text.end;
            last.sources[0].end = piece.sources[0].end;
            return;
        }
        self.pieces.push(piece);
    }

    /// Adds the line break of `<br>` `node` at `source`; the spaces before it
    /// and after it are dropped with it.
    fn line_break(&mut self, node: usize, source: Range<usize>, context: &Inherited) {
        self.open_line();
        let mut sources = self.pending.take().map_or(vec![], |space| space.sources);
        extend(&mut sources, source);
        self.shown(node, context);
        let start = self.text.len();
        self.text.push('\n');
        self.pieces.push(Piece {
            text: start..start + 1,
            sources,
            literal: false,
            pre: context.pre,
        });
        self.line_start = !context.pre;
        self.after_break = true;
    }

    /// Starts a line after a block element inside `pre`, unless one has
    /// started. The line break has no place in the original, as the one
    /// joining the paragraphs of a table cell.
    fn open_line(&mut self) {
        if std::mem::take(&mut self.block_break)
            && !self.text.is_empty()
            && !self.text.ends_with('\n')
        {
            let start = self.text.len();
            self.text.push('\n');
            self.pieces.push(Piece {
                text: start..start + 1,
                sources: vec![],
                literal: false,
                pre: true,
            });
        }
    }
}

fn extend(sources: &mut Vec<Range<usize>>, source: Range<usize>) {
    match sources.last_mut() {
        Some(last) if last.end == source.start => last.end = source.end,
        _ => sources.push(source),
    }
}

/// Reads the text of `node` into `run`.
fn read_text(
    run: &mut Run,
    text: &str,
    node: usize,
    range: Range<usize>,
    references: bool,
    context: &Inherited,
) {
    let mut at = range.start;
    // HTML drops a line break that starts the content of `pre`.
    if context.pre_start == Some(at) {
        if text[at..range.end].starts_with("\r\n") {
            at += 2;
        } else if text[at..range.end].starts_with(['\n', '\r']) {
            at += 1;
        }
    }
    while at < range.end {
        let c = text[at..].chars().next().unwrap();
        match c {
            '&' if references => {
                if let Some((chars, end)) = html_parse::reference(text, at, false)
                    && end <= range.end
                {
                    run.unit(node, &chars, at..end, false, context);
                    at = end;
                    continue;
                }
                run.unit(node, "&", at..at + 1, true, context);
                at += 1;
            }
            // HTML reads CR LF and a lone CR as LF.
            '\r' => {
                let end = if text[at + 1..range.end].starts_with('\n') {
                    at + 2
                } else {
                    at + 1
                };
                run.unit(node, "\n", at..end, false, context);
                at = end;
            }
            c => {
                let end = at + c.len_utf8();
                run.unit(node, &text[at..end], at..end, true, context);
                at = end;
            }
        }
    }
}

struct Walker<'t> {
    tree: &'t Tree,
    text: &'t str,
    layout: Layout,
    run: Run,
    /// The next row of the sheet.
    row: usize,
    /// The blocks of the table cell being read.
    cell: Option<Vec<Block>>,
    /// The blocks made so far, in cells too.
    made: usize,
    /// The block elements and tables entered so far.
    entered: usize,
    /// The row of the block made last outside tables.
    last_row: Option<usize>,
    /// The headings of the sections being read, with their levels.
    headings: Vec<(usize, String)>,
    /// Whether each node holds a table (see [`Walker::frame`]).
    holds_table: Vec<bool>,
    /// The marker of the list item being read, for the first block it makes.
    marker: Option<String>,
}

/// The bullet or number browsers show before list item `node`: in a
/// numbered list the number its `start` and the `value` of its items count
/// to, in decimal, else a disc, circle or square by how many lists hold it.
fn marker(tree: &Tree, node: usize) -> String {
    let number = |node: usize, name: &str| {
        tree.element(node)
            .and_then(|e| e.attribute(name))
            .and_then(|value| value.trim().parse::<i64>().ok())
    };
    if let Some(list) = tree.parent(node).filter(|list| tree.name(*list) == "ol") {
        let mut count = number(list, "start").unwrap_or(1);
        for item in tree.children(list).filter(|item| tree.name(*item) == "li") {
            count = number(item, "value").unwrap_or(count);
            if item == node {
                return format!("{count}.");
            }
            count += 1;
        }
    }
    let lists = tree
        .ancestors(node)
        .filter(|a| matches!(tree.name(*a), "ul" | "ol" | "menu" | "dir"))
        .count();
    match lists {
        0 | 1 => "•",
        2 => "◦",
        _ => "▪",
    }
    .to_owned()
}

fn heading_size(name: &str) -> Option<u32> {
    Some(match name {
        "h1" => 24,
        "h2" => 18,
        "h3" => 14,
        "h4" => 12,
        "h5" => 10,
        "h6" => 8,
        _ => return None,
    })
}

impl Walker<'_> {
    fn hidden(&self, node: usize) -> bool {
        hidden(self.tree, node)
    }

    /// The context of the content of element `node` in `context`.
    fn inside(&self, node: usize, mut context: Inherited) -> Inherited {
        let element = self.tree.element(node).unwrap();
        let name: &str = &element.name;
        if matches!(name, "pre" | "listing" | "plaintext" | "xmp") {
            context.pre = true;
            context.pre_start = Some(element.start_tag.end);
        }
        if matches!(name, "b" | "strong" | "th") {
            context.bold = true;
        }
        if let Some(size) = heading_size(name) {
            context.bold = true;
            context.size = size;
            context.level = name[1..].parse().ok();
        }
        if matches!(
            name,
            "ul" | "ol" | "menu" | "dir" | "dd" | "blockquote" | "figure"
        ) {
            context.indent += 1;
        }
        let kind = match name {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "heading",
            "p" => "paragraph",
            "li" => "list_item",
            "dt" => "term",
            "dd" => "definition",
            "pre" | "listing" | "plaintext" | "xmp" => "code",
            "blockquote" => "quote",
            "caption" | "figcaption" => "caption",
            "td" | "th" => "table_cell",
            name if BLOCK.contains(&name) => "text",
            _ => context.kind,
        };
        if kind != context.kind {
            context.kind = kind;
            if kind != "heading" {
                context.level = None;
            }
        }
        context
    }

    fn visit(&mut self, node: usize, context: Inherited, depth: usize) -> Result<()> {
        ensure!(depth < 512, "HTML nests elements more than 512 deep");
        for child in self.tree.children(node) {
            self.child(child, context, depth)?;
        }
        Ok(())
    }

    /// Reads node `child` of a node at `depth`.
    fn child(&mut self, child: usize, context: Inherited, depth: usize) -> Result<()> {
        let tree = self.tree;
        match &tree.nodes[child].kind {
            Kind::Text { raw } => {
                let range = tree.nodes[child].range.clone();
                read_text(&mut self.run, self.text, child, range, !raw, &context);
            }
            Kind::Element(element) if !self.hidden(child) => match &*element.name {
                "br" => {
                    let range = tree.nodes[child].range.clone();
                    self.run.line_break(child, range, &context);
                }
                "table" if !self.frame(child) => self.table(child, context, depth + 1)?,
                // Code a highlighter puts on a block element for each line.
                name if context.pre && BLOCK.contains(&name) => {
                    let inner = Inherited {
                        kind: context.kind,
                        level: context.level,
                        ..self.inside(child, context)
                    };
                    self.run.block_break = true;
                    self.visit(child, inner, depth + 1)?;
                    self.run.block_break = true;
                }
                name if BLOCK.contains(&name) => self.block(child, context, depth + 1)?,
                _ => {
                    let inner = self.inside(child, context);
                    self.visit(child, inner, depth + 1)?;
                }
            },
            _ => {}
        }
        Ok(())
    }

    /// Reads what table `node` or its row group or row holds outside its
    /// cells, which browsers show before the table.
    fn stray(&mut self, node: usize, context: Inherited, depth: usize) -> Result<()> {
        ensure!(depth < 512, "HTML nests elements more than 512 deep");
        for child in self.tree.children(node) {
            match self.tree.name(child) {
                "caption" | "colgroup" | "col" | "td" | "th" => {}
                "thead" | "tbody" | "tfoot" | "tr" => {
                    if !self.hidden(child) {
                        self.stray(child, context, depth + 1)?;
                    }
                }
                _ => self.child(child, context, depth)?,
            }
        }
        Ok(())
    }

    /// Whether table `node` holds tables, so that it lays them out as a
    /// frame rather than being a table of its own.
    fn frame(&self, node: usize) -> bool {
        self.holds_table[node]
    }

    fn block(&mut self, node: usize, context: Inherited, depth: usize) -> Result<()> {
        self.flush(true);
        self.entered += 1;
        let (made, entered) = (self.made, self.entered);
        let inner = self.inside(node, context);
        let name = self.tree.name(node);
        let item = name == "li";
        if item {
            self.marker = Some(marker(self.tree, node));
        }
        self.visit(node, inner, depth)?;
        self.flush(true);
        if item {
            self.marker = None;
        }
        // The element is the row's own when its text is all it holds.
        if self.cell.is_none()
            && self.made == made + 1
            && self.entered == entered
            && let Some(row) = self.last_row
        {
            if name == "caption" {
                self.layout.rows.get_mut(&row).unwrap().kind = RowKind::Caption(node);
            } else if ROW_ELEMENTS.contains(&name) {
                self.layout.rows.get_mut(&row).unwrap().kind = RowKind::Element(node);
            }
        }
        Ok(())
    }

    /// Ends the block being read: a row of its own outside tables, else a
    /// paragraph of the cell being read.
    fn flush(&mut self, breaks: bool) {
        let mut run = std::mem::replace(&mut self.run, Run::new());
        run.pending = None;
        // A line break ending a block starts no line, and a block of white
        // space alone, such as `&nbsp;` or a lone `<br>`, shows nothing.
        if run.after_break {
            run.pieces.pop();
            run.text.pop();
        }
        if run.text.chars().all(char::is_whitespace) {
            return;
        }
        self.made += 1;
        let (kind, level) = run.kind.unwrap_or(("text", None));
        // A heading outside tables ends the sections of its level and below.
        if let Some(level) = level
            && self.cell.is_none()
        {
            self.headings.retain(|(l, _)| *l < level);
            self.headings.push((level, run.text.replace('\n', " ")));
        }
        let block = Block {
            address: String::new(),
            text: run.text,
            pieces: run.pieces,
            breaks,
            bold: run.bold,
            size: run.size.unwrap_or(12),
            in_table: false,
            kind,
            headings: self.headings.iter().map(|(_, h)| h.clone()).collect(),
            indent: run.indent.unwrap_or(0),
            marker: self.marker.take(),
        };
        if let Some(cell) = &mut self.cell {
            cell.push(block);
            return;
        }
        let row = self.row;
        self.row += 1;
        self.layout.rows.insert(
            row,
            Row {
                kind: RowKind::Fixed("its text is not the whole content of an element"),
                anchor: run.anchor.unwrap_or(0),
                blocks: vec![self.layout.blocks.len()],
            },
        );
        self.layout.blocks.push(Block {
            address: format!("A{row}"),
            ..block
        });
        self.last_row = Some(row);
    }

    fn table(&mut self, table: usize, context: Inherited, depth: usize) -> Result<()> {
        self.flush(true);
        self.entered += 1;
        let tree = self.tree;
        self.stray(table, context, depth)?;
        self.flush(true);
        // A caption is shown above the table.
        for child in tree.children(table) {
            if tree.name(child) == "caption" && !self.hidden(child) {
                self.block(child, context, depth)?;
            }
        }
        // The rows of each row group, rows directly in the table making one
        // of their own, with the header first and the footer last as browsers
        // show them.
        let mut groups: Vec<(&str, Vec<usize>)> = vec![];
        let mut loose = false;
        for child in tree.children(table) {
            match tree.name(child) {
                "tr" => {
                    if !loose {
                        groups.push(("tbody", vec![]));
                        loose = true;
                    }
                    groups.last_mut().unwrap().1.push(child);
                }
                group @ ("thead" | "tbody" | "tfoot") => {
                    loose = false;
                    if !self.hidden(child) {
                        let rows = tree.children(child).filter(|r| tree.name(*r) == "tr");
                        groups.push((group, rows.collect()));
                    }
                }
                _ => {}
            }
        }
        groups.sort_by_key(|(group, _)| match *group {
            "thead" => 0,
            "tbody" => 1,
            _ => 2,
        });
        // Each row, its group and where the group ends, past which no cell
        // spans.
        let mut rows = vec![];
        for (group, members) in groups {
            let members: Vec<_> = members.into_iter().filter(|r| !self.hidden(*r)).collect();
            let end = rows.len() + members.len();
            rows.extend(members.into_iter().map(|row| (row, group, end)));
        }
        if rows.is_empty() {
            return Ok(());
        }
        // Each cell placed on the grid: (node, row, column, columns, rows).
        let count = rows.len();
        let mut taken: Vec<Vec<bool>> = vec![vec![]; count];
        let mut cells = vec![];
        for (r, &(row, _, end)) in rows.iter().enumerate() {
            let mut column = 0;
            for cell in tree.children(row) {
                if !matches!(tree.name(cell), "td" | "th") || self.hidden(cell) {
                    continue;
                }
                while taken[r].get(column).copied().unwrap_or(false) {
                    column += 1;
                }
                let element = self.tree.element(cell).unwrap();
                let number = |name: &str| {
                    element
                        .attribute(name)
                        .and_then(|v| v.trim().parse::<usize>().ok())
                };
                let across = number("colspan").unwrap_or(1).clamp(1, 1000);
                let down = match number("rowspan") {
                    Some(0) => end - r,
                    Some(n) => n.min(end - r),
                    None => 1,
                };
                for line in &mut taken[r..r + down] {
                    if line.len() < column + across {
                        line.resize(column + across, false);
                    }
                    line[column..column + across].fill(true);
                }
                cells.push((cell, r, column, across, down));
            }
        }
        let width = taken.iter().map(Vec::len).max().unwrap_or(0).max(1);
        drop(taken);
        let top = self.row;
        self.row += count;
        // The cells starting in each row, and the cells spanning rows that
        // start in or cross it, counted as they start and end.
        let mut own: Vec<Vec<(usize, usize)>> = vec![vec![]; count];
        let mut spans = vec![0isize; count + 1];
        for &(cell, r, column, _, down) in &cells {
            own[r].push((cell, column));
            if down > 1 {
                spans[r] += 1;
                spans[r + down] -= 1;
            }
        }
        let header_cells: Vec<bool> = own
            .iter()
            .map(|cells| {
                !cells.is_empty() && cells.iter().all(|(cell, _)| tree.name(*cell) == "th")
            })
            .collect();
        let mut spanning = 0;
        for (r, ((row, _, _), own)) in rows.iter().zip(own).enumerate() {
            spanning += spans[r];
            let spanned = spanning > 0;
            self.layout.rows.insert(
                top + r,
                Row {
                    kind: RowKind::TableRow {
                        row: *row,
                        table,
                        cells: own,
                        spanned,
                    },
                    anchor: *row,
                    blocks: vec![],
                },
            );
        }
        let address = |row: usize, column: usize| {
            Ok::<_, anyhow::Error>(format!("{}{row}", column_name(column as u32 + 1)?))
        };
        for &(cell, r, column, across, down) in &cells {
            let inner = self.inside(cell, context);
            self.cell = Some(vec![]);
            self.visit(cell, inner, depth + 1)?;
            self.flush(true);
            let paragraphs = self.cell.take().unwrap();
            let place = address(top + r, column)?;
            if across > 1 || down > 1 {
                self.layout.merges.push(format!(
                    "{place}:{}",
                    address(top + r + down - 1, column + across - 1)?
                ));
            }
            if let Some(mut block) = joined(paragraphs) {
                block.address = place;
                block.in_table = true;
                block.kind = "table_cell";
                self.layout
                    .rows
                    .get_mut(&(top + r))
                    .unwrap()
                    .blocks
                    .push(self.layout.blocks.len());
                self.layout.blocks.push(block);
            }
        }
        // Header rows: the rows of thead, else the leading rows of th cells.
        // Totals rows: the rows of tfoot, when rows are left between.
        let header = rows.iter().take_while(|(_, g, _)| *g == "thead").count();
        let header = if header > 0 && header < count {
            header
        } else {
            let leading = header_cells.iter().take_while(|th| **th).count();
            if leading < count { leading } else { 0 }
        };
        let totals = rows.iter().rev().take_while(|(_, g, _)| *g == "tfoot");
        let totals = Some(totals.count())
            .filter(|t| header + t < count)
            .unwrap_or(0);
        let name = format!("table-{}", self.layout.tables.len() + 1);
        self.layout.tables.push(json!({"name":name,
            "range":format!("A{top}:{}", address(top + count - 1, width - 1)?),
            "header_rows":header,"totals_rows":totals}));
        Ok(())
    }
}

/// The paragraphs of a table cell as one block, joined with line breaks.
fn joined(paragraphs: Vec<Block>) -> Option<Block> {
    let mut paragraphs = paragraphs.into_iter();
    let mut block = paragraphs.next()?;
    for next in paragraphs {
        let at = block.text.len();
        block.pieces.push(Piece {
            text: at..at + 1,
            sources: vec![],
            literal: false,
            pre: false,
        });
        block.text.push('\n');
        let shift = block.text.len();
        block
            .pieces
            .extend(next.pieces.into_iter().map(|mut piece| {
                piece.text = piece.text.start + shift..piece.text.end + shift;
                piece
            }));
        block.text.push_str(&next.text);
        block.bold &= next.bold;
    }
    Some(block)
}

/// The layout of the document `text`, whose tree is `tree`.
pub(super) fn layout(tree: &Tree, text: &str) -> Result<Layout> {
    // Nodes are numbered after their ancestors: whether a node holds a table
    // is known from the nodes after it, and whether it or an element holding
    // it shows no text from those before.
    let count = tree.nodes.len();
    let mut holds_table = vec![false; count];
    let mut excluded = vec![false; count];
    for node in (1..count).rev() {
        let table = tree
            .element(node)
            .is_some_and(|e| e.name == "table" && !e.foreign);
        if (table || holds_table[node])
            && let Some(parent) = tree.parent(node)
        {
            holds_table[parent] = true;
        }
    }
    for node in 1..count {
        excluded[node] =
            hidden(tree, node) || tree.parent(node).is_some_and(|parent| excluded[parent]);
    }
    let mut walker = Walker {
        tree,
        text,
        layout: Layout {
            blocks: vec![],
            merges: vec![],
            tables: vec![],
            rows: BTreeMap::new(),
            links: vec![],
            anchors: vec![],
            images: vec![],
        },
        run: Run::new(),
        row: 1,
        cell: None,
        made: 0,
        entered: 0,
        last_row: None,
        headings: vec![],
        holds_table,
        marker: None,
    };
    let context = Inherited {
        pre: false,
        pre_start: None,
        bold: false,
        size: 12,
        kind: "text",
        level: None,
        indent: 0,
    };
    let title_context = Inherited {
        kind: "title",
        ..context
    };
    // The title of the document comes first.
    if let Some(title) = (0..tree.nodes.len()).find(|n| {
        tree.element(*n)
            .is_some_and(|e| e.name == "title" && !e.foreign)
    }) {
        for child in tree.children(title) {
            if matches!(tree.nodes[child].kind, Kind::Text { .. }) {
                read_text(
                    &mut walker.run,
                    text,
                    child,
                    tree.nodes[child].range.clone(),
                    true,
                    &title_context,
                );
            }
        }
        walker.run.anchor.get_or_insert(title);
        walker.flush(false);
        if let Some(row) = walker.last_row {
            walker.layout.rows.get_mut(&row).unwrap().kind =
                RowKind::Fixed("it is the document title");
        }
    }
    // A page that marks its main content has its body text there.
    let main = main_of(tree, &excluded);
    match main {
        Some(main) => walker.block(main, context, 1)?,
        None => walker.visit(0, context, 0)?,
    }
    walker.flush(true);
    let mut layout = walker.layout;
    // Within the main content, else anywhere shown.
    let scope = main.map_or(0..count, |main| main..tree.descendants(main).end);
    for (node, excluded) in excluded.iter_mut().enumerate() {
        *excluded |= !scope.contains(&node);
    }
    // A link's fragment finds any element of the page, shown or not, but
    // not the content of a template, which is not part of the page.
    let mut inert = vec![false; count];
    for node in 1..count {
        inert[node] = tree
            .parent(node)
            .is_some_and(|parent| inert[parent] || tree.name(parent) == "template");
    }
    layout.link(tree, &excluded, &inert);
    Ok(layout)
}

/// The extraction sheet of `layout`.
pub(super) fn sheet(
    layout: &Layout,
    raw: &[u8],
    decoded: &Decoded,
) -> Result<crate::extraction::Sheet> {
    // Where each block lies in the original, from its first character to its
    // last, as its position gives it: lines and bytes of the original. The
    // bytes of an encoding that is not written back are not placed.
    let ranges: Vec<Option<Range<usize>>> = layout
        .blocks
        .iter()
        .map(|block| {
            let sources = || block.pieces.iter().flat_map(|p| &p.sources);
            Some(sources().map(|s| s.start).min()?..sources().map(|s| s.end).max()?)
        })
        .collect();
    let wanted: BTreeSet<usize> = ranges
        .iter()
        .flatten()
        .flat_map(|r| [r.start, r.end])
        .collect();
    let offsets = html_parse::raw_offsets(raw, decoded, &wanted).ok();
    let text = decoded.text.as_bytes();
    let mut lines = vec![0];
    for (at, byte) in text.iter().enumerate() {
        if *byte == b'\n' || (*byte == b'\r' && text.get(at + 1) != Some(&b'\n')) {
            lines.push(at + 1);
        }
    }
    let line = |at: usize| lines.partition_point(|start| *start <= at);
    let mut sheet = crate::extraction::Sheet::build(json!({}));
    for (index, block) in layout.blocks.iter().enumerate() {
        let position = match (&ranges[index], &offsets) {
            (Some(range), Some(offsets)) => Some(json!({"kind":block.kind,
                "headings":block.headings,"line_start":line(range.start),
                "line_end":line(range.end.saturating_sub(1).max(range.start)),
                "byte_start":offsets[&range.start],"byte_end":offsets[&range.end],"columns":[]})),
            _ => None,
        };
        let cell = crate::extraction::NewCell {
            id: &format!("text-{}", index + 1),
            address: &block.address,
            kind: crate::extraction::CellKind::String,
            value: &Value::String(block.text.clone()),
            formula: None,
            number_format: "",
            font: None,
            style: None,
            position,
        };
        sheet.push(
            cell,
            Some(json!({"latin":null,"east_asian":null,"size":block.size,"color":null})),
            Some(json!({"bold":block.bold,"fill":0,"border":u8::from(block.in_table)})),
        )?;
    }
    let mut sheet = sheet.finish();
    sheet.fields = json!({"name":"document","part":"html","state":"visible",
        "merges":layout.merges,"tables":layout.tables,
        "links":layout.links,"anchors":layout.anchors,"images":layout.images});
    Ok(sheet)
}

/// `text` written as HTML text: `&`, `<` and `>` as references, and outside
/// `pre` a line break as `<br>`.
fn escaped(text: &str, pre: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' if !pre => out.push_str("<br>"),
            c => out.push(c),
        }
    }
    out
}

/// Refuses text whose `start..end` would show otherwise than written outside
/// `pre`: HTML collapses white space to one space, and drops it at the start
/// and end of a line.
fn ensure_spaces(after: &str, start: usize, end: usize, place: &str) -> Result<()> {
    ensure!(
        !after[start..end].contains(['\t', '\x0c']),
        "HTML shows a tab as a space ({place}); write the text without tabs, or edit text inside <pre>"
    );
    let from = after[..start]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i);
    let to = after[end..]
        .chars()
        .next()
        .map_or(after.len(), |c| end + c.len_utf8());
    let window = &after[from..to];
    ensure!(
        !window.contains("  ")
            && !window.contains(" \n")
            && !window.contains("\n ")
            && !(from == 0 && window.starts_with(' '))
            && !(to == after.len() && window.ends_with(' ')),
        "HTML shows consecutive spaces as one space and drops spaces at the start and end of a line ({place}); write a space HTML keeps as U+00A0 (no-break space)"
    );
    Ok(())
}

impl Block {
    /// The piece holding the character at byte `at` of the text or ending
    /// there, that an edit at `at` writes into.
    fn target(&self, at: usize) -> Option<&Piece> {
        self.pieces
            .iter()
            .find(|p| p.text.start <= at && at < p.text.end && !p.sources.is_empty())
            .or_else(|| {
                self.pieces
                    .iter()
                    .find(|p| p.text.end == at && !p.sources.is_empty())
            })
    }

    /// The edits of the original that replace `start..end` of the text with
    /// `inserted`.
    fn source_edits(
        &self,
        start: usize,
        end: usize,
        inserted: &str,
    ) -> Result<Vec<(Range<usize>, String)>> {
        let place = &self.address;
        if start == end {
            // An insertion joins the piece ending there, else the one starting
            // there; white space HTML dropped before that is left out of both.
            let ending = self
                .pieces
                .iter()
                .find(|p| p.text.end == start && !p.sources.is_empty());
            let starting = self
                .pieces
                .iter()
                .find(|p| p.text.start <= start && start < p.text.end && !p.sources.is_empty());
            let (position, pre) = match (ending, starting) {
                (Some(piece), _) => (piece.sources.last().unwrap().end, piece.pre),
                (None, Some(piece)) if piece.text.start == start => {
                    (piece.sources[0].start, piece.pre)
                }
                (None, Some(piece)) if piece.literal => {
                    (piece.sources[0].start + start - piece.text.start, piece.pre)
                }
                _ => bail!(
                    "the edit in {place} falls inside a character reference or between the paragraphs of a table cell; include the whole character, or edit each paragraph separately"
                ),
            };
            return Ok(vec![(position..position, escaped(inserted, pre))]);
        }
        let touched: Vec<&Piece> = self
            .pieces
            .iter()
            .filter(|p| p.text.start < end && start < p.text.end)
            .collect();
        ensure!(
            touched.iter().all(|p| !p.sources.is_empty()),
            "the edit crosses the line between two paragraphs of a table cell in {place}; edit each paragraph separately"
        );
        let mut edits = vec![];
        for piece in &touched {
            if piece.literal {
                let base = piece.sources[0].start - piece.text.start;
                edits.push((
                    base + start.max(piece.text.start)..base + end.min(piece.text.end),
                    String::new(),
                ));
            } else {
                ensure!(
                    start <= piece.text.start && piece.text.end <= end,
                    "the edit in {place} changes part of the characters of one character reference; change them all"
                );
                edits.extend(piece.sources.iter().map(|s| (s.clone(), String::new())));
            }
        }
        edits[0].1 = escaped(inserted, touched[0].pre);
        Ok(edits)
    }

    /// The edits of the original `text` that make the block show `after`.
    /// The change must stay within one paragraph of a table cell; when it
    /// spans text in several elements, the first takes the new text, as
    /// Word's first run does.
    fn edit(&self, text: &str, after: &str) -> Result<Vec<(Range<usize>, String)>> {
        let before = self.text.as_str();
        let place = &self.address;
        let (start, end) = word::changed_range(before, after, true);
        let inserted_end = after.len() - (before.len() - end);
        if start == end && start == inserted_end {
            return Ok(vec![]);
        }
        let inserted = &after[start..inserted_end];
        ensure!(
            !inserted.contains('\r'),
            "HTML text replacement cannot contain a carriage return in {place}; write a line break as \\n"
        );
        ensure!(
            self.breaks || !inserted.contains('\n'),
            "the document title cannot take line breaks ({place})"
        );
        if !self.target(start).is_some_and(|p| p.pre) {
            ensure_spaces(after, start, inserted_end, place)?;
        }
        let edits = self.source_edits(start, end, inserted)?;
        // Repeated text lets the same change sit further left; refuse it when
        // that writes the original otherwise, as Word does for runs.
        let (left_start, left_end) = word::changed_range(before, after, false);
        if (left_start, left_end) != (start, end)
            && let Ok(left) = self.source_edits(
                left_start,
                left_end,
                &after[left_start..after.len() - (before.len() - left_end)],
            )
        {
            let ranges = edits.iter().chain(&left).map(|(r, _)| r);
            let from = ranges.clone().map(|r| r.start).min().unwrap_or(0);
            let to = ranges.map(|r| r.end).max().unwrap_or(0);
            ensure!(
                applied(text, from..to, &edits) == applied(text, from..to, &left),
                "the edit in {place} could apply to more than one place because the text around it repeats; include unrepeated text in the change"
            );
        }
        Ok(edits)
    }
}

/// `region` of `text` after `edits`, which lie within it.
fn applied(text: &str, region: Range<usize>, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::new();
    let mut at = region.start;
    let mut sorted: Vec<_> = edits.iter().collect();
    sorted.sort_by_key(|(r, _)| (r.start, r.end));
    for (range, new) in sorted {
        out.push_str(&text[at..range.start]);
        out.push_str(new);
        at = range.end;
    }
    out.push_str(&text[at..region.end]);
    out
}

/// The edits row operations make, the ranges they delete, and the rows after them.
struct Restructured {
    edits: Vec<(Range<usize>, String)>,
    removed: Vec<Range<usize>>,
    placed: Vec<Placed>,
}

/// The start tag of element `node` without its `id`, which names one element.
fn opening(tree: &Tree, text: &str, node: usize) -> String {
    static ID: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)\s+id\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+)"#).unwrap()
    });
    let element = tree.element(node).unwrap();
    ID.replace_all(&text[element.start_tag.clone()], "")
        .into_owned()
}

impl Layout {
    /// Edits that insert and delete block elements and table rows as
    /// `operations` (row insertions and deletions, in order) do. A new row
    /// copies the element it takes its format from (the row above it, or
    /// `style_from`) with the text `values` gives it, next to the rows around
    /// it that share its parent; a table or list whose rows are all deleted is
    /// deleted.
    fn restructure(
        &self,
        tree: &Tree,
        text: &str,
        operations: &[&StructuralOperation],
        values: &InsertedText,
    ) -> Result<Restructured> {
        let placed = word::placed_rows(&self.rows, operations, "HTML")?;
        if operations.is_empty() {
            return Ok(Restructured {
                edits: vec![],
                removed: vec![],
                placed,
            });
        }
        let (before_rows, after_rows) = word::original_neighbors(&placed);
        let mut insertions: BTreeMap<usize, String> = BTreeMap::new();
        for (index, entry) in placed.iter().enumerate() {
            let Placed::Inserted {
                operation,
                offset,
                template,
            } = entry
            else {
                continue;
            };
            let value = |column: &str| -> Result<String> {
                let value = values
                    .get(&(operation.clone(), *offset, column.to_owned()))
                    .cloned()
                    .unwrap_or_default();
                ensure!(
                    !value.contains('\r'),
                    "HTML text cannot contain a carriage return; write a line break as \\n"
                );
                Ok(value)
            };
            let (node, markup) = match &self.rows[template].kind {
                RowKind::Element(node) => {
                    let name = tree.name(*node);
                    let pre = matches!(name, "pre" | "listing");
                    let mut value = value("A")?;
                    if !pre && !value.is_empty() {
                        ensure_spaces(
                            &value,
                            0,
                            value.len(),
                            &format!("{operation}-{}", offset + 1),
                        )?;
                    }
                    // HTML drops a line break that starts the content of pre.
                    if pre && value.starts_with('\n') {
                        value.insert(0, '\n');
                    }
                    (
                        *node,
                        format!(
                            "{}{}</{name}>",
                            opening(tree, text, *node),
                            escaped(&value, pre)
                        ),
                    )
                }
                RowKind::TableRow {
                    row,
                    cells,
                    spanned,
                    ..
                } => {
                    ensure!(
                        !spanned,
                        "row {template} is part of a cell spanning rows, so it cannot be copied; insert the row in the HTML"
                    );
                    let mut markup = opening(tree, text, *row);
                    for (cell, column) in cells {
                        let letter = column_name(*column as u32 + 1)?;
                        let value = value(&letter)?;
                        if !value.is_empty() {
                            ensure_spaces(
                                &value,
                                0,
                                value.len(),
                                &format!("{operation}-{} {letter}", offset + 1),
                            )?;
                        }
                        markup.push_str(&format!(
                            "{}{}</{}>",
                            opening(tree, text, *cell),
                            escaped(&value, false),
                            tree.name(*cell)
                        ));
                    }
                    markup.push_str("</tr>");
                    (*row, markup)
                }
                RowKind::Caption(_) => bail!(
                    "row {template} is the caption of a table, which has one; copy another row"
                ),
                RowKind::Fixed(reason) => bail!(
                    "row {template} cannot be copied: {reason}; choose --style-from a paragraph, list item or table row"
                ),
            };
            // Next to the nearest original row on either side, or the
            // element holding it, that shares the new element's parent.
            let parent = tree.parent(node);
            let sibling = |row: usize| {
                let anchor = self.rows[&row].anchor;
                std::iter::once(anchor)
                    .chain(tree.ancestors(anchor))
                    .find(|n| tree.parent(*n) == parent)
            };
            let position = before_rows[index]
                .and_then(sibling)
                .map(|n| tree.nodes[n].range.end)
                .or_else(|| {
                    after_rows[index]
                        .and_then(sibling)
                        .map(|n| tree.nodes[n].range.start)
                })
                .with_context(|| {
                    format!(
                        "a {} copied from row {template} has no place next to the rows around it; choose --style-from a row of the same list, table or section",
                        tree.name(node)
                    )
                })?;
            insertions.entry(position).or_default().push_str(&markup);
        }
        let kept: BTreeSet<usize> = placed
            .iter()
            .filter_map(|p| match p {
                Placed::Original(row) => Some(*row),
                Placed::Inserted { .. } => None,
            })
            .collect();
        // The rows and kept rows inside each list and table.
        let mut inside: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        if kept.len() < self.rows.len() {
            for (row, info) in &self.rows {
                for container in std::iter::once(info.anchor).chain(tree.ancestors(info.anchor)) {
                    let name = tree.name(container);
                    if LISTS.contains(&name) || name == "table" {
                        let entry = inside.entry(container).or_default();
                        entry.0 += 1;
                        entry.1 += usize::from(kept.contains(row));
                    }
                }
            }
        }
        let emptied = |container: usize| inside.get(&container).is_some_and(|(_, kept)| *kept == 0);
        let mut removed = vec![];
        for (row, info) in &self.rows {
            if kept.contains(row) {
                continue;
            }
            let node = match &info.kind {
                RowKind::Element(node) => match tree.parent(*node) {
                    Some(list) if LISTS.contains(&tree.name(list)) && emptied(list) => list,
                    _ => *node,
                },
                RowKind::TableRow {
                    row: tr,
                    table,
                    spanned,
                    ..
                } => {
                    ensure!(
                        !spanned,
                        "row {row} is part of a cell spanning rows; delete it in the HTML"
                    );
                    if emptied(*table) { *table } else { *tr }
                }
                RowKind::Caption(node) => *node,
                RowKind::Fixed(reason) => bail!("row {row} cannot be deleted: {reason}"),
            };
            removed.push(tree.nodes[node].range.clone());
        }
        // A list or table taken whole takes its rows.
        removed.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        let mut outer: Vec<Range<usize>> = vec![];
        for range in removed {
            if outer.last().is_none_or(|last| range.start >= last.end) {
                outer.push(range);
            }
        }
        let mut edits: Vec<(Range<usize>, String)> = insertions
            .into_iter()
            .map(|(position, markup)| (position..position, markup))
            .collect();
        edits.extend(outer.iter().map(|r| (r.clone(), String::new())));
        Ok(Restructured {
            edits,
            removed: outer,
            placed,
        })
    }

    /// The texts of the blocks after the rows are `placed`, the blocks keyed
    /// by index in `changes` show their new text and inserted rows `values`.
    fn expected(
        &self,
        placed: &[Placed],
        changes: &BTreeMap<usize, String>,
        values: &InsertedText,
    ) -> Result<Vec<String>> {
        let mut texts = vec![];
        for entry in placed {
            match entry {
                Placed::Original(row) => {
                    for block in &self.rows[row].blocks {
                        texts.push(
                            changes
                                .get(block)
                                .cloned()
                                .unwrap_or_else(|| self.blocks[*block].text.clone()),
                        );
                    }
                }
                Placed::Inserted {
                    operation,
                    offset,
                    template,
                } => {
                    let columns = match &self.rows[template].kind {
                        RowKind::TableRow { cells, .. } => cells
                            .iter()
                            .map(|(_, column)| column_name(*column as u32 + 1))
                            .collect::<Result<Vec<_>>>()?,
                        _ => vec!["A".to_owned()],
                    };
                    for column in columns {
                        let value = values
                            .get(&(operation.clone(), *offset, column))
                            .cloned()
                            .unwrap_or_default();
                        if !value.is_empty() {
                            texts.push(value);
                        }
                    }
                }
            }
        }
        Ok(texts)
    }
}

/// The document `raw` with the blocks keyed by index in `changes` showing
/// their new text and the rows `operations` insert (with `values`) and
/// delete, read back to check that it shows what they ask.
pub(super) fn patch(
    raw: &[u8],
    operations: &[&StructuralOperation],
    changes: &BTreeMap<usize, String>,
    values: &InsertedText,
) -> Result<Vec<u8>> {
    let decoded = html_parse::decode(raw)?;
    html_parse::ensure_writable(decoded.encoding)?;
    let tree = html_parse::parse(&decoded.text);
    let layout = layout(&tree, &decoded.text)?;
    let restructured = layout.restructure(&tree, &decoded.text, operations, values)?;
    let mut edits = restructured.edits;
    for (index, after) in changes {
        let block = layout
            .blocks
            .get(*index)
            .context("text block disappeared")?;
        for (range, new) in block.edit(&decoded.text, after)? {
            // The text of a deleted row goes with it. The deleted ranges are
            // in order and apart.
            let before = restructured
                .removed
                .partition_point(|r| r.start <= range.start);
            if !before
                .checked_sub(1)
                .is_some_and(|k| range.end <= restructured.removed[k].end)
            {
                edits.push((range, new));
            }
        }
    }
    // The texts to read back are all that is kept of the original's layout
    // while the output is read.
    let expected = layout.expected(&restructured.placed, changes, values)?;
    drop((tree, layout));
    let output = splice(raw, &decoded, edits)?;
    let read = html_parse::decode(&output)?;
    ensure!(
        read.encoding == decoded.encoding,
        "the edited HTML no longer reads as {}",
        decoded.encoding.name()
    );
    let shown: Vec<String> = self::layout(&html_parse::parse(&read.text), &read.text)?
        .blocks
        .into_iter()
        .map(|b| b.text)
        .collect();
    if let Some(index) =
        (0..shown.len().max(expected.len())).find(|i| shown.get(*i) != expected.get(*i))
    {
        bail!(
            "the edited HTML reads back otherwise than the edits ask at text block {}: expected {:?}, read {:?}; edit the original in an HTML editor",
            index + 1,
            expected.get(index).map_or("(none)", String::as_str),
            shown.get(index).map_or("(none)", String::as_str)
        );
    }
    Ok(output)
}

/// Rejects row `operations` the document `raw` cannot take, before any
/// output is written.
pub(super) fn ensure_rows(raw: &[u8], operations: &[&StructuralOperation]) -> Result<()> {
    let decoded = html_parse::decode(raw)?;
    html_parse::ensure_writable(decoded.encoding)?;
    let tree = html_parse::parse(&decoded.text);
    layout(&tree, &decoded.text)?.restructure(
        &tree,
        &decoded.text,
        operations,
        &InsertedText::new(),
    )?;
    Ok(())
}

/// `raw` with `edits` of its decoded text made on its bytes, each new text
/// in the encoding of the original, so that the bytes around them stay.
fn splice(
    raw: &[u8],
    decoded: &Decoded,
    mut edits: Vec<(Range<usize>, String)>,
) -> Result<Vec<u8>> {
    edits.sort_by_key(|(r, _)| (r.start, r.end));
    for pair in edits.windows(2) {
        ensure!(
            pair[0].0.end <= pair[1].0.start,
            "edits of the HTML overlap; make the row operations and text edits in separate exports"
        );
    }
    let wanted: BTreeSet<usize> = edits.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    let offsets = html_parse::raw_offsets(raw, decoded, &wanted)?;
    let mut output = Vec::with_capacity(raw.len());
    let mut at = 0;
    for (range, new) in &edits {
        let (start, end) = (offsets[&range.start], offsets[&range.end]);
        output.extend_from_slice(&raw[at..start]);
        output.extend_from_slice(&html_parse::encode(new, decoded.encoding)?);
        at = end;
    }
    output.extend_from_slice(&raw[at..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::excel::OperationKind;

    fn laid(html: &str) -> Layout {
        layout(&html_parse::parse(html), html).unwrap()
    }

    /// Pages of about `size` bytes in shapes that stress the reader: a
    /// manual, one long table, unclosed inline tags, deep nesting, and one
    /// paragraph of references and line breaks.
    fn bench_pages(size: usize) -> Vec<(&'static str, String)> {
        let repeat = |unit: &dyn Fn(usize) -> String| {
            let mut page = String::from(
                "<!DOCTYPE html><html><head><meta charset=utf-8><title>Bench</title></head><body>\n",
            );
            let mut i = 0;
            while page.len() < size {
                page.push_str(&unit(i));
                i += 1;
            }
            page
        };
        vec![
            (
                "manual",
                repeat(&|i| {
                    format!(
                        "<h2 id=\"s{i}\">Section {i}</h2>\n<p>The <code>core.autocrlf</code> setting &amp; its <a href=\"x{i}.html#y\">link</a> controls\n   how line endings are handled.<br>\n  Second line here.</p>\n<ul><li>item one</li><li>item <b>two</b></li></ul>\n<pre>git config --global core.autocrlf true\n</pre>\n<table><tr><th>key<th>value<tr><td>a{i}<td>1</table>\n"
                    )
                }),
            ),
            ("table", {
                let mut page = repeat(&|i| {
                    if i == 0 {
                        "<table><thead><tr><th>ID<th>Name<th>Type<th>Default<th>Note</thead><tbody>\n".into()
                    } else {
                        format!(
                            "<tr><td>F{i:06}</td><td>field name {i}</td><td>string</td><td>none</td><td>note &lt;{i}&gt;</td></tr>\n"
                        )
                    }
                });
                page.push_str("</tbody></table>");
                page
            }),
            (
                "unclosed",
                repeat(&|i| {
                    if i % 10 == 9 {
                        format!("<p>paragraph {i}</p>\n")
                    } else {
                        format!("<span class=c>text {i} ")
                    }
                }),
            ),
            (
                "deep",
                repeat(&|i| {
                    if i < 2000 {
                        "<div>".into()
                    } else {
                        format!("word{i} ")
                    }
                }),
            ),
            (
                "references",
                repeat(&|i| format!("w{i} &amp; &#x41;&nbsp;<br>\n ")),
            ),
        ]
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn bench_html_scaling() {
        use std::time::Instant;
        let mb = |bytes: usize| bytes as f64 / 1048576.0;
        for size in [1 << 20, 4 << 20, 16 << 20] {
            for (shape, page) in bench_pages(size) {
                let raw = page.into_bytes();
                let start = Instant::now();
                let decoded = html_parse::decode(&raw).unwrap();
                let decode = start.elapsed();
                let tree = html_parse::parse(&decoded.text);
                let parse = start.elapsed() - decode;
                let laid = layout(&tree, &decoded.text);
                let lay = start.elapsed() - decode - parse;
                let Ok(laid) = laid else {
                    eprintln!(
                        "{shape} {:.0}MB: layout failed: {}",
                        mb(raw.len()),
                        laid.err().unwrap()
                    );
                    continue;
                };
                let sheet_start = Instant::now();
                sheet(&laid, &raw, &decoded).unwrap();
                let sheet_time = sheet_start.elapsed();
                // One edit in every hundredth block, read back.
                let changes: BTreeMap<usize, String> = (0..laid.blocks.len())
                    .step_by(100)
                    .map(|i| (i, format!("{}x", laid.blocks[i].text)))
                    .collect();
                let patch_start = Instant::now();
                let patched = patch(&raw, &[], &changes, &InsertedText::new());
                let patch_time = patch_start.elapsed();
                let pieces: usize = laid.blocks.iter().map(|b| b.pieces.len()).sum();
                let sources: usize = laid
                    .blocks
                    .iter()
                    .flat_map(|b| &b.pieces)
                    .map(|p| p.sources.len())
                    .sum();
                let attributes: usize = tree
                    .nodes
                    .iter()
                    .filter_map(|n| match &n.kind {
                        Kind::Element(e) => Some(e.attributes.len()),
                        _ => None,
                    })
                    .sum();
                let estimate = tree.nodes.len() * std::mem::size_of::<html_parse::Node>()
                    + tree.nodes.len() * std::mem::size_of::<usize>()
                    + attributes * 2 * std::mem::size_of::<String>()
                    + pieces * std::mem::size_of::<Piece>()
                    + sources * std::mem::size_of::<Range<usize>>();
                eprintln!(
                    "{shape:10} {:5.1}MB nodes={:8} blocks={:7} pieces={:8} est={:6.0}MB decode={:5}ms parse={:5}ms layout={:6}ms sheet={:5}ms patch({} edits)={:6}ms {}",
                    mb(raw.len()),
                    tree.nodes.len(),
                    laid.blocks.len(),
                    pieces,
                    mb(estimate),
                    decode.as_millis(),
                    parse.as_millis(),
                    lay.as_millis(),
                    sheet_time.as_millis(),
                    changes.len(),
                    patch_time.as_millis(),
                    patched
                        .err()
                        .map_or(String::new(), |e| format!("patch failed: {e}"))
                );
            }
        }
    }

    fn texts(html: &str) -> Vec<(String, String)> {
        laid(html)
            .blocks
            .into_iter()
            .map(|b| (b.address, b.text))
            .collect()
    }

    fn edited(html: &str, index: usize, after: &str) -> Result<String> {
        let changes = BTreeMap::from([(index, after.to_owned())]);
        Ok(
            String::from_utf8(patch(html.as_bytes(), &[], &changes, &InsertedText::new())?)
                .unwrap(),
        )
    }

    fn operation(kind: OperationKind, at: u32, count: u32) -> StructuralOperation {
        StructuralOperation {
            id: "op".into(),
            sheet: "document".into(),
            kind,
            at,
            count,
            style_from: None,
            style_rows: 1,
            merges: false,
            vba_unchanged: false,
            band: None,
        }
    }

    #[test]
    fn blocks_collapse_white_space_and_keep_pre() {
        let html = "<title> Guide\n</title><h1>Install</h1>\n<p>Run  the\n  <b>setup</b> tool.<br>\n  Then restart.</p><pre>\n a  b\n</pre><ul><li>one<li>two &amp; three</ul>";
        assert_eq!(
            texts(html),
            [
                ("A1", "Guide"),
                ("A2", "Install"),
                ("A3", "Run the setup tool.\nThen restart."),
                ("A4", " a  b\n"),
                ("A5", "one"),
                ("A6", "two & three"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        let layout = laid(html);
        assert_eq!(layout.blocks[1].size, 24);
        assert!(layout.blocks[1].bold && !layout.blocks[2].bold);
    }

    #[test]
    fn blocks_name_the_headings_they_are_under() {
        let html = "<h1>Guide</h1><p>a</p><h2>Install</h2><p>b</p><h3>Linux</h3><h2>Use</h2><ul><li>c</ul><h1>FAQ</h1><table><tr><td>d</table>";
        let headings: Vec<_> = laid(html)
            .blocks
            .into_iter()
            .map(|b| (b.kind, b.headings))
            .collect();
        let expected: Vec<(&str, Vec<&str>)> = vec![
            ("heading", vec!["Guide"]),
            ("paragraph", vec!["Guide"]),
            ("heading", vec!["Guide", "Install"]),
            ("paragraph", vec!["Guide", "Install"]),
            ("heading", vec!["Guide", "Install", "Linux"]),
            ("heading", vec!["Guide", "Use"]),
            ("list_item", vec!["Guide", "Use"]),
            ("heading", vec!["FAQ"]),
            ("table_cell", vec!["FAQ"]),
        ];
        assert_eq!(
            headings,
            expected
                .into_iter()
                .map(|(k, h)| (k, h.into_iter().map(str::to_owned).collect()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn links_and_anchors_name_their_blocks() {
        let html = "<head><link id=css href=x.css></head><h1 id=top>Guide</h1><section id=install><p>See <a href=\"linux.html#apt\">the <b>Linux</b> page</a> and <a href=#top>top</a>.</p></section><p><a name=old></a>Old <a href=pic.html><img src=p.png></a></p><p hidden><a href=no.html id=gone>x</a></p><template><p id=inert>t</p></template><h1 id=top>Again</h1>";
        let layout = laid(html);
        assert_eq!(
            layout.links,
            [
                json!({"range":"A2","url":"linux.html#apt","text":"the Linux page"}),
                json!({"range":"A2","url":"#top","text":"top"}),
                json!({"range":"A3","url":"pic.html"}),
            ]
        );
        // Elements shown or not are found by a link, at the block after
        // them; the content of a template is not part of the page.
        assert_eq!(
            layout.anchors,
            [
                json!({"name":"css","range":"A1"}),
                json!({"name":"top","range":"A1"}),
                json!({"name":"install","range":"A2"}),
                json!({"name":"old","range":"A3"}),
                json!({"name":"gone","range":"A4"}),
            ]
        );
    }

    #[test]
    fn pictures_name_their_blocks_and_become_assets() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let html = format!(
            "<p>Intro <img src=\"img/a%20b.png\" alt=\"図1 構成\"></p><figure><img src=\"data:image/png;base64,{PNG}\"><figcaption>Caption</figcaption></figure><p><img src=missing.png alt=\"\"></p><p>end <img src=\"img/a b.png\"></p><img src=https://example.com/x.png>"
        );
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("img")).unwrap();
        fs::write(temp.path().join("img/a b.png"), b"picture").unwrap();
        let mut layout = laid(&html);
        let folders = [crate::data::canonicalize(temp.path()).unwrap()];
        let pictures = layout.assets(&temp.path().join("page.html"), &folders);
        let images: Vec<_> = layout
            .images
            .iter()
            .map(|i| (i["address"].clone(), i["alt"].clone(), i["asset"].clone()))
            .collect();
        assert_eq!(
            images,
            [
                (json!("A1"), json!("図1 構成"), json!("image-001.png")),
                (json!("A2"), Value::Null, json!("image-002.png")),
                (json!("A3"), json!(""), Value::Null),
                (json!("A3"), Value::Null, json!("image-001.png")),
                (json!("A3"), Value::Null, Value::Null),
            ]
        );
        assert_eq!(pictures.files.keys().collect::<Vec<_>>(), ["image-001.png"]);
        assert_eq!(
            pictures.embedded["image-002.png"][..4],
            [0x89, b'P', b'N', b'G']
        );
    }

    #[test]
    fn pictures_outside_the_folders_are_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::data::canonicalize(temp.path()).unwrap();
        for file in ["site/img/a.png", "shared/b.png", "secret.png"] {
            fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
            fs::write(root.join(file), file).unwrap();
        }
        let html = "<p>図 <img src=img/a.png><img src=../shared/b.png><img src=../secret.png><img src=img/../../secret.png></p>";
        let mut layout = laid(html);
        let pictures = layout.assets(
            &root.join("site/page.html"),
            &[root.join("site"), root.join("shared")],
        );
        let assets: Vec<_> = layout.images.iter().map(|i| i["asset"].clone()).collect();
        assert_eq!(
            assets,
            [
                json!("image-001.png"),
                json!("image-002.png"),
                Value::Null,
                Value::Null
            ]
        );
        assert_eq!(
            pictures.files.values().collect::<Vec<_>>(),
            [&root.join("site/img/a.png"), &root.join("shared/b.png")]
        );
    }

    #[test]
    fn tab_panels_not_chosen_are_read() {
        let html = "<div role=tablist><button role=tab>Python</button><button role=tab>Rust</button></div><div role=tabpanel><pre>print(1)</pre></div><div role=tabpanel hidden><pre>println!(1)</pre><p hidden>gone</p></div><p hidden>no</p>";
        let texts: Vec<_> = laid(html).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["PythonRust", "print(1)", "println!(1)"]);
    }

    #[test]
    fn landmarks_keep_the_site_frame_out_of_the_body() {
        let page = "<title>API</title><header role=banner id=top><a href=/>Home</a></header><nav><a href=a.html>A</a></nav>";
        // The main content, when marked, is the body; its own navigation is not.
        let html = format!(
            "{page}<div role=main><h1 id=api>API</h1><nav class=toc><a href=#api id=toc-api>x</a></nav><p>Text <a href=b.html>B</a></p></div><footer role=contentinfo id=end>(c)</footer><p>after</p>"
        );
        let layout = laid(&html);
        let texts: Vec<_> = layout.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["API", "API", "Text B"]);
        assert_eq!(
            layout.links,
            [json!({"range":"A3","url":"b.html","text":"B"})]
        );
        // A link still finds the names in the frame, as a heading's link back
        // to its entry in the table of contents: at the block after them.
        assert_eq!(
            layout.anchors,
            [
                json!({"name":"top","range":"A2"}),
                json!({"name":"api","range":"A2"}),
                json!({"name":"toc-api","range":"A3"}),
                json!({"name":"end","range":"A3"}),
            ]
        );
        // Without it, the frame's landmarks are left out.
        let html = format!(
            "{page}<header><h1>Guide</h1></header><div role=search>find</div><p>Body</p><footer role=contentinfo>(c)</footer>"
        );
        let texts: Vec<_> = laid(&html).blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, ["API", "Guide", "Body"]);
    }

    #[test]
    fn blocks_take_the_indent_and_markers_browsers_give_them() {
        let html = "<p>Steps</p><ol start=3><li>Get</li><li><p>Unpack</p><ul><li>zip<ul><li>gz</li></ul></li></ul></li><li value=10>Run</li></ol>\
            <dl><dt>key</dt><dd>its value</dd></dl><blockquote><p>Note</p></blockquote><li>alone</li>";
        let shapes: Vec<_> = laid(html)
            .blocks
            .into_iter()
            .map(|b| (b.kind, b.indent, b.marker))
            .collect();
        let marked = |kind, indent, marker: &str| (kind, indent, Some(marker.to_owned()));
        assert_eq!(
            shapes,
            [
                ("paragraph", 0, None),
                marked("list_item", 1, "3."),
                marked("paragraph", 1, "4."),
                marked("list_item", 2, "◦"),
                marked("list_item", 3, "▪"),
                marked("list_item", 1, "10."),
                ("term", 0, None),
                ("definition", 1, None),
                ("paragraph", 1, None),
                marked("list_item", 0, "•"),
            ]
        );
    }

    fn kinds(layout: &Layout) -> Vec<(&str, &str)> {
        layout
            .blocks
            .iter()
            .map(|b| (b.kind, b.text.as_str()))
            .collect()
    }

    #[test]
    fn sphinx_pages_keep_notes_code_and_signatures() {
        // The shape Sphinx gives a page: navigation bars around a body of role main.
        let html = "<title>json</title><div class=related role=navigation><ul><li><a href=index.html>Index</a></ul></div>\
            <div class=document><div class=body role=main><section id=json><h1>json</h1><p>Encodes <a href=#x>JSON</a>.</p>\
            <div class=\"admonition note\"><p class=admonition-title>Note</p><p>Keys are strings.</p></div>\
            <div class=\"highlight-python3 notranslate\"><div class=highlight><pre><span></span><span class=gp>&gt;&gt;&gt; </span><span class=kn>import</span> json\n\
            <span class=go>&#39;[1]&#39;</span>\n</pre></div></div>\
            <dl class=\"py function\"><dt class=\"sig sig-object py\" id=json.dumps><span class=sig-name>json.dumps</span><span class=sig-paren>(</span><em class=sig-param>obj</em><span class=sig-paren>)</span></dt>\
            <dd><p>Returns a string.</p><dl class=\"field-list simple\"><dt class=field-odd>Parameters<span class=colon>:</span></dt><dd class=field-odd><p><strong>obj</strong> -- the value.</p></dd></dl></dd></dl>\
            </section></div></div><div class=sphinxsidebar role=navigation><h3>Contents</h3></div><div class=footer>(c)</div>";
        let layout = laid(html);
        assert_eq!(
            kinds(&layout),
            [
                ("title", "json"),
                ("heading", "json"),
                ("paragraph", "Encodes JSON."),
                ("paragraph", "Note"),
                ("paragraph", "Keys are strings."),
                ("code", ">>> import json\n'[1]'\n"),
                ("term", "json.dumps(obj)"),
                ("paragraph", "Returns a string."),
                ("term", "Parameters:"),
                ("paragraph", "obj -- the value."),
            ]
        );
        assert_eq!(
            layout.anchors,
            [
                json!({"name":"json","range":"A2"}),
                json!({"name":"json.dumps","range":"A7"})
            ]
        );
    }

    #[test]
    fn book_and_docs_sites_leave_their_menus_and_pagers_out() {
        // mdBook: the chapter list and the menu bar sit outside <main>.
        let book = "<title>Data Types</title><nav id=sidebar class=sidebar aria-label=\"Table of contents\"><ol class=chapter><li class=chapter-item><a href=ch01.html>1. Start</a></ol></nav>\
            <div id=page-wrapper class=page-wrapper><div class=page><div id=menu-bar class=menu-bar><h1 class=menu-title>The Book</h1></div>\
            <div id=content class=content><main><h1 id=types><a class=header href=#types>Data Types</a></h1>\
            <pre class=playground><code class=\"language-rust edition2021\">let x: u32 = 1;\n</code></pre>\
            <table><thead><tr><th>Length</th><th>Signed</th></tr></thead><tbody><tr><td>8-bit</td><td><code>i8</code></td></tr></tbody></table></main>\
            <nav class=nav-wrapper aria-label=\"Page navigation\"><a rel=prev href=ch02.html>Previous</a></nav></div></div></div>";
        let layout = laid(book);
        assert_eq!(
            kinds(&layout),
            [
                ("title", "Data Types"),
                ("heading", "Data Types"),
                ("code", "let x: u32 = 1;\n"),
                ("table_cell", "Length"),
                ("table_cell", "Signed"),
                ("table_cell", "8-bit"),
                ("table_cell", "i8"),
            ]
        );
        assert_eq!(layout.tables[0]["header_rows"], 1);
        // Docusaurus: breadcrumbs and the pager are <nav>; each package
        // manager's tab is read though only the first is shown.
        let docs = "<title>Install | Site</title><nav aria-label=Main class=navbar><a href=/>Site</a></nav><div class=main-wrapper><aside class=theme-doc-sidebar-container><nav aria-label=\"Docs sidebar\"><a href=/docs>Intro</a></nav></aside>\
            <main class=docMainContainer><nav aria-label=Breadcrumbs><ul><li><a href=/>Home</a></ul></nav><article><div class=\"theme-doc-markdown markdown\"><header><h1>Install</h1></header>\
            <div class=\"tabs-container tabList\"><ul role=tablist class=tabs><li role=tab aria-selected=true>npm</li><li role=tab aria-selected=false>Yarn</li></ul>\
            <div class=margin-top--md><div role=tabpanel class=tabItem><p>Run npm install.</p></div><div role=tabpanel class=tabItem hidden><p>Run yarn.</p></div></div></div></div></article>\
            <nav class=pagination-nav aria-label=\"Docs pages\"><a href=/docs/next>Next</a></nav></main></div><footer class=footer>(c)</footer>";
        assert_eq!(
            kinds(&laid(docs)),
            [
                ("title", "Install | Site"),
                ("heading", "Install"),
                ("list_item", "npm"),
                ("list_item", "Yarn"),
                ("paragraph", "Run npm install."),
                ("paragraph", "Run yarn."),
            ]
        );
    }

    #[test]
    fn docbook_synopses_and_variable_lists_keep_their_kinds() {
        let html = "<title>CREATE TABLE</title><div class=refentry><h2>Synopsis</h2><pre class=synopsis>\nCREATE TABLE <em class=replaceable><code>name</code></em> (\n  ...\n)\n</pre>\
            <div class=variablelist><dl class=variablelist><dt id=temp><span class=term><code class=literal>TEMPORARY</code> or <code class=literal>TEMP</code></span></dt>\
            <dd><p>Makes a temporary table.</p></dd></dl></div>\
            <div class=table id=numeric><p class=title><strong>Table 8.1. Numeric Types</strong></p><div class=table-contents><table class=table summary=\"Numeric Types\" border=1><colgroup><col><col></colgroup>\
            <thead><tr><th>Name</th><th>Size</th></tr></thead><tbody><tr><td><code class=type>smallint</code></td><td>2 bytes</td></tr></tbody></table></div></div></div>";
        let layout = laid(html);
        assert_eq!(
            kinds(&layout),
            [
                ("title", "CREATE TABLE"),
                ("heading", "Synopsis"),
                ("code", "CREATE TABLE name (\n  ...\n)\n"),
                ("term", "TEMPORARY or TEMP"),
                ("paragraph", "Makes a temporary table."),
                ("paragraph", "Table 8.1. Numeric Types"),
                ("table_cell", "Name"),
                ("table_cell", "Size"),
                ("table_cell", "smallint"),
                ("table_cell", "2 bytes"),
            ]
        );
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A7:B8","header_rows":1,"totals_rows":0})]
        );
    }

    #[test]
    fn wiki_tables_keep_row_spans_and_infobox_captions() {
        // A MediaWiki infobox of row headers, then a wikitable whose cells
        // span the versions of one maker and of one standard.
        let html = "<title>Markup</title><table class=\"infobox bordered\"><caption>Markup</caption><tbody>\
            <tr><th scope=row>Extension</th><td>.mk</td></tr><tr><th scope=row>Type</th><td>language<br>format</td></tr></tbody></table>\
            <table class=\"wikitable\"><caption>Versions</caption><tbody><tr><th>Name</th><th>Maker</th><th>Issued</th></tr>\
            <tr><td rowspan=2>2.0</td><td rowspan=3>IETF</td><td>1995</td></tr><tr><td>1996</td></tr><tr><td>3.0</td><td>1997</td></tr></tbody></table>";
        let layout = laid(html);
        assert_eq!(
            texts(html)[1..6],
            [
                ("A2", "Markup"),
                ("A3", "Extension"),
                ("B3", ".mk"),
                ("A4", "Type"),
                ("B4", "language\nformat"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(layout.merges, ["A7:A8", "B7:B9"]);
        assert_eq!(
            layout.tables,
            [
                json!({"name":"table-1","range":"A3:B4","header_rows":0,"totals_rows":0}),
                json!({"name":"table-2","range":"A6:C9","header_rows":1,"totals_rows":0}),
            ]
        );
    }

    #[test]
    fn layout_tables_of_old_pages_are_frames_around_their_data_tables() {
        // A government page of the 2000s: the page is a table of a menu
        // column and a body column, and the body holds data tables.
        let html = "<HTML><HEAD><META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=Shift_JIS\"><TITLE>報道発表</TITLE></HEAD>\r\n\
            <BODY><TABLE WIDTH=\"100%\" BORDER=0><TR><TD WIDTH=150><A HREF=\"../index.html\">トップ</A></TD>\r\n\
            <TD><P ALIGN=RIGHT>平成２０年１２月２６日</P><CENTER><FONT SIZE=4><B>集計結果</B></FONT></CENTER>\r\n\
            <TABLE BORDER=1><TR><TD ROWSPAN=2><B>年度</B></TD><TD COLSPAN=2><B>有料</B></TD></TR>\r\n\
            <TR><TD><B>人日</B></TD><TD><B>対前年度<BR>増減率</B></TD></TR>\r\n\
            <TR><TD>平成１９年度</TD><TD ALIGN=RIGHT>18,866,857</TD><TD ALIGN=RIGHT>△&nbsp;1.3</TD></TR></TABLE>\r\n\
            <P>（注）&nbsp;速報値</P></TD></TR></TABLE></BODY></HTML>";
        let layout = laid(html);
        assert_eq!(
            texts(html),
            [
                ("A1", "報道発表"),
                ("A2", "トップ"),
                ("A3", "平成２０年１２月２６日"),
                ("A4", "集計結果"),
                ("A5", "年度"),
                ("B5", "有料"),
                ("B6", "人日"),
                ("C6", "対前年度\n増減率"),
                ("A7", "平成１９年度"),
                ("B7", "18,866,857"),
                ("C7", "△\u{a0}1.3"),
                ("A8", "（注）\u{a0}速報値"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(layout.merges, ["A5:A6", "B5:C5"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A5:C7","header_rows":0,"totals_rows":0})]
        );
    }

    #[test]
    fn line_breaks_and_spaces_alone_show_no_line() {
        let html = "<p>a<br></p><p><br></p><p>&nbsp;</p><p>b<br><br> </p><table><tr><td>&nbsp;<td>c<br></table><p>\u{3000}</p>";
        assert_eq!(
            texts(html),
            [("A1", "a"), ("A2", "b\n"), ("B3", "c")].map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(
            laid(html).tables,
            [json!({"name":"table-1","range":"A3:B3","header_rows":0,"totals_rows":0})]
        );
        // The line break dropped stays in the original.
        assert_eq!(
            edited(html, 0, "a2").unwrap(),
            html.replacen("a<br>", "a2<br>", 1)
        );
    }

    #[test]
    fn blocks_inside_pre_are_its_lines() {
        // A highlighter's code: a block element and a line break for each line.
        let html = "<pre class=prism-code><code><div class=token-line><span class=plain>my-site</span><br></div>\
            <div class=token-line><span class=plain>\u{251c}\u{2500}\u{2500} docs</span><br></div></code></pre><pre>a<div>b</div>c</pre>";
        assert_eq!(
            kinds(&laid(html)),
            [
                ("code", "my-site\n\u{251c}\u{2500}\u{2500} docs"),
                ("code", "a\nb\nc")
            ]
        );
        assert_eq!(
            edited(html, 0, "my-web\n\u{251c}\u{2500}\u{2500} docs").unwrap(),
            html.replace(">my-site<", ">my-web<")
        );
        // The line break between blocks is in no element to edit.
        assert!(edited(html, 1, "abc").is_err());
    }

    #[test]
    fn text_in_a_table_outside_its_cells_comes_before_it() {
        let html = "<table>stray<tr><td>x</td>loose</tr><tbody><p>para</p><tr><td>y</table>";
        assert_eq!(
            texts(html),
            [
                ("A1", "strayloose"),
                ("A2", "para"),
                ("A3", "x"),
                ("A4", "y")
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(
            laid(html).tables,
            [json!({"name":"table-1","range":"A3:A4","header_rows":0,"totals_rows":0})]
        );
    }

    #[test]
    fn footers_are_totals_rows_shown_last() {
        // HTML 4 put tfoot before tbody; browsers show it last, and a cell
        // spans no rows past its row group.
        let html = "<table><thead><tr><th>k<th>v</thead><tfoot><tr><td>sum<td>3</tfoot>\
            <tbody><tr><td rowspan=5>a<td>1<tr><td>2</tbody></table>";
        let layout = laid(html);
        assert_eq!(
            texts(html),
            [
                ("A1", "k"),
                ("B1", "v"),
                ("A2", "a"),
                ("B2", "1"),
                ("B3", "2"),
                ("A4", "sum"),
                ("B4", "3"),
            ]
            .map(|(a, t)| (a.to_owned(), t.to_owned()))
        );
        assert_eq!(layout.merges, ["A2:A3"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A1:B4","header_rows":1,"totals_rows":1})]
        );
        // A row inserted above the footer goes into the body.
        let html = "<table><tfoot><tr><td>sum<td>3</tfoot><tbody><tr><td>a<td>1</tbody></table>";
        let values = InsertedText::from([
            (("op".to_owned(), 0, "A".to_owned()), "b".to_owned()),
            (("op".to_owned(), 0, "B".to_owned()), "2".to_owned()),
        ]);
        let insert = operation(OperationKind::InsertRows, 2, 1);
        let out = patch(html.as_bytes(), &[&insert], &BTreeMap::new(), &values).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            html.replace(
                "<td>1</tbody>",
                "<td>1<tr><td>b</td><td>2</td></tr></tbody>"
            )
        );
        // A table of a header and a footer alone has no totals rows.
        let html = "<table><thead><tr><th>k</thead><tfoot><tr><td>sum</tfoot></table>";
        assert_eq!(laid(html).tables[0]["totals_rows"], 0);
    }

    #[test]
    fn tables_keep_rows_columns_spans_and_headers() {
        let html = "<p>intro</p><table><caption>Options</caption><thead><tr><th>Name<th>Default<th>Note</thead>\
            <tbody><tr><td rowspan=2>a<td>1<td>x<tr><td colspan=2>wide<tr><td>c<td><td><p>p1</p><p>p2</p></table>";
        let layout = laid(html);
        let cells: Vec<_> = layout
            .blocks
            .iter()
            .map(|b| (b.address.as_str(), b.text.as_str()))
            .collect();
        assert_eq!(
            cells,
            [
                ("A1", "intro"),
                ("A2", "Options"),
                ("A3", "Name"),
                ("B3", "Default"),
                ("C3", "Note"),
                ("A4", "a"),
                ("B4", "1"),
                ("C4", "x"),
                ("B5", "wide"),
                ("A6", "c"),
                ("C6", "p1\np2"),
            ]
        );
        assert_eq!(layout.merges, ["A4:A5", "B5:C5"]);
        assert_eq!(
            layout.tables,
            [json!({"name":"table-1","range":"A3:C6","header_rows":1,"totals_rows":0})]
        );
    }

    #[test]
    fn tables_holding_tables_are_frames() {
        let html =
            "<table><tr><td><p>side</p><td><table><tr><th>k<th>v<tr><td>1<td>2</table></table>";
        let layout = laid(html);
        let cells: Vec<_> = layout
            .blocks
            .iter()
            .map(|b| (b.address.as_str(), b.text.as_str()))
            .collect();
        assert_eq!(
            cells,
            [
                ("A1", "side"),
                ("A2", "k"),
                ("B2", "v"),
                ("A3", "1"),
                ("B3", "2")
            ]
        );
        assert_eq!(layout.tables.len(), 1);
    }

    #[test]
    fn hidden_and_script_content_is_left_out() {
        let html = "<head><style>p{}</style><script>x</script></head><body><p hidden>no</p><template><p>t</p></template><p>yes<svg><text>s</text></svg></p></body>";
        assert_eq!(texts(html), [("A1".to_owned(), "yes".to_owned())]);
    }

    #[test]
    fn edits_replace_only_the_characters_they_change() {
        let html = "<p>Run  the\n  <b>setup</b> tool &amp; wait.</p>";
        assert_eq!(
            edited(html, 0, "Run the setup tool & go.").unwrap(),
            "<p>Run  the\n  <b>setup</b> tool &amp; go.</p>"
        );
        assert_eq!(
            edited(html, 0, "Run the install tool & wait.").unwrap(),
            "<p>Run  the\n  <b>install</b> tool &amp; wait.</p>"
        );
        // Text across elements goes to the first.
        assert_eq!(
            edited(html, 0, "Run X & wait.").unwrap(),
            "<p>Run  X<b></b> &amp; wait.</p>"
        );
        assert_eq!(
            edited(html, 0, "Run the setup tool <now> & wait.").unwrap(),
            "<p>Run  the\n  <b>setup</b> tool &lt;now&gt; &amp; wait.</p>"
        );
        assert_eq!(
            edited(html, 0, "Run the setup tool\n& wait.").unwrap(),
            "<p>Run  the\n  <b>setup</b> tool<br>&amp; wait.</p>"
        );
    }

    #[test]
    fn edits_around_line_breaks_keep_dropped_white_space_out() {
        let html = "<p>a<br>\n  b</p>";
        assert_eq!(edited(html, 0, "a\nXb").unwrap(), "<p>a<br>\n  Xb</p>");
        assert_eq!(edited(html, 0, "aXb").unwrap(), "<p>aXb</p>");
        assert_eq!(edited(html, 0, "Za\nb").unwrap(), "<p>Za<br>\n  b</p>");
    }

    #[test]
    fn edits_html_would_show_otherwise_are_refused() {
        let html =
            "<title>T</title><p>a b</p><p>注意注意事項</p><table><tr><td><p>x</p><p>y</p></table>";
        assert!(edited(html, 0, "T\nU").is_err());
        assert!(edited(html, 1, "a  b").is_err());
        assert!(edited(html, 1, "a b ").is_err());
        assert!(edited(html, 1, "a\tb").is_err());
        assert!(edited(html, 1, "a\u{a0}\u{a0}b").is_ok());
        // Deleting either 注意 writes the same text.
        assert_eq!(
            edited(html, 2, "注意事項").unwrap(),
            html.replace("注意注意事項", "注意事項")
        );
        assert!(edited(html, 3, "xy").is_err());
        assert!(edited(html, 3, "x2\ny").is_ok());
    }

    #[test]
    fn rows_are_inserted_and_deleted_as_elements() {
        let html = "<h1>T</h1><ul><li id=a class=k>one</li><li>two</li></ul><table><tr><th>k<th>v</tr><tr><td>1<td>2</tr></table><p>end</p>";
        let insert = StructuralOperation {
            style_from: None,
            ..operation(OperationKind::InsertRows, 3, 1)
        };
        let values = InsertedText::from([(
            ("op".to_owned(), 0, "A".to_owned()),
            "one and a half".to_owned(),
        )]);
        let out = patch(html.as_bytes(), &[&insert], &BTreeMap::new(), &values).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            html.replace("one</li>", "one</li><li class=k>one and a half</li>")
        );
        let row = operation(OperationKind::InsertRows, 6, 1);
        let values = InsertedText::from([
            (("op".to_owned(), 0, "A".to_owned()), "3".to_owned()),
            (("op".to_owned(), 0, "B".to_owned()), "4 & 5".to_owned()),
        ]);
        let out = patch(html.as_bytes(), &[&row], &BTreeMap::new(), &values).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            html.replace(
                "<td>2</tr>",
                "<td>2</tr><tr><td>3</td><td>4 &amp; 5</td></tr>"
            )
        );
        // Deleting every item deletes the list; every row the table.
        let delete = operation(OperationKind::DeleteRows, 2, 2);
        let out = patch(
            html.as_bytes(),
            &[&delete],
            &BTreeMap::new(),
            &InsertedText::new(),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "<h1>T</h1><table><tr><th>k<th>v</tr><tr><td>1<td>2</tr></table><p>end</p>"
        );
        let delete = operation(OperationKind::DeleteRows, 4, 2);
        let out = patch(
            html.as_bytes(),
            &[&delete],
            &BTreeMap::new(),
            &InsertedText::new(),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "<h1>T</h1><ul><li id=a class=k>one</li><li>two</li></ul><p>end</p>"
        );
    }

    #[test]
    fn rows_html_cannot_copy_or_delete_are_refused() {
        let html =
            "<title>T</title><table><tr><td rowspan=2>a<td>b<tr><td>c</table><div>x<p>y</p></div>";
        let refused = |kind, at| {
            ensure_rows(html.as_bytes(), &[&operation(kind, at, 1)])
                .unwrap_err()
                .to_string()
        };
        assert!(refused(OperationKind::DeleteRows, 1).contains("document title"));
        assert!(refused(OperationKind::DeleteRows, 2).contains("spanning rows"));
        assert!(refused(OperationKind::InsertRows, 4).contains("cannot be copied"));
        assert!(
            ensure_rows(
                html.as_bytes(),
                &[&operation(OperationKind::DeleteRows, 5, 1)]
            )
            .is_ok()
        );
    }

    #[test]
    fn shift_jis_originals_keep_their_bytes() {
        // 0xED40 is an NEC-selected IBM extension that encodes back as 0xFA5C.
        let mut raw = encoding_rs::SHIFT_JIS
            .encode("<meta charset=\"Shift_JIS\"><p>機能</p><p>")
            .0
            .into_owned();
        raw.extend_from_slice(&[0xED, 0x40]);
        raw.extend_from_slice(&encoding_rs::SHIFT_JIS.encode("説明</p>").0);
        let changes = BTreeMap::from([(0, "新機能".to_owned())]);
        let out = patch(&raw, &[], &changes, &InsertedText::new()).unwrap();
        let expected = [
            &encoding_rs::SHIFT_JIS
                .encode("<meta charset=\"Shift_JIS\"><p>新機能</p><p>")
                .0[..],
            &[0xED, 0x40],
            &encoding_rs::SHIFT_JIS.encode("説明</p>").0[..],
        ]
        .concat();
        assert_eq!(out, expected);
        let changes = BTreeMap::from([(0, "機能😀".to_owned())]);
        assert!(patch(&raw, &[], &changes, &InsertedText::new()).is_err());
    }
}
