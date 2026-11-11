//! HTML read with the place of every node in the text. A tokenizer and a tree
//! builder keep the byte range of each tag and text, so that writeback
//! replaces only the text an edit changes and leaves every other byte of the
//! original as it is. The tree follows the parsing rules that shape documents
//! (void elements, raw text, implied end tags, the head and table scopes),
//! not every error recovery of a browser; writers read their output back.
//!
//! The text is the original decoded from the encoding its byte order mark or
//! `<meta charset>` names. Offsets in the tree are in that decoded text;
//! [`raw_offsets`] gives the places in the original bytes.
use anyhow::{Context, Result, bail, ensure};
use encoding_rs::{EncoderResult, Encoding, ISO_2022_JP, UTF_8, UTF_16BE, UTF_16LE};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// An original decoded to text.
pub(super) struct Decoded {
    pub text: String,
    pub encoding: &'static Encoding,
    /// The length of the byte order mark the original starts with.
    pub bom: usize,
}

/// The text of `raw` in the encoding its byte order mark names, else the one
/// its `<meta>` declares within the first 1024 bytes, else UTF-8. A
/// character the encoding does not define is refused rather than replaced.
pub(super) fn decode(raw: &[u8]) -> Result<Decoded> {
    let (encoding, bom, declared) = match Encoding::for_bom(raw) {
        Some((encoding, length)) => (encoding, length, true),
        None => match prescan(&raw[..raw.len().min(1024)]) {
            Some(encoding) => (encoding, 0, true),
            None => (UTF_8, 0, false),
        },
    };
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(&raw[bom..])
        .with_context(|| {
            if declared {
                format!(
                    "HTML is not valid {} as the original declares; save it in the encoding its <meta charset> names and import it again",
                    encoding.name()
                )
            } else {
                "HTML is not valid UTF-8 and declares no other encoding; add <meta charset=\"...\"> naming the encoding it is written in (such as Shift_JIS) and import it again".to_owned()
            }
        })?
        .into_owned();
    ensure!(
        !text.contains('\0'),
        "HTML contains NUL characters; binary files are not supported"
    );
    Ok(Decoded {
        text,
        encoding,
        bom,
    })
}

/// The encoding the first `<meta>` with `charset`, or `http-equiv` of
/// `content-type` and a charset in `content`, names in `head`, as browsers
/// look for it before they decode. UTF-16 labels stand for UTF-8 there, since
/// the bytes read as ASCII.
fn prescan(head: &[u8]) -> Option<&'static Encoding> {
    let text = String::from_utf8_lossy(head);
    let mut at = 0;
    while let Some(found) = text[at..].find('<') {
        let start = at + found;
        match token(&text, start, false) {
            Some((Token::Start(tag), end)) => {
                if tag.name == "meta"
                    && let Some(encoding) = meta_charset(&tag)
                {
                    return Some(encoding);
                }
                at = end;
            }
            Some((_, end)) => at = end,
            None => at = start + 1,
        }
    }
    None
}

fn meta_charset(tag: &Tag) -> Option<&'static Encoding> {
    let label = match tag.attribute("charset") {
        Some(label) => label.to_owned(),
        None => {
            if !tag
                .attribute("http-equiv")?
                .eq_ignore_ascii_case("content-type")
            {
                return None;
            }
            content_charset(tag.attribute("content")?)?
        }
    };
    let encoding = Encoding::for_label(label.trim().as_bytes())?;
    Some(if encoding == UTF_16LE || encoding == UTF_16BE {
        UTF_8
    } else if encoding == encoding_rs::X_USER_DEFINED {
        encoding_rs::WINDOWS_1252
    } else {
        encoding
    })
}

/// The charset `content="text/html; charset=..."` names.
fn content_charset(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let mut from = 0;
    while let Some(found) = lower[from..].find("charset") {
        let mut rest = lower[from + found + 7..].trim_start();
        from += found + 7;
        let Some(after) = rest.strip_prefix('=') else {
            continue;
        };
        rest = after.trim_start();
        let start = content.len() - rest.len();
        return match rest.chars().next()? {
            quote @ ('"' | '\'') => {
                let end = rest[1..].find(quote)?;
                Some(content[start + 1..start + 1 + end].to_owned())
            }
            _ => {
                let end = rest
                    .find(|c: char| c.is_ascii_whitespace() || c == ';')
                    .unwrap_or(rest.len());
                Some(content[start..start + end].to_owned())
            }
        };
    }
    None
}

/// Refuses writing an original in `encoding`: UTF-16 has no encoder, and
/// ISO-2022-JP shifts between character sets, so a piece written in the
/// middle could change how the bytes after it read.
pub(super) fn ensure_writable(encoding: &'static Encoding) -> Result<()> {
    ensure!(
        encoding != UTF_16LE
            && encoding != UTF_16BE
            && encoding != ISO_2022_JP
            && encoding.output_encoding() == encoding,
        "HTML in {} is read but not written back; save the original as UTF-8 or Shift_JIS and import it again",
        encoding.name()
    );
    Ok(())
}

/// `text` in `encoding`. A character the encoding cannot hold is refused.
pub(super) fn encode(text: &str, encoding: &'static Encoding) -> Result<Vec<u8>> {
    if encoding == UTF_8 {
        return Ok(text.as_bytes().to_vec());
    }
    let mut encoder = encoding.new_encoder();
    let mut out = Vec::with_capacity(text.len() + 16);
    let mut rest = text;
    loop {
        let (result, read) =
            encoder.encode_from_utf8_to_vec_without_replacement(rest, &mut out, true);
        rest = &rest[read..];
        match result {
            EncoderResult::InputEmpty => return Ok(out),
            EncoderResult::OutputFull => out.reserve(rest.len() * 2 + 16),
            EncoderResult::Unmappable(c) => bail!(
                "{c} (U+{:04X}) cannot be written in {}, the encoding of the original",
                c as u32,
                encoding.name()
            ),
        }
    }
}

/// The offset in the original bytes `raw` of each of the `wanted` offsets of
/// its decoded text, which must fall between characters.
pub(super) fn raw_offsets(
    raw: &[u8],
    decoded: &Decoded,
    wanted: &BTreeSet<usize>,
) -> Result<BTreeMap<usize, usize>> {
    if decoded.encoding == UTF_8 {
        return Ok(wanted.iter().map(|&at| (at, at + decoded.bom)).collect());
    }
    ensure_writable(decoded.encoding)?;
    let body = &raw[decoded.bom..];
    let mut decoder = decoded.encoding.new_decoder_without_bom_handling();
    let mut offsets = BTreeMap::new();
    let mut wanted = wanted.iter().copied().peekable();
    let (mut read, mut written) = (0usize, 0usize);
    // Bytes of a character the decoder holds, which it gives when complete.
    let mut pending = false;
    let mut buffer = [0u8; 32];
    loop {
        while wanted.next_if_eq(&written).is_some() {
            offsets.insert(written, read + decoded.bom);
        }
        let Some(&next) = wanted.peek() else {
            return Ok(offsets);
        };
        ensure!(
            next > written && read < body.len(),
            "offset {next} is not between characters of the decoded HTML"
        );
        // An ASCII byte is a character of its own in the encodings written
        // back, unless it ends one the decoder holds.
        if !pending && body[read] < 0x80 {
            read += 1;
            written += 1;
            continue;
        }
        let (_, used, out) =
            decoder.decode_to_utf8_without_replacement(&body[read..read + 1], &mut buffer, false);
        read += used;
        written += out;
        pending = out == 0;
    }
}

/// A node of the tree, with the range it takes in the text: an element from
/// the start of its start tag to the end of its end tag, or to where another
/// tag closes it.
///
/// Nodes are numbered in document order, so that a node's descendants are
/// the nodes after it up to its next sibling or an ancestor's. A node links
/// to its relatives by number ([`NONE`] for none): a page of a million tags
/// held a list of children for each, at several times the memory.
pub(super) struct Node {
    parent: u32,
    first_child: u32,
    last_child: u32,
    next_sibling: u32,
    pub kind: Kind,
    pub range: Range<usize>,
}

/// The number of no node.
const NONE: u32 = u32::MAX;

/// The deepest an element is opened, as browsers limit it: a tag opened
/// deeper is closed at once, and what it holds goes to its parent.
const MAX_DEPTH: usize = 500;

fn link(node: u32) -> Option<usize> {
    (node != NONE).then_some(node as usize)
}

pub(super) enum Kind {
    Document,
    /// An element, boxed so that text nodes do not take its size.
    Element(Box<Element>),
    /// Text, in which `raw` text (of script and style) has no character references.
    Text {
        raw: bool,
    },
    /// A comment, doctype or processing instruction.
    Other,
}

pub(super) struct Element {
    /// The name in lower case, shared for the names HTML defines.
    pub name: Cow<'static, str>,
    pub start_tag: Range<usize>,
    pub end_tag: Option<Range<usize>>,
    pub attributes: Vec<(String, String)>,
    /// Inside `svg` or `math`, whose elements follow XML's rules.
    pub foreign: bool,
}

impl Element {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub(super) struct Tree {
    pub nodes: Vec<Node>,
}

impl Tree {
    pub fn element(&self, node: usize) -> Option<&Element> {
        match &self.nodes[node].kind {
            Kind::Element(element) => Some(element),
            _ => None,
        }
    }

    /// The name of element `node`, or "" for other nodes.
    pub fn name(&self, node: usize) -> &str {
        self.element(node).map_or("", |e| &e.name)
    }

    pub fn parent(&self, node: usize) -> Option<usize> {
        link(self.nodes[node].parent)
    }

    pub fn children(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(link(self.nodes[node].first_child), |n| {
            link(self.nodes[*n].next_sibling)
        })
    }

    pub fn ancestors(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.parent(node), |n| self.parent(*n))
    }

    /// The descendants of `node`, in document order.
    pub fn descendants(&self, node: usize) -> Range<usize> {
        let end = std::iter::once(node)
            .chain(self.ancestors(node))
            .find_map(|n| link(self.nodes[n].next_sibling))
            .unwrap_or(self.nodes.len());
        node + 1..end
    }
}

/// `name` shared with the names HTML defines, which most tags have.
fn interned(name: String) -> Cow<'static, str> {
    static NAMES: std::sync::LazyLock<BTreeSet<&'static str>> = std::sync::LazyLock::new(|| {
        [
            VOID,
            RAW_TEXT,
            ESCAPABLE_RAW_TEXT,
            CLOSES_P,
            SPECIAL,
            SCOPE,
            TABLE_PARTS,
            HEAD_CONTENT,
            &[
                "a", "abbr", "acronym", "audio", "b", "bdi", "bdo", "big", "canvas", "cite",
                "code", "data", "datalist", "del", "dfn", "em", "font", "i", "ins", "kbd", "label",
                "map", "mark", "math", "meter", "nobr", "optgroup", "option", "output", "picture",
                "progress", "q", "rp", "rt", "ruby", "s", "samp", "small", "span", "strike",
                "strong", "sub", "sup", "svg", "time", "tt", "u", "var", "video",
            ],
        ]
        .concat()
        .into_iter()
        .collect()
    });
    match NAMES.get(name.as_str()) {
        Some(known) => Cow::Borrowed(known),
        None => Cow::Owned(name),
    }
}

pub(super) const VOID: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];
const RAW_TEXT: &[&str] = &[
    "script", "style", "xmp", "iframe", "noembed", "noframes", "noscript",
];
const ESCAPABLE_RAW_TEXT: &[&str] = &["title", "textarea"];
/// Elements whose start closes an open `p`.
const CLOSES_P: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dd",
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
    "hr",
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
    "ul",
    "xmp",
];
/// The special elements of the HTML parser, which stop the search for an
/// open list item to close.
const SPECIAL: &[&str] = &[
    "address",
    "applet",
    "area",
    "article",
    "aside",
    "base",
    "basefont",
    "bgsound",
    "blockquote",
    "body",
    "br",
    "button",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dir",
    "div",
    "dl",
    "dt",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "iframe",
    "img",
    "input",
    "keygen",
    "li",
    "link",
    "listing",
    "main",
    "marquee",
    "menu",
    "meta",
    "nav",
    "noembed",
    "noframes",
    "noscript",
    "object",
    "ol",
    "p",
    "param",
    "plaintext",
    "pre",
    "script",
    "search",
    "section",
    "select",
    "source",
    "style",
    "summary",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
    "wbr",
    "xmp",
];
const SCOPE: &[&str] = &[
    "html", "table", "td", "th", "caption", "template", "applet", "marquee", "object",
];
const TABLE_PARTS: &[&str] = &[
    "tr", "td", "th", "thead", "tbody", "tfoot", "caption", "colgroup",
];
/// The elements a head holds; another start tag ends a head left open.
const HEAD_CONTENT: &[&str] = &[
    "base", "basefont", "bgsound", "link", "meta", "noframes", "script", "style", "template",
    "title", "noscript",
];

pub(super) struct Tag {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub self_closing: bool,
}

impl Tag {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

enum Token {
    Start(Tag),
    End(String),
    /// A comment, doctype, processing instruction or `</>`.
    Other,
}

fn space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// The token starting with the `<` at `start`, and where it ends, or None
/// when the `<` starts no tag and is text. `foreign` reads CDATA sections.
fn token(text: &str, start: usize, foreign: bool) -> Option<(Token, usize)> {
    let bytes = text.as_bytes();
    let rest = &text[start..];
    let after = |pattern: &str, from: usize| {
        text[from..]
            .find(pattern)
            .map_or(text.len(), |i| from + i + pattern.len())
    };
    if let Some(comment) = rest.strip_prefix("<!--") {
        // `<!-->` and `<!--->` are comments that end at once.
        let end = if comment.starts_with('>') {
            start + 5
        } else if comment.starts_with("->") {
            start + 6
        } else {
            after("-->", start + 4)
        };
        return Some((Token::Other, end));
    }
    if foreign && rest.starts_with("<![CDATA[") {
        return Some((Token::Other, after("]]>", start + 9)));
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        return Some((Token::Other, after(">", start + 2)));
    }
    let end_tag = rest.starts_with("</");
    let name_start = start + if end_tag { 2 } else { 1 };
    match bytes.get(name_start) {
        Some(c) if c.is_ascii_alphabetic() => {}
        Some(b'>') if end_tag => return Some((Token::Other, name_start + 1)),
        Some(_) if end_tag => return Some((Token::Other, after(">", name_start))),
        _ => return None,
    }
    let mut at = name_start;
    while at < bytes.len() && !space(bytes[at]) && bytes[at] != b'/' && bytes[at] != b'>' {
        at += 1;
    }
    let name = text[name_start..at].to_ascii_lowercase();
    let (attributes, self_closing, end) = attributes(text, at)?;
    Some((
        if end_tag {
            Token::End(name)
        } else {
            Token::Start(Tag {
                name,
                attributes,
                self_closing,
            })
        },
        end,
    ))
}

type Attributes = (Vec<(String, String)>, bool, usize);

/// The attributes of a tag from `at` to its `>`, whether it ends with `/>`,
/// and where it ends. None when the text ends inside the tag, which then
/// is no tag.
fn attributes(text: &str, mut at: usize) -> Option<Attributes> {
    let bytes = text.as_bytes();
    let mut attributes: Vec<(String, String)> = vec![];
    let mut self_closing = false;
    loop {
        while at < bytes.len() && space(bytes[at]) {
            at += 1;
        }
        match bytes.get(at)? {
            b'>' => return Some((attributes, self_closing, at + 1)),
            b'/' => {
                at += 1;
                self_closing = bytes.get(at) == Some(&b'>');
                continue;
            }
            _ => self_closing = false,
        }
        let start = at;
        at += text[at..].chars().next()?.len_utf8();
        while at < bytes.len() && !space(bytes[at]) && !matches!(bytes[at], b'/' | b'>' | b'=') {
            at += 1;
        }
        let name = text[start..at].to_ascii_lowercase();
        while at < bytes.len() && space(bytes[at]) {
            at += 1;
        }
        let mut value = String::new();
        if bytes.get(at) == Some(&b'=') {
            at += 1;
            while at < bytes.len() && space(bytes[at]) {
                at += 1;
            }
            let (from, to, next) = match bytes.get(at)? {
                quote @ (b'"' | b'\'') => {
                    let close = text[at + 1..].find(*quote as char)? + at + 1;
                    (at + 1, close, close + 1)
                }
                _ => {
                    let mut end = at;
                    while end < bytes.len() && !space(bytes[end]) && bytes[end] != b'>' {
                        end += 1;
                    }
                    (at, end, end)
                }
            };
            value = unescape(&text[from..to], true);
            at = next;
        }
        if !attributes.iter().any(|(n, _)| *n == name) {
            attributes.push((name, value));
        }
    }
}

/// `text` with its character references replaced, as in an attribute value
/// (`attribute`) or in text.
pub(super) fn unescape(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = text[at..].find('&') {
        out.push_str(&text[at..at + found]);
        at += found;
        match reference(text, at, attribute) {
            Some((chars, end)) => {
                out.push_str(&chars);
                at = end;
            }
            None => {
                out.push('&');
                at += 1;
            }
        }
    }
    out.push_str(&text[at..]);
    out
}

/// The characters of the character reference at `at` (an `&`) and where it
/// ends, or None when the `&` starts none and is text. In an `attribute`, a
/// named reference without `;` followed by `=` or an alphanumeric is text.
pub(super) fn reference(text: &str, at: usize, attribute: bool) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(at + 1) == Some(&b'#') {
        let hex = matches!(bytes.get(at + 2), Some(b'x' | b'X'));
        let digits = at + if hex { 3 } else { 2 };
        let mut end = digits;
        while end < bytes.len()
            && if hex {
                bytes[end].is_ascii_hexdigit()
            } else {
                bytes[end].is_ascii_digit()
            }
        {
            end += 1;
        }
        if end == digits {
            return None;
        }
        let value =
            u32::from_str_radix(&text[digits..end], if hex { 16 } else { 10 }).unwrap_or(u32::MAX);
        if bytes.get(end) == Some(&b';') {
            end += 1;
        }
        return Some((numeric(value).to_string(), end));
    }
    let longest = (bytes.len() - at).min(htmlize::ENTITY_MAX_LENGTH);
    for length in (htmlize::ENTITY_MIN_LENGTH..=longest).rev() {
        let key = &bytes[at..at + length];
        if let Some(glyph) = htmlize::ENTITIES.get(key) {
            let end = at + length;
            if attribute
                && key.last() != Some(&b';')
                && bytes
                    .get(end)
                    .is_some_and(|c| *c == b'=' || c.is_ascii_alphanumeric())
            {
                return None;
            }
            return Some((String::from_utf8_lossy(glyph).into_owned(), end));
        }
    }
    None
}

/// The character a numeric reference to `value` stands for, as browsers
/// read it: C1 controls name the windows-1252 characters.
fn numeric(value: u32) -> char {
    const C1: [u32; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013,
        0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    match value {
        0 => '\u{FFFD}',
        0x80..=0x9F => char::from_u32(C1[(value - 0x80) as usize]).unwrap_or('\u{FFFD}'),
        value => char::from_u32(value).unwrap_or('\u{FFFD}'),
    }
}

struct Builder<'a> {
    text: &'a str,
    nodes: Vec<Node>,
    /// The open elements, the document first.
    stack: Vec<usize>,
}

impl Builder<'_> {
    fn current(&self) -> usize {
        *self.stack.last().unwrap()
    }

    fn name(&self, node: usize) -> &str {
        match &self.nodes[node].kind {
            Kind::Element(element) => &element.name,
            _ => "",
        }
    }

    fn foreign(&self) -> bool {
        matches!(&self.nodes[self.current()].kind, Kind::Element(e) if e.foreign || e.name == "svg" || e.name == "math")
    }

    fn append(&mut self, kind: Kind, range: Range<usize>) -> usize {
        let parent = self.current();
        let index = self.nodes.len();
        self.nodes.push(Node {
            parent: parent as u32,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            kind,
            range,
        });
        match link(self.nodes[parent].last_child) {
            Some(last) => self.nodes[last].next_sibling = index as u32,
            None => self.nodes[parent].first_child = index as u32,
        }
        self.nodes[parent].last_child = index as u32;
        index
    }

    fn text(&mut self, range: Range<usize>, raw: bool) {
        // Text that is not white space ends a head left open.
        if !raw
            && self.name(self.current()) == "head"
            && !self.text.as_bytes()[range.clone()]
                .iter()
                .all(|c| space(*c))
        {
            self.pop(self.stack.len() - 1, range.start);
        }
        let parent = self.current();
        if let Some(last) = link(self.nodes[parent].last_child)
            && let Kind::Text { raw: last_raw } = self.nodes[last].kind
            && last_raw == raw
            && self.nodes[last].range.end == range.start
        {
            self.nodes[last].range.end = range.end;
            return;
        }
        self.append(Kind::Text { raw }, range);
    }

    /// Closes the open elements from `depth` of the stack up, at `at`.
    fn pop(&mut self, depth: usize, at: usize) {
        for node in self.stack.drain(depth..) {
            self.nodes[node].range.end = at;
        }
    }

    /// The depth of the nearest open element named one of `names`, searching
    /// down from the current node and stopping at any of `boundary`.
    fn open(&self, names: &[&str], boundary: &[&str]) -> Option<usize> {
        for depth in (1..self.stack.len()).rev() {
            let name = self.name(self.stack[depth]);
            if names.contains(&name) {
                return Some(depth);
            }
            if boundary.contains(&name) {
                return None;
            }
        }
        None
    }

    fn close_p(&mut self, at: usize) {
        let boundary: Vec<&str> = SCOPE.iter().copied().chain(["button"]).collect();
        if let Some(depth) = self.open(&["p"], &boundary) {
            self.pop(depth, at);
        }
    }

    /// Closes the open list item named one of `names`, as a new item does.
    fn close_item(&mut self, names: &[&str], at: usize) {
        for depth in (1..self.stack.len()).rev() {
            let name = self.name(self.stack[depth]);
            if names.contains(&name) {
                self.pop(depth, at);
                return;
            }
            if SPECIAL.contains(&name) && !matches!(name, "address" | "div" | "p") {
                return;
            }
        }
    }

    fn start(&mut self, tag: Tag, range: Range<usize>) {
        let at = range.start;
        let foreign = self.foreign();
        if !foreign {
            // A head is opened by the document or its html element.
            if self.stack.iter().take(3).any(|n| self.name(*n) == "head")
                && !HEAD_CONTENT.contains(&tag.name.as_str())
                && let Some(depth) = self.open(&["head"], &[])
            {
                self.pop(depth, at);
            }
            match tag.name.as_str() {
                "li" => {
                    self.close_item(&["li"], at);
                    self.close_p(at);
                }
                "dt" | "dd" => {
                    self.close_item(&["dt", "dd"], at);
                    self.close_p(at);
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    self.close_p(at);
                    if matches!(
                        self.name(self.current()),
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    ) {
                        self.pop(self.stack.len() - 1, at);
                    }
                }
                "tr" => {
                    if let Some(depth) = self.open(&["tr"], &["table", "html", "template"]) {
                        self.pop(depth, at);
                    }
                }
                "td" | "th" => {
                    if let Some(depth) =
                        self.open(&["td", "th"], &["tr", "table", "html", "template"])
                    {
                        self.pop(depth, at);
                    }
                }
                "thead" | "tbody" | "tfoot" | "caption" | "colgroup" => {
                    if let Some(depth) = self.open(&["table"], &["html", "template"]) {
                        self.pop(depth + 1, at);
                    }
                }
                "option" | "optgroup" => {
                    if self.name(self.current()) == "option" {
                        self.pop(self.stack.len() - 1, at);
                    }
                    if tag.name == "optgroup" && self.name(self.current()) == "optgroup" {
                        self.pop(self.stack.len() - 1, at);
                    }
                }
                name if CLOSES_P.contains(&name) => self.close_p(at),
                _ => {}
            }
        }
        let void = if foreign || tag.name == "svg" || tag.name == "math" {
            tag.self_closing
        } else {
            VOID.contains(&tag.name.as_str())
        };
        // Raw text is the element's even past the deepest, so that a script
        // there does not read as text.
        let raw = !foreign && raw_text(&tag.name);
        let index = self.append(
            Kind::Element(Box::new(Element {
                name: interned(tag.name),
                start_tag: range.clone(),
                end_tag: None,
                attributes: tag.attributes,
                foreign,
            })),
            range.clone(),
        );
        if !void && (self.stack.len() < MAX_DEPTH || raw) {
            self.stack.push(index);
        }
    }

    fn end(&mut self, name: &str, range: Range<usize>) {
        let found = if self.foreign() {
            // Foreign elements close by name up to the svg or math they are in.
            self.open(&[name], &[])
        } else {
            match name {
                "p" => self.open(&["p"], &[SCOPE, &["button"]].concat()),
                "br" | "body" | "html" => None,
                "li" => self.open(&["li"], &[SCOPE, &["ol", "ul"]].concat()),
                "table" => self.open(&["table"], &["html", "template"]),
                name if TABLE_PARTS.contains(&name) => {
                    self.open(&[name], &["table", "html", "template"])
                }
                name => self.open(&[name], SCOPE),
            }
        };
        let Some(depth) = found else {
            self.append(Kind::Other, range);
            return;
        };
        let node = self.stack[depth];
        self.pop(depth + 1, range.start);
        self.stack.truncate(depth);
        self.nodes[node].range.end = range.end;
        if let Kind::Element(element) = &mut self.nodes[node].kind {
            element.end_tag = Some(range);
        }
    }
}

/// The tree of `text`.
pub(super) fn parse(text: &str) -> Tree {
    let mut builder = Builder {
        text,
        nodes: vec![Node {
            parent: NONE,
            first_child: NONE,
            last_child: NONE,
            next_sibling: NONE,
            kind: Kind::Document,
            range: 0..text.len(),
        }],
        stack: vec![0],
    };
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'<'
            && let Some((token, end)) = token(text, at, builder.foreign())
        {
            match token {
                Token::Start(tag) => {
                    let raw = !builder.foreign() && raw_text(&tag.name);
                    let name = tag.name.clone();
                    builder.start(tag, at..end);
                    if raw {
                        let close = if name == "plaintext" {
                            text.len()
                        } else {
                            closing(text, end, &name)
                        };
                        if close > end {
                            builder.text(end..close, RAW_TEXT.contains(&name.as_str()));
                        }
                        at = close;
                        continue;
                    }
                }
                Token::End(name) => builder.end(&name, at..end),
                Token::Other => {
                    builder.append(Kind::Other, at..end);
                }
            }
            at = end;
            continue;
        }
        let from = at + text[at..].chars().next().map_or(1, char::len_utf8);
        let next = text[from..].find('<').map_or(text.len(), |i| from + i);
        builder.text(at..next, false);
        at = next;
    }
    builder.pop(1, text.len());
    Tree {
        nodes: builder.nodes,
    }
}

/// Whether the element `name` holds raw text, no tags.
fn raw_text(name: &str) -> bool {
    RAW_TEXT.contains(&name) || ESCAPABLE_RAW_TEXT.contains(&name) || name == "plaintext"
}

/// Where the end tag of the raw text element `name` whose content starts at
/// `from` starts, or the end of the text.
fn closing(text: &str, from: usize, name: &str) -> usize {
    let bytes = text.as_bytes();
    let mut at = from;
    while let Some(found) = text[at..].find("</") {
        let start = at + found;
        let end = start + 2 + name.len();
        if end <= bytes.len()
            && bytes[start + 2..end].eq_ignore_ascii_case(name.as_bytes())
            && bytes
                .get(end)
                .is_none_or(|c| space(*c) || *c == b'/' || *c == b'>')
        {
            return start;
        }
        at = start + 2;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(tree: &Tree, node: usize) -> Vec<String> {
        tree.children(node)
            .filter_map(|c| tree.element(c).map(|e| e.name.to_string()))
            .collect()
    }

    fn find(tree: &Tree, name: &str) -> Vec<usize> {
        (0..tree.nodes.len())
            .filter(|n| tree.name(*n) == name)
            .collect()
    }

    #[test]
    fn implied_end_tags_close_paragraphs_items_and_cells() {
        let text = "<ul><li>a<li>b</ul><p>x<div>y</div><table><tr><td>1<td>2<tr><td>3</table>";
        let tree = parse(text);
        let items = find(&tree, "li");
        assert_eq!(items.len(), 2);
        assert_eq!(&text[tree.nodes[items[0]].range.clone()], "<li>a");
        assert_eq!(&text[tree.nodes[items[1]].range.clone()], "<li>b");
        let p = find(&tree, "p")[0];
        assert_eq!(&text[tree.nodes[p].range.clone()], "<p>x");
        assert_eq!(names(&tree, 0), ["ul", "p", "div", "table"]);
        let rows = find(&tree, "tr");
        assert_eq!(names(&tree, rows[0]), ["td", "td"]);
        assert_eq!(&text[tree.nodes[rows[0]].range.clone()], "<tr><td>1<td>2");
    }

    #[test]
    fn nesting_past_the_deepest_goes_to_the_parent() {
        let text = format!("{}<script>x<y</script>z", "<span>".repeat(MAX_DEPTH + 10));
        let tree = parse(&text);
        let depth = |n: usize| tree.ancestors(n).count();
        assert_eq!((0..tree.nodes.len()).map(depth).max(), Some(MAX_DEPTH + 1));
        let script = find(&tree, "script")[0];
        let content = tree.children(script).next().unwrap();
        assert_eq!(&text[tree.nodes[content].range.clone()], "x<y");
        assert_eq!(tree.descendants(0), 1..tree.nodes.len());
        assert_eq!(tree.descendants(script), script + 1..script + 2);
    }

    #[test]
    fn raw_text_and_comments_hold_no_tags() {
        let text = "<script>if (a<b) x='</p>'</script><title>A &amp; B</title><!-- <p> -->z";
        let tree = parse(text);
        assert!(find(&tree, "p").is_empty());
        let title = find(&tree, "title")[0];
        let content = tree.children(title).next().unwrap();
        assert_eq!(&text[tree.nodes[content].range.clone()], "A &amp; B");
    }

    #[test]
    fn references_follow_the_html_rules() {
        assert_eq!(
            unescape("a &amp; b &lt;&#x41;&#66;&copy &notit; &#128;", false),
            "a & b <AB© ¬it; €"
        );
        assert_eq!(unescape("?a=1&copy=2", true), "?a=1&copy=2");
        assert_eq!(unescape("&unknown; & &#;", false), "&unknown; & &#;");
    }

    #[test]
    fn head_closes_when_body_content_starts() {
        let text = "<html><head><meta charset=utf-8><title>t</title><p>body</p></html>";
        let tree = parse(text);
        let head = find(&tree, "head")[0];
        assert_eq!(names(&tree, head), ["meta", "title"]);
        assert!(
            tree.ancestors(find(&tree, "p")[0])
                .all(|n| tree.name(n) != "head")
        );
    }

    #[test]
    fn encodings_come_from_bom_or_meta() {
        let (sjis, _, _) =
            encoding_rs::SHIFT_JIS.encode("<meta charset=\"Shift_JIS\"><p>日本語</p>");
        let decoded = decode(&sjis).unwrap();
        assert_eq!(decoded.encoding, encoding_rs::SHIFT_JIS);
        assert!(decoded.text.contains("日本語"));
        let (euc, _, _) = encoding_rs::EUC_JP.encode(
            "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=EUC-JP\"><p>表</p>",
        );
        assert_eq!(decode(&euc).unwrap().encoding, encoding_rs::EUC_JP);
        assert!(decode(&sjis[26..]).is_err());
        let bom = [&[0xEF, 0xBB, 0xBF][..], "<p>a</p>".as_bytes()].concat();
        let decoded = decode(&bom).unwrap();
        assert_eq!((decoded.encoding, decoded.bom), (UTF_8, 3));
    }

    #[test]
    fn raw_offsets_follow_multibyte_characters() {
        let (raw, _, _) = encoding_rs::SHIFT_JIS.encode("<meta charset=shift_jis>表\\ｱx");
        let decoded = decode(&raw).unwrap();
        let x = decoded.text.find('x').unwrap();
        let offsets = raw_offsets(&raw, &decoded, &[0, 24, x].into()).unwrap();
        // 表 takes two bytes, the backslash one and half-width ｱ one.
        assert_eq!(offsets[&x], 24 + 2 + 1 + 1);
        assert_eq!(offsets[&24], 24);
        assert!(encode("表", encoding_rs::SHIFT_JIS).is_ok());
        assert!(encode("😀", encoding_rs::SHIFT_JIS).is_err());
        assert!(ensure_writable(ISO_2022_JP).is_err());
    }
}
