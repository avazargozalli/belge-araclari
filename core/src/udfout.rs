//! The UDF document model the Word and PDF readers build, and the writer that turns it into a
//! `content.xml` UYAP Doküman Editörü opens. The Editör's rules, as documented by people who
//! reverse-engineered it: one `content.xml` in the ZIP, `format_id="1.8"`, all text in one CDATA
//! block that the `startOffset`/`length` of every run covers without gaps (UTF-16 units), each
//! paragraph ending in "\n", no characters outside the Basic Multilingual Plane, one placeholder
//! character per image, `<page-break>` elements for page breaks (top level only), no line breaks
//! inside a paragraph, and paper sizes given as `mediaSizeName` codes.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;

use base64::Engine as _;

// MARK: - Properties

#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct RunProps {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub color: Option<Option<u32>>,
    pub background: Option<Option<u32>>,
    pub vert: Option<i8>,
    pub caps: Option<bool>,
    pub hidden: Option<bool>,
    pub turkish: Option<bool>,
}

impl RunProps {
    pub fn apply(&mut self, o: &RunProps) {
        overlay!(self, o, font, size, bold, italic, underline, strike, color, background, vert, caps, hidden, turkish);
    }

    /// Same visible formatting (runs with the same look are merged).
    pub fn same_look(&self, b: &RunProps) -> bool {
        self.font == b.font && self.size == b.size && self.bold == b.bold && self.italic == b.italic && self.underline == b.underline && self.strike == b.strike && self.color == b.color && self.background == b.background && self.vert == b.vert
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Line {
    /// Multiple of single spacing.
    Auto(f32),
    Exact(f32),
    AtLeast(f32),
}

#[derive(Clone, Debug)]
pub(crate) enum TabOp {
    /// Position from the margin, alignment (0 left, 1 right, 2 centre, 4 decimal, 5 bar), leader.
    Set(f32, u8, u8),
    Clear(f32),
}

#[derive(Clone, Default, Debug)]
pub(crate) struct ParaProps {
    pub align: Option<u8>,
    pub left: Option<f32>,
    pub right: Option<f32>,
    /// Positive: first-line indent; negative: hanging indent.
    pub first: Option<f32>,
    pub before: Option<f32>,
    pub after: Option<f32>,
    pub line: Option<Line>,
    pub tabs: Vec<TabOp>,
    pub num: Option<(String, u32)>,
    pub contextual: Option<bool>,
    pub page_break: Option<bool>,
}

impl ParaProps {
    pub fn apply(&mut self, o: &ParaProps) {
        overlay!(self, o, align, left, right, first, before, after, line, num, contextual, page_break);
        self.tabs.extend(o.tabs.iter().cloned());
    }

    /// Tab stops after applying set/clear operations in order, sorted by position.
    pub fn stops(&self) -> Vec<(f32, u8, u8)> {
        let mut out: Vec<(f32, u8, u8)> = vec![];
        for op in &self.tabs {
            match op {
                TabOp::Set(pos, a, l) => {
                    out.retain(|s| (s.0 - pos).abs() > 0.5);
                    out.push((*pos, *a, *l));
                }
                TabOp::Clear(pos) => out.retain(|s| (s.0 - pos).abs() > 0.5),
            }
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }
}

// MARK: - Content model

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Field {
    Page,
    Total,
}

#[derive(Clone)]
pub(crate) enum Piece {
    Text(String, RunProps),
    Image { data: Vec<u8>, width: f32, height: f32, props: RunProps },
    Field(Field, RunProps),
}

/// A native UYAP list item.
#[derive(Clone, Debug)]
pub(crate) struct ListItem {
    pub numbered: bool,
    /// NUMBER_TYPE_… or BULLET_TYPE_…
    pub kind: String,
    pub id: String,
    /// 1-based.
    pub level: u32,
    /// The item's number as the source shows it.
    pub number: i64,
}

#[derive(Clone, Default)]
pub(crate) struct Para {
    /// From a text box or other floating shape.
    pub floating: bool,
    pub props: ParaProps,
    pub mark: RunProps,
    pub style: Option<String>,
    pub pieces: Vec<Piece>,
    pub list: Option<ListItem>,
    /// Continues the previous paragraph after a line break (no first-line indent, no list label).
    pub continuation: bool,
}

#[derive(Clone)]
pub(crate) struct Cell {
    pub span: usize,
    pub blocks: Vec<Block>,
}

#[derive(Clone)]
pub(crate) struct Table {
    /// Grid column widths in points.
    pub grid: Vec<f32>,
    pub indent: f32,
    pub border: &'static str,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Clone)]
pub(crate) enum Block {
    Para(Para),
    Table(Table),
}

pub(crate) struct Background {
    pub data: Vec<u8>,
    /// Position and size on the page, in points.
    pub rect: [f32; 4],
}

/// Everything a UDF holds.
pub(crate) struct Document {
    /// Width, height in points.
    pub page: [f32; 2],
    /// Top, right, bottom, left.
    pub margins: [f32; 4],
    pub header_offset: f32,
    pub footer_offset: f32,
    pub header: Vec<Block>,
    pub footer: Vec<Block>,
    pub body: Vec<Block>,
    /// UDF page-number attributes (see [`page_number`]).
    pub header_number: Option<String>,
    pub footer_number: Option<String>,
    pub background: Option<Background>,
    pub font: String,
    pub size: f32,
    /// Default tab interval, in points.
    pub default_tab: f32,
}

// MARK: - Helpers

/// Java's signed ARGB integer for an opaque colour.
pub(crate) fn argb(rgb: u32) -> i32 {
    (0xFF00_0000u32 | rgb) as i32
}

pub(crate) fn num_text(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) fn attr_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 => {}
            c if (c as u32) > 0xFFFF => out.push('\u{25A1}'),
            c => out.push(c),
        }
    }
    out
}

/// Text the Editör can hold: no control characters (tabs and newlines aside), nothing outside the
/// Basic Multilingual Plane (emoji make it fail to open the file), and no "]]>" closing the CDATA.
pub(crate) fn clean(text: &str) -> String {
    let out: String = text
        .chars()
        .filter_map(|c| match c {
            '\t' | '\n' => Some(c),
            c if (c as u32) < 0x20 => None,
            '\u{FFFE}' | '\u{FFFF}' | '\u{2028}' | '\u{2029}' => None,
            c if (c as u32) > 0xFFFF => Some('\u{25A1}'),
            c => Some(c),
        })
        .collect();
    out.replace("]]>", "]] >")
}

/// Image placeholder character in the text.
const IMAGE_CHAR: &str = "\u{00B8}";

/// Paper sizes the Editör knows, portrait, in points.
const MEDIA: [(u8, f32, f32); 6] = [(1, 595.28, 841.89), (2, 419.53, 595.28), (3, 612.0, 792.0), (4, 515.91, 728.5), (5, 612.0, 1008.0), (6, 522.0, 756.0)];

/// `mediaSizeName` code and portrait size for a page; unknown sizes become A4.
pub(crate) fn media(width: f32, height: f32) -> (u8, f32, f32) {
    let (short, long) = if width < height { (width, height) } else { (height, width) };
    MEDIA.iter().copied().find(|(_, w, h)| (w - short).abs() <= 6.0 && (h - long).abs() <= 6.0).unwrap_or(MEDIA[0])
}

/// Paper size for a `mediaSizeName` code (portrait).
pub fn media_size(code: &str) -> Option<(f32, f32)> {
    let code: u8 = code.trim().parse().ok()?;
    MEDIA.iter().find(|m| m.0 == code).map(|m| (m.1, m.2))
}

/// Splits paragraphs at line breaks (U+2028): the Editör has no line break inside a paragraph.
fn split_breaks(p: &Para) -> Vec<Para> {
    let mut parts: Vec<Vec<Piece>> = vec![vec![]];
    for piece in &p.pieces {
        match piece {
            Piece::Text(text, props) if text.contains('\u{2028}') => {
                for (i, segment) in text.split('\u{2028}').enumerate() {
                    if i > 0 {
                        parts.push(vec![]);
                    }
                    if !segment.is_empty() {
                        parts.last_mut().unwrap().push(Piece::Text(segment.to_string(), props.clone()));
                    }
                }
            }
            other => parts.last_mut().unwrap().push(other.clone()),
        }
    }
    if parts.len() == 1 {
        return vec![p.clone()];
    }
    let count = parts.len();
    parts
        .into_iter()
        .enumerate()
        .map(|(i, pieces)| {
            let mut part = p.clone();
            part.pieces = pieces;
            if i > 0 {
                part.continuation = true;
                part.list = None;
                part.props.before = Some(0.0);
                part.props.page_break = Some(false);
                part.props.contextual = None;
            }
            if i + 1 < count {
                part.props.after = Some(0.0);
            }
            part
        })
        .collect()
}

/// Whether a page break comes before any visible content of the paragraph.
pub(crate) fn page_break_at_start(p: &Para) -> bool {
    for piece in &p.pieces {
        match piece {
            Piece::Text(t, _) => {
                let t = t.trim_start_matches(|c: char| c.is_whitespace() && c != '\u{000C}');
                if t.starts_with('\u{000C}') {
                    return true;
                }
                if !t.is_empty() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    false
}

// MARK: - Writer

struct Writer {
    text: String,
    units: usize,
    out: String,
    content_width: f32,
    default_tab: f32,
    /// 0 in the body, more inside table cells; page breaks only exist at the top.
    depth: usize,
    in_body: bool,
    /// The Editör's own list counters, to know when NumberSetted is needed.
    counters: HashMap<String, BTreeMap<u32, i64>>,
}

impl Writer {
    fn add(&mut self, text: &str) -> (usize, usize) {
        let start = self.units;
        let len = text.encode_utf16().count();
        self.text.push_str(text);
        self.units += len;
        (start, len)
    }

    fn run_attrs(p: &RunProps, with_background: bool) -> String {
        let font = p.font.clone().unwrap_or_else(|| "Times New Roman".into());
        let mut a = format!(
            "family=\"{}\" size=\"{}\" bold=\"{}\" italic=\"{}\" underline=\"{}\" strikethrough=\"{}\" superscript=\"{}\" subscript=\"{}\" foreground=\"{}\"",
            attr_escape(&font),
            num_text(p.size.unwrap_or(10.0)),
            p.bold.unwrap_or(false),
            p.italic.unwrap_or(false),
            p.underline.unwrap_or(false),
            p.strike.unwrap_or(false),
            p.vert == Some(1),
            p.vert == Some(-1),
            argb(p.color.flatten().unwrap_or(0)),
        );
        if with_background {
            if let Some(bg) = p.background.flatten() {
                write!(a, " background=\"{}\"", argb(bg)).ok();
            }
        }
        a
    }

    /// An empty paragraph inside `<page-break>`, as small as possible.
    fn page_break(&mut self) {
        let (start, len) = self.add("\n");
        write!(
            self.out,
            "<page-break><paragraph LineSpacing=\"0\" SpaceAbove=\"0\" SpaceBelow=\"0\" size=\"1\"><content startOffset=\"{start}\" length=\"{len}\" size=\"1\"/></paragraph></page-break>"
        )
        .ok();
    }

    fn list_attrs(&mut self, item: &ListItem) -> String {
        // Simulate the Editör's counters (per list, deeper levels restart) and fix the number
        // with NumberSetted only where it would differ from the source.
        let counter = self.counters.entry(item.id.clone()).or_default();
        counter.retain(|k, _| *k <= item.level);
        let expected = counter.get(&item.level).copied().unwrap_or(0) + 1;
        let mut a = String::new();
        let (flag, key) = if item.numbered { ("Numbered", "NumberType") } else { ("Bulleted", "BulletType") };
        write!(a, " {flag}=\"true\" {key}=\"{}\" ListId=\"{}\" ListLevel=\"{}\"", item.kind, attr_escape(&item.id), item.level).ok();
        if item.level > 1 {
            write!(a, " SecListTypeLevel{}=\"{}\"", item.level, item.kind).ok();
        }
        if item.numbered && item.number != expected {
            write!(a, " NumberSetted=\"{}\"", item.number - 1).ok();
        }
        counter.insert(item.level, if item.numbered { item.number } else { expected });
        a
    }

    fn paragraph(&mut self, p: &Para, next_style_same: bool, prev_contextual_same: bool) {
        let props = &p.props;
        let size = p.mark.size.unwrap_or(10.0).max(1.0);
        // Source: first line at left + first, wrapped lines at left. UDF: first line at LeftIndent +
        // FirstLineIndent, wrapped lines at LeftIndent + Hanging.
        let left = props.left.unwrap_or(0.0).max(0.0);
        let first = if p.continuation { 0.0 } else { props.first.unwrap_or(0.0) };
        let (udf_left, udf_first, udf_hanging) = if first >= 0.0 {
            (left, first, 0.0)
        } else {
            let l = (left + first).max(0.0);
            // A native list draws its label itself; the text starts where the source's does.
            if p.list.is_some() { (l, left - l, left - l) } else { (l, 0.0, left - l) }
        };
        let mut stops: Vec<(f32, u8, u8)> = props.stops();
        if first < 0.0 && p.list.is_none() && !stops.iter().any(|s| (s.0 - left).abs() < 0.5) {
            // A hanging indent doubles as a tab stop (text list labels tab to the text).
            stops.push((left, 0, 0));
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        // Past the last explicit stop the source continues on its default grid.
        let last = stops.last().map_or(0.0, |s| s.0);
        let mut k = (last / self.default_tab).floor() + 1.0;
        while k * self.default_tab < self.content_width + self.default_tab && stops.len() < 60 {
            stops.push((k * self.default_tab, 0, 0));
            k += 1.0;
        }
        let tabset: Vec<String> = stops.iter().filter(|s| s.0 - udf_left > 0.25).map(|(pos, a, l)| format!("{}:{a}:{l}", num_text(pos - udf_left))).collect();
        let line = match props.line {
            Some(Line::Auto(k)) => k - 1.0,
            Some(Line::Exact(pt)) => pt / (size * 1.15) - 1.0,
            Some(Line::AtLeast(pt)) => (pt / (size * 1.15)).max(1.0) - 1.0,
            None => 0.0,
        };
        let contextual = props.contextual == Some(true);
        let before = if contextual && prev_contextual_same { 0.0 } else { props.before.unwrap_or(0.0) };
        let after = if contextual && next_style_same { 0.0 } else { props.after.unwrap_or(0.0) };
        let list = p.list.as_ref().map(|item| self.list_attrs(item)).unwrap_or_default();
        write!(
            self.out,
            "<paragraph Alignment=\"{}\" LeftIndent=\"{}\" RightIndent=\"{}\" FirstLineIndent=\"{}\" Hanging=\"{}\" LineSpacing=\"{}\" SpaceAbove=\"{}\" SpaceBelow=\"{}\" TabSet=\"{}\"{list} {}>",
            props.align.unwrap_or(0),
            num_text(udf_left),
            num_text(props.right.unwrap_or(0.0).max(0.0)),
            num_text(udf_first),
            num_text(udf_hanging),
            num_text(line.max(-0.3)),
            num_text(before.max(0.0)),
            num_text(after.max(0.0)),
            tabset.join(","),
            Self::run_attrs(&p.mark, false),
        )
        .ok();
        let mut wrote_text = false;
        let count = p.pieces.len();
        for (i, piece) in p.pieces.iter().enumerate() {
            let last_piece = i + 1 == count;
            match piece {
                Piece::Text(text, props) => {
                    let mut text = clean(text);
                    if last_piece {
                        text.push('\n');
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let (start, len) = self.add(&text);
                    write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(props, true)).ok();
                    wrote_text = true;
                }
                Piece::Image { data, width, height, props } => {
                    let (start, len) = self.add(IMAGE_CHAR);
                    let mut size = String::new();
                    if *width > 0.0 && *height > 0.0 {
                        size = format!(" width=\"{}\" height=\"{}\"", num_text(*width), num_text(*height));
                    }
                    write!(
                        self.out,
                        "<image startOffset=\"{start}\" length=\"{len}\" imageData=\"{}\"{size} {}/>",
                        base64::engine::general_purpose::STANDARD.encode(data),
                        Self::run_attrs(props, false)
                    )
                    .ok();
                    if last_piece {
                        let (start, len) = self.add("\n");
                        write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(props, false)).ok();
                    }
                    wrote_text = true;
                }
                Piece::Field(..) => {}
            }
        }
        if !wrote_text || matches!(p.pieces.last(), Some(Piece::Field(..))) {
            let (start, len) = self.add("\n");
            write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(&p.mark, false)).ok();
        }
        self.out.push_str("</paragraph>");
    }

    fn blocks(&mut self, blocks: &[Block], width: f32) {
        let saved = self.content_width;
        self.content_width = width;
        let expanded: Vec<Block> = blocks
            .iter()
            .flat_map(|b| match b {
                Block::Para(p) => split_breaks(p).into_iter().map(Block::Para).collect::<Vec<_>>(),
                Block::Table(t) => vec![Block::Table(t.clone())],
            })
            .collect();
        let breaks_allowed = self.in_body && self.depth == 0;
        // A page break inside a paragraph's text moves the following block to a new page.
        let mut pending_break = false;
        for (i, block) in expanded.iter().enumerate() {
            match block {
                Block::Para(p) => {
                    let breaks = p.props.page_break == Some(true) || pending_break || page_break_at_start(p);
                    if breaks && breaks_allowed && (i > 0 || self.units > 0) {
                        self.page_break();
                    }
                    let same = |other: Option<&Block>| matches!(other, Some(Block::Para(o)) if o.style == p.style);
                    let prev_same = i > 0 && same(expanded.get(i - 1)) && matches!(expanded.get(i - 1), Some(Block::Para(o)) if o.props.contextual == Some(true));
                    self.paragraph(p, same(expanded.get(i + 1)), prev_same);
                    pending_break = !page_break_at_start(p) && p.pieces.iter().any(|x| matches!(x, Piece::Text(t, _) if t.contains('\u{000C}')));
                }
                Block::Table(t) => {
                    if pending_break && breaks_allowed {
                        self.page_break();
                    }
                    self.table(t, width);
                    pending_break = false;
                }
            }
        }
        self.content_width = saved;
    }

    fn table(&mut self, t: &Table, width: f32) {
        let total: f32 = t.grid.iter().sum();
        let left = t.indent.max(0.0);
        let right = (width - left - total).max(0.0);
        let spans = |cells: &[Cell]| -> Vec<f32> {
            let mut col = 0;
            cells
                .iter()
                .map(|c| {
                    let w: f32 = t.grid.iter().skip(col).take(c.span).sum();
                    col += c.span;
                    w.max(1.0)
                })
                .collect()
        };
        let join = |v: &[f32]| v.iter().map(|w| num_text(*w)).collect::<Vec<_>>().join(",");
        write!(
            self.out,
            "<table tableName=\"Sabit\" columnCount=\"{}\" columnSpans=\"{}\" border=\"{}\" LeftIndent=\"{}\" RightIndent=\"{}\">",
            t.grid.len(),
            join(&t.grid),
            t.border,
            num_text(left),
            num_text(right)
        )
        .ok();
        self.depth += 1;
        for (r, row) in t.rows.iter().enumerate() {
            let widths = spans(row);
            write!(self.out, "<row rowName=\"row{}\" rowType=\"dataRow\" columnSpans=\"{}\">", r + 1, join(&widths)).ok();
            for (cell, w) in row.iter().zip(&widths) {
                self.out.push_str("<cell>");
                if cell.blocks.is_empty() {
                    let (start, len) = self.add("\n");
                    write!(self.out, "<paragraph><content startOffset=\"{start}\" length=\"{len}\"/></paragraph>").ok();
                } else {
                    self.blocks(&cell.blocks, *w - 2.0);
                }
                self.out.push_str("</cell>");
            }
            self.out.push_str("</row>");
        }
        self.depth -= 1;
        self.out.push_str("</table>");
    }
}

/// Takes the paragraph holding a PAGE field out of a header/footer and describes it as UDF
/// page-number attributes. Also says whether it came from a floating shape (often a rotated box in
/// the margin), which UDF cannot place; those go to the bottom right instead.
pub(crate) fn page_number(blocks: &mut Vec<Block>) -> Option<(String, bool)> {
    let index = blocks.iter().position(|b| matches!(b, Block::Para(p) if p.pieces.iter().any(|x| matches!(x, Piece::Field(Field::Page, _)))))?;
    let Block::Para(mut p) = blocks.remove(index) else { return None };
    let floating = p.floating;
    if floating {
        p.props.align = Some(2);
    }
    let (mut fore, mut sep, mut after) = (String::new(), String::new(), String::new());
    let mut stage = 0;
    let mut total = false;
    let mut font = p.mark.clone();
    for piece in &p.pieces {
        match piece {
            Piece::Text(t, _) => {
                let t = t.replace(['\t', '\u{2028}', '\u{000C}'], " ");
                match stage {
                    0 => fore.push_str(&t),
                    1 => sep.push_str(&t),
                    _ => after.push_str(&t),
                }
            }
            Piece::Field(Field::Page, props) => {
                font = props.clone();
                stage = stage.max(1);
            }
            Piece::Field(Field::Total, _) => {
                total = true;
                stage = 2;
            }
            Piece::Image { .. } => {}
        }
    }
    if !total {
        after = sep.clone() + &after;
        sep = "/".into();
    }
    Some((page_number_attrs(&fore, &sep, &after, total, p.props.align.unwrap_or(0), &font, floating), floating))
}

/// UDF page-number attributes. Bits of `pageNumber-spec` BSP32_n: 8 shows the number, 32 centres
/// it, 64 right-aligns it, 2048 adds the total.
pub(crate) fn page_number_attrs(fore: &str, sep: &str, after: &str, total: bool, align: u8, font: &RunProps, small: bool) -> String {
    let mut flags = 8;
    match align {
        1 => flags |= 32,
        2 => flags |= 64,
        _ => {}
    }
    if total {
        flags |= 2048;
    }
    let size = font.size.unwrap_or(10.0);
    let mut a = format!(
        " pageNumber-spec=\"BSP32_{flags}\" pageNumber-foreStr=\"{}\" pageNumber-afterStr=\"{}\" pageNumber-seperator=\"{}\" pageNumber-fontFace=\"{}\" pageNumber-fontSize=\"{}\"",
        attr_escape(fore.trim_start()),
        attr_escape(after.trim_end()),
        attr_escape(sep),
        attr_escape(font.font.as_deref().unwrap_or("Times New Roman")),
        num_text(if small { size.min(11.0) } else { size }),
    );
    for (key, value) in [("Bold", font.bold), ("Italic", font.italic), ("Underline", font.underline)] {
        if value == Some(true) {
            write!(a, " pageNumber-font{key}=\"true\"").ok();
        }
    }
    if let Some(c) = font.color.flatten() {
        write!(a, " pageNumber-color=\"{}\"", argb(c)).ok();
    }
    a
}

/// Writes a UDF `content.xml`.
pub(crate) fn write(doc: &Document) -> String {
    let [w, h] = doc.page;
    let [mt, mr, mb, ml] = doc.margins;
    let content_width = (w - ml - mr).max(50.0);
    let mut writer = Writer { text: String::new(), units: 0, out: String::new(), content_width, default_tab: doc.default_tab, depth: 0, in_body: false, counters: HashMap::new() };
    let mut elements = String::new();
    for (tag, blocks, number) in [("header", &doc.header, &doc.header_number), ("footer", &doc.footer, &doc.footer_number)] {
        if blocks.is_empty() && number.is_none() {
            continue;
        }
        writer.out.clear();
        if blocks.is_empty() {
            // The Editör keeps a header/footer element with at least one paragraph.
            let (start, len) = writer.add("\n");
            write!(writer.out, "<paragraph><content startOffset=\"{start}\" length=\"{len}\"/></paragraph>").ok();
        } else {
            writer.blocks(blocks, content_width);
        }
        write!(elements, "<{tag} background=\"-1\" foreground=\"-16777216\"{}>{}</{tag}>", number.as_deref().unwrap_or(""), writer.out).ok();
    }
    writer.out.clear();
    writer.in_body = true;
    let body: Vec<Block> = if doc.body.is_empty() {
        vec![Block::Para(Para { mark: RunProps { font: Some(doc.font.clone()), size: Some(doc.size), ..Default::default() }, ..Default::default() })]
    } else {
        doc.body.clone()
    };
    writer.blocks(&body, content_width);
    elements.push_str(&writer.out);

    let (code, pw, ph) = media(w, h);
    let landscape = w > h;
    let mut properties = format!(
        "<pageFormat mediaSizeName=\"{code}\" leftMargin=\"{}\" rightMargin=\"{}\" topMargin=\"{}\" bottomMargin=\"{}\" paperOrientation=\"{}\" headerFOffset=\"{}\" footerFOffset=\"{}\"/>",
        num_text(ml),
        num_text(mr),
        num_text(mt),
        num_text(mb),
        if landscape { 0 } else { 1 },
        num_text(doc.header_offset),
        num_text(doc.footer_offset),
    );
    if let Some(bg) = &doc.background {
        // Positions are relative to the page the Editör will use.
        let (page_w, page_h) = if landscape { (ph, pw) } else { (pw, ph) };
        let [x, y, bw, bh] = bg.rect;
        write!(
            properties,
            "<bgImage bgImageSource=\"antet\" bgImageData=\"{}\" bgImageUpMargin=\"{}\" bgImageLeftMargin=\"{}\" bgImageBottomMargin=\"{}\" bgImageRigtMargin=\"{}\"/>",
            base64::engine::general_purpose::STANDARD.encode(&bg.data),
            num_text(y),
            num_text(x),
            num_text(page_h - y - bh),
            num_text(page_w - x - bw)
        )
        .ok();
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<template format_id=\"1.8\" >\n<content><![CDATA[{}]]></content>\n<properties>{properties}</properties>\n\
<elements resolver=\"hvl-default\" >{elements}</elements>\n<styles><style name=\"default\" description=\"Geçerli\" family=\"{f}\" size=\"{s}\"/>\
<style name=\"hvl-default\" family=\"{f}\" size=\"{s}\" description=\"Gövde\"/></styles>\n</template>\n",
        writer.text,
        f = attr_escape(&doc.font),
        s = num_text(doc.size),
    )
}

/// Wraps a `content.xml` into a .udf (ZIP) file.
pub fn udf_bytes(content_xml: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("content.xml", options).map_err(|e| e.to_string())?;
    zip.write_all(content_xml.as_bytes()).map_err(|e| e.to_string())?;
    zip.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Checks a `content.xml` against the Editör's rules. Returns the problems found.
pub fn validate(xml: &str) -> Vec<String> {
    let mut problems = vec![];
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = match roxmltree::Document::parse_with_options(xml, options) {
        Ok(d) => d,
        Err(e) => return vec![format!("XML bozuk: {e}")],
    };
    let root = doc.root_element();
    if root.attribute("format_id") != Some("1.8") {
        problems.push("format_id 1.8 değil".into());
    }
    let text: String = root.children().find(|n| n.has_tag_name("content")).map(|n| n.children().filter_map(|c| c.text()).collect()).unwrap_or_default();
    let units: Vec<u16> = text.encode_utf16().collect();
    if text.chars().any(|c| (c as u32) > 0xFFFF) {
        problems.push("BMP dışı karakter var".into());
    }
    let Some(elements) = root.children().find(|n| n.has_tag_name("elements")) else {
        problems.push("elements yok".into());
        return problems;
    };
    let mut next = 0usize;
    for run in elements.descendants().filter(|n| n.has_tag_name("content") || n.has_tag_name("image")) {
        let (Some(start), Some(len)) = (run.attribute("startOffset").and_then(|v| v.parse::<usize>().ok()), run.attribute("length").and_then(|v| v.parse::<usize>().ok())) else {
            problems.push("startOffset/length eksik".into());
            continue;
        };
        if start != next {
            problems.push(format!("metin aralığında boşluk veya çakışma: {next} beklenirken {start}"));
        }
        next = start + len;
    }
    if next != units.len() {
        problems.push(format!("aralıklar metni kapsamıyor: {next} / {}", units.len()));
    }
    for p in elements.descendants().filter(|n| n.has_tag_name("paragraph")) {
        let last = p.children().filter(|c| c.has_tag_name("content") || c.has_tag_name("image")).last();
        let ends_well = last.and_then(|c| {
            let start: usize = c.attribute("startOffset")?.parse().ok()?;
            let len: usize = c.attribute("length")?.parse().ok()?;
            units.get(start + len - 1).copied()
        }) == Some(b'\n' as u16);
        if !ends_well {
            problems.push("paragraf \\n ile bitmiyor".into());
        }
    }
    for t in elements.descendants().filter(|n| n.has_tag_name("table")) {
        let columns: usize = t.attribute("columnCount").and_then(|v| v.parse().ok()).unwrap_or(0);
        for row in t.children().filter(|n| n.has_tag_name("row")) {
            let cells = row.children().filter(|n| n.has_tag_name("cell")).count();
            let expected = row.attribute("columnSpans").map(|s| s.split(',').count()).unwrap_or(columns);
            if cells != expected {
                problems.push(format!("tablo satırında {cells} hücre var, {expected} bekleniyor"));
            }
        }
    }
    for b in elements.descendants().filter(|n| n.has_tag_name("page-break")) {
        if b.parent().map(|p| p.has_tag_name("elements")) != Some(true) {
            problems.push("page-break yalnızca en üst düzeyde olabilir".into());
        }
    }
    problems
}
