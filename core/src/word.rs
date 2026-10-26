//! Word (.docx) → UDF, the built-in Office converter.
//!
//! A .docx is read into paragraphs and tables with Word's own rules (document defaults, style
//! inheritance, numbering, direct formatting) and written as a UDF `content.xml`, so the existing
//! UDF renderers (TextKit on macOS, WebView2 on Windows) can draw it as a PDF. Things UDF has no
//! place for are approximated: list numbers become text, footnotes move to the end of the document,
//! the last section's page setup and default header/footer apply to the whole document, a large
//! picture behind the header text becomes the letterhead, and a PAGE / NUMPAGES field in the header
//! or footer becomes the UDF page number.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::path::Path;

use base64::Engine as _;
use roxmltree::Node;

type Doc<'a> = roxmltree::Document<'a>;

// MARK: - XML helpers

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i: 'a>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// Attribute by local name (w:val, r:id…), which also covers Strict OOXML namespaces.
fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn val<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    attr(n, "val")
}

fn twips(v: Option<&str>) -> Option<f32> {
    v.and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v / 20.0)
}

/// On/off properties: present without a value means on.
fn on(n: Node) -> bool {
    !matches!(val(n), Some("0" | "false" | "off" | "none"))
}

// MARK: - Properties

#[derive(Clone, Default, Debug)]
struct RunProps {
    font: Option<String>,
    size: Option<f32>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Option<u32>>,
    background: Option<Option<u32>>,
    vert: Option<i8>,
    caps: Option<bool>,
    hidden: Option<bool>,
    turkish: Option<bool>,
}

macro_rules! overlay {
    ($self:ident, $other:ident, $($f:ident),*) => { $( if $other.$f.is_some() { $self.$f = $other.$f.clone(); } )* };
}

impl RunProps {
    fn apply(&mut self, o: &RunProps) {
        overlay!(self, o, font, size, bold, italic, underline, strike, color, background, vert, caps, hidden, turkish);
    }

    fn read(n: Option<Node>, theme: &Theme) -> RunProps {
        let mut p = RunProps::default();
        let Some(n) = n else { return p };
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "rFonts" => {
                    p.font = attr(c, "ascii")
                        .or_else(|| attr(c, "hAnsi"))
                        .map(str::to_string)
                        .or_else(|| attr(c, "asciiTheme").or_else(|| attr(c, "hAnsiTheme")).and_then(|t| theme.font(t)));
                }
                "sz" => p.size = val(c).and_then(|v| v.parse::<f32>().ok()).map(|v| v / 2.0),
                "b" => p.bold = Some(on(c)),
                "i" => p.italic = Some(on(c)),
                "u" => p.underline = Some(val(c).map_or(true, |v| v != "none")),
                "strike" | "dstrike" => p.strike = Some(on(c)),
                "color" => p.color = Some(val(c).and_then(hex)),
                "highlight" => p.background = Some(val(c).and_then(highlight)),
                "shd" => {
                    if p.background.is_none() {
                        p.background = Some(attr(c, "fill").and_then(hex));
                    }
                }
                "vertAlign" => {
                    p.vert = Some(match val(c) {
                        Some("superscript") => 1,
                        Some("subscript") => -1,
                        _ => 0,
                    })
                }
                "caps" => p.caps = Some(on(c)),
                "vanish" => p.hidden = Some(on(c)),
                "lang" => p.turkish = val(c).map(|v| v.to_ascii_lowercase().starts_with("tr")),
                _ => {}
            }
        }
        p
    }
}

fn hex(v: &str) -> Option<u32> {
    if v.eq_ignore_ascii_case("auto") {
        return None;
    }
    u32::from_str_radix(v.trim_start_matches('#'), 16).ok().filter(|_| v.len() >= 6)
}

fn highlight(name: &str) -> Option<u32> {
    Some(match name {
        "yellow" => 0xFFFF00,
        "green" => 0x00FF00,
        "cyan" => 0x00FFFF,
        "magenta" => 0xFF00FF,
        "blue" => 0x0000FF,
        "red" => 0xFF0000,
        "darkBlue" => 0x000080,
        "darkCyan" => 0x008080,
        "darkGreen" => 0x008000,
        "darkMagenta" => 0x800080,
        "darkRed" => 0x800000,
        "darkYellow" => 0x808000,
        "darkGray" => 0x808080,
        "lightGray" => 0xC0C0C0,
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug)]
enum Line {
    Auto(f32),
    Exact(f32),
    AtLeast(f32),
}

#[derive(Clone, Debug)]
enum TabOp {
    Set(f32, u8, u8),
    Clear(f32),
}

#[derive(Clone, Default, Debug)]
struct ParaProps {
    align: Option<u8>,
    left: Option<f32>,
    right: Option<f32>,
    /// Positive: first-line indent; negative: hanging indent.
    first: Option<f32>,
    before: Option<f32>,
    after: Option<f32>,
    line: Option<Line>,
    tabs: Vec<TabOp>,
    num: Option<(String, u32)>,
    contextual: Option<bool>,
    page_break: Option<bool>,
}

impl ParaProps {
    fn apply(&mut self, o: &ParaProps) {
        overlay!(self, o, align, left, right, first, before, after, line, num, contextual, page_break);
        self.tabs.extend(o.tabs.iter().cloned());
    }

    fn read(n: Option<Node>) -> ParaProps {
        let mut p = ParaProps::default();
        let Some(n) = n else { return p };
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "jc" => {
                    p.align = Some(match val(c) {
                        Some("center") => 1,
                        Some("right" | "end") => 2,
                        Some("both" | "distribute" | "justify" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute") => 3,
                        _ => 0,
                    })
                }
                "ind" => {
                    if let Some(v) = twips(attr(c, "left").or_else(|| attr(c, "start"))) {
                        p.left = Some(v);
                    }
                    if let Some(v) = twips(attr(c, "right").or_else(|| attr(c, "end"))) {
                        p.right = Some(v);
                    }
                    if let Some(v) = twips(attr(c, "hanging")) {
                        p.first = Some(-v);
                    } else if let Some(v) = twips(attr(c, "firstLine")) {
                        p.first = Some(v);
                    }
                }
                "spacing" => {
                    if let Some(v) = twips(attr(c, "before")) {
                        p.before = Some(v);
                    }
                    if let Some(v) = twips(attr(c, "after")) {
                        p.after = Some(v);
                    }
                    if attr(c, "beforeAutospacing").is_some_and(|v| v == "1" || v == "true") {
                        p.before = Some(14.0);
                    }
                    if attr(c, "afterAutospacing").is_some_and(|v| v == "1" || v == "true") {
                        p.after = Some(14.0);
                    }
                    if let Some(line) = attr(c, "line").and_then(|v| v.parse::<f32>().ok()) {
                        p.line = Some(match attr(c, "lineRule") {
                            Some("exact") => Line::Exact(line / 20.0),
                            Some("atLeast") => Line::AtLeast(line / 20.0),
                            _ => Line::Auto(line / 240.0),
                        });
                    }
                }
                "tabs" => {
                    for t in children(c, "tab") {
                        let Some(pos) = twips(attr(t, "pos")) else { continue };
                        let align = match val(t) {
                            Some("clear") => {
                                p.tabs.push(TabOp::Clear(pos));
                                continue;
                            }
                            Some("right" | "end") => 1,
                            Some("center") => 2,
                            Some("decimal") => 4,
                            Some("bar") => 5,
                            _ => 0,
                        };
                        let leader = match attr(t, "leader") {
                            Some("dot" | "middleDot") => 1,
                            Some("hyphen") => 2,
                            Some("underscore") => 3,
                            Some("heavy") => 4,
                            _ => 0,
                        };
                        p.tabs.push(TabOp::Set(pos, align, leader));
                    }
                }
                "numPr" => {
                    let id = child(c, "numId").and_then(val).map(str::to_string);
                    let level = child(c, "ilvl").and_then(val).and_then(|v| v.parse().ok()).unwrap_or(0);
                    if let Some(id) = id {
                        p.num = Some((id, level));
                    }
                }
                "contextualSpacing" => p.contextual = Some(on(c)),
                "pageBreakBefore" => p.page_break = Some(on(c)),
                _ => {}
            }
        }
        p
    }

    /// Tab stops after applying set/clear operations in order, sorted by position.
    fn stops(&self) -> Vec<(f32, u8, u8)> {
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

/// Which lines a table draws. `None` means not specified at this level.
#[derive(Clone, Copy, Default, Debug)]
struct Borders {
    outer: Option<bool>,
    inside_h: Option<bool>,
    inside_v: Option<bool>,
}

impl Borders {
    fn read(n: Option<Node>) -> Borders {
        let Some(n) = n else { return Borders::default() };
        let line = |name: &str| child(n, name).map(|b| !matches!(val(b), Some("none" | "nil")));
        let outer = ["top", "left", "start", "bottom", "right", "end"].iter().filter_map(|s| line(s)).reduce(|a, b| a || b);
        Borders { outer, inside_h: line("insideH"), inside_v: line("insideV") }
    }

    fn apply(&mut self, o: &Borders) {
        overlay!(self, o, outer, inside_h, inside_v);
    }

    /// UDF border type.
    fn kind(&self) -> &'static str {
        match (self.outer.unwrap_or(false), self.inside_h.unwrap_or(false), self.inside_v.unwrap_or(false)) {
            (_, true, true) => "borderCell",
            (true, true, false) => "borderRow",
            (true, false, _) => "borderTable",
            (false, false, true) | (false, true, false) => "borderCell",
            (false, false, false) => "borderNone",
        }
    }
}

// MARK: - Styles, theme, numbering

#[derive(Default)]
struct Theme {
    major: Option<String>,
    minor: Option<String>,
}

impl Theme {
    fn font(&self, name: &str) -> Option<String> {
        if name.starts_with("major") { self.major.clone() } else { self.minor.clone() }
    }
}

#[derive(Default)]
struct Style {
    based_on: Option<String>,
    para: ParaProps,
    run: RunProps,
    borders: Borders,
}

#[derive(Default)]
struct Styles {
    styles: HashMap<String, Style>,
    default_para: Option<String>,
    default_table: Option<String>,
    run: RunProps,
    para: ParaProps,
}

impl Styles {
    fn chain(&self, id: Option<&str>) -> Vec<&Style> {
        let mut out = vec![];
        let mut next = id.map(str::to_string);
        while let Some(id) = next {
            let Some(s) = self.styles.get(&id) else { break };
            if out.len() > 20 {
                break;
            }
            out.push(s);
            next = s.based_on.clone();
        }
        out.reverse();
        out
    }
}

#[derive(Clone)]
struct Level {
    start: i64,
    format: String,
    text: String,
    para: ParaProps,
    run: RunProps,
    suffix: char,
}

#[derive(Default)]
struct Numbering {
    abstracts: HashMap<String, Vec<Level>>,
    /// numId → abstract id and start overrides per level
    nums: HashMap<String, (String, HashMap<u32, i64>)>,
    counters: HashMap<String, [Option<i64>; 9]>,
}

impl Numbering {
    fn level(&self, id: &str, level: u32) -> Option<&Level> {
        let (abs, _) = self.nums.get(id)?;
        self.abstracts.get(abs)?.get(level as usize)
    }

    /// The label for the next item of list `id` at `level`.
    fn next(&mut self, id: &str, level: u32) -> Option<(String, Level)> {
        let (abs, overrides) = self.nums.get(id)?.clone();
        let levels = self.abstracts.get(&abs)?.clone();
        let lvl = levels.get(level as usize)?.clone();
        let counters = self.counters.entry(id.to_string()).or_insert([None; 9]);
        let level = level as usize;
        for c in counters.iter_mut().skip(level + 1) {
            *c = None;
        }
        let start = |l: usize| overrides.get(&(l as u32)).copied().unwrap_or_else(|| levels.get(l).map_or(1, |x| x.start));
        counters[level] = Some(counters[level].map_or(start(level), |n| n + 1));
        if lvl.format == "none" {
            return Some((String::new(), lvl));
        }
        let mut text = lvl.text.clone();
        for (i, l) in levels.iter().enumerate().take(9) {
            let token = format!("%{}", i + 1);
            if text.contains(&token) {
                let n = counters[i].unwrap_or_else(|| start(i));
                text = text.replace(&token, &format_number(&l.format, n));
            }
        }
        if lvl.format == "bullet" {
            text = text.chars().map(bullet_char).collect();
        }
        Some((text, lvl))
    }
}

fn roman(mut n: i64) -> String {
    let mut out = String::new();
    for (v, r) in [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            out.push_str(r);
            n -= v;
        }
    }
    out
}

fn format_number(format: &str, n: i64) -> String {
    let n = n.max(0);
    match format {
        "lowerLetter" | "upperLetter" => {
            if n == 0 {
                return String::new();
            }
            let c = (b'a' + ((n - 1) % 26) as u8) as char;
            let s = c.to_string().repeat(((n - 1) / 26 + 1) as usize);
            if format == "upperLetter" { s.to_uppercase() } else { s }
        }
        "lowerRoman" => roman(n),
        "upperRoman" => roman(n).to_uppercase(),
        "decimalZero" => format!("{n:02}"),
        "none" => String::new(),
        _ => n.to_string(),
    }
}

/// Bullets are often Symbol / Wingdings private-use characters.
fn bullet_char(c: char) -> char {
    match c as u32 {
        0xF0B7 | 0xF09F | 0x00B7 => '\u{2022}',
        0xF0A7 | 0xF06E => '\u{25AA}',
        0xF0D8 | 0xF0E0 => '\u{27A2}',
        0xF076 => '\u{2756}',
        0xF0FC => '\u{2713}',
        0xF0A8 | 0xF071 => '\u{25A1}',
        0xF02D => '-',
        0xE000..=0xF8FF => '\u{2022}',
        _ => c,
    }
}

// MARK: - Content model

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Page,
    Total,
}

enum Piece {
    Text(String, RunProps),
    Image { data: Vec<u8>, width: f32, height: f32, props: RunProps },
    Field(Field, RunProps),
}

struct Para {
    /// From a text box or other floating shape.
    floating: bool,
    props: ParaProps,
    mark: RunProps,
    style: Option<String>,
    pieces: Vec<Piece>,
}

struct Cell {
    span: usize,
    blocks: Vec<Block>,
}

struct Table {
    grid: Vec<f32>,
    indent: f32,
    border: &'static str,
    rows: Vec<Vec<Cell>>,
}

enum Block {
    Para(Para),
    Table(Table),
}

struct Background {
    data: Vec<u8>,
    /// Position and size on the page, in points.
    rect: [f32; 4],
}

// MARK: - Reader

struct Package {
    files: HashMap<String, Vec<u8>>,
}

impl Package {
    fn open(bytes: &[u8]) -> Result<Package, String> {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "geçerli bir Word (.docx) dosyası değil.".to_string())?;
        let mut files = HashMap::new();
        let mut total = 0usize;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().to_string();
            let mut data = Vec::new();
            f.by_ref().take(512 << 20).read_to_end(&mut data).map_err(|e| e.to_string())?;
            total += data.len();
            if total > 1 << 30 {
                return Err("dosya çok büyük.".into());
            }
            files.insert(name, data);
        }
        if !files.contains_key("word/document.xml") {
            return Err("geçerli bir Word (.docx) dosyası değil (word/document.xml yok).".into());
        }
        Ok(Package { files })
    }

    fn text(&self, name: &str) -> Option<String> {
        self.files.get(name).map(|d| String::from_utf8_lossy(d).into_owned())
    }

    /// Relationship id → target path inside the package, for part `name`.
    fn rels(&self, name: &str) -> HashMap<String, String> {
        let (dir, file) = name.rsplit_once('/').unwrap_or(("", name));
        let mut out = HashMap::new();
        let Some(xml) = self.text(&format!("{dir}/_rels/{file}.rels")) else { return out };
        let Ok(doc) = Doc::parse(&xml) else { return out };
        for r in doc.root_element().children().filter(Node::is_element) {
            if attr(r, "TargetMode") == Some("External") {
                continue;
            }
            let (Some(id), Some(target)) = (attr(r, "Id"), attr(r, "Target")) else { continue };
            let path = if let Some(abs) = target.strip_prefix('/') { abs.to_string() } else { normalize(&format!("{dir}/{target}")) };
            out.insert(id.to_string(), path);
        }
        out
    }
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = vec![];
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn parse_xml(xml: &str) -> Option<Doc<'_>> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    Doc::parse_with_options(xml, options).ok()
}

/// Image formats both renderers can draw.
fn drawable(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG") || data.starts_with(&[0xFF, 0xD8]) || data.starts_with(b"GIF8") || data.starts_with(b"BM") || data.starts_with(b"II*\0") || data.starts_with(b"MM\0*")
}

struct Reader<'p> {
    pkg: &'p Package,
    styles: Styles,
    numbering: Numbering,
    theme: Theme,
    default_tab: f32,
    /// Footnote id → content, and the order they were referenced in.
    footnotes: HashMap<String, Vec<Block>>,
    used_notes: Vec<String>,
    page: [f32; 2],
    margins: [f32; 4],
    background: Option<Background>,
}

/// State while reading one part (body, header, footer, footnotes).
struct Ctx {
    rels: HashMap<String, String>,
    header_footer: bool,
    floating: bool,
}

/// Open complex fields: the instruction and whether its result is being replaced.
struct FieldState {
    instr: String,
    separated: bool,
    replaced: bool,
}

impl<'p> Reader<'p> {
    fn new(pkg: &'p Package) -> Reader<'p> {
        let mut r = Reader {
            pkg,
            styles: Styles::default(),
            numbering: Numbering::default(),
            theme: Theme::default(),
            default_tab: 36.0,
            footnotes: HashMap::new(),
            used_notes: vec![],
            page: [595.28, 841.89],
            margins: [72.0; 4],
            background: None,
        };
        r.read_theme();
        r.read_styles();
        r.read_numbering();
        if let Some(xml) = pkg.text("word/settings.xml") {
            if let Some(doc) = parse_xml(&xml) {
                if let Some(t) = doc.descendants().find(|n| n.tag_name().name() == "defaultTabStop").and_then(|n| twips(val(n))) {
                    if t > 1.0 {
                        r.default_tab = t;
                    }
                }
            }
        }
        r
    }

    fn read_theme(&mut self) {
        let Some(xml) = self.pkg.text("word/theme/theme1.xml") else { return };
        let Some(doc) = parse_xml(&xml) else { return };
        let latin = |scheme: &str| {
            doc.descendants()
                .find(|n| n.tag_name().name() == scheme)
                .and_then(|n| child(n, "latin"))
                .and_then(|n| attr(n, "typeface"))
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        };
        self.theme.major = latin("majorFont");
        self.theme.minor = latin("minorFont");
    }

    fn read_styles(&mut self) {
        let Some(xml) = self.pkg.text("word/styles.xml") else { return };
        let Some(doc) = parse_xml(&xml) else { return };
        let root = doc.root_element();
        if let Some(d) = child(root, "docDefaults") {
            self.styles.run = RunProps::read(child(d, "rPrDefault").and_then(|n| child(n, "rPr")), &self.theme);
            self.styles.para = ParaProps::read(child(d, "pPrDefault").and_then(|n| child(n, "pPr")));
        }
        for s in children(root, "style") {
            let Some(id) = attr(s, "styleId") else { continue };
            let kind = attr(s, "type").unwrap_or("paragraph");
            let default = matches!(attr(s, "default"), Some("1" | "true"));
            if default && kind == "paragraph" {
                self.styles.default_para = Some(id.to_string());
            }
            if default && kind == "table" {
                self.styles.default_table = Some(id.to_string());
            }
            let style = Style {
                based_on: child(s, "basedOn").and_then(val).map(str::to_string),
                para: ParaProps::read(child(s, "pPr")),
                run: RunProps::read(child(s, "rPr"), &self.theme),
                borders: Borders::read(child(s, "tblPr").and_then(|n| child(n, "tblBorders"))),
            };
            self.styles.styles.insert(id.to_string(), style);
        }
    }

    fn read_numbering(&mut self) {
        let Some(xml) = self.pkg.text("word/numbering.xml") else { return };
        let Some(doc) = parse_xml(&xml) else { return };
        let root = doc.root_element();
        for a in children(root, "abstractNum") {
            let Some(id) = attr(a, "abstractNumId") else { continue };
            let mut levels = vec![];
            for l in children(a, "lvl") {
                levels.push(Level {
                    start: child(l, "start").and_then(val).and_then(|v| v.parse().ok()).unwrap_or(1),
                    format: child(l, "numFmt").and_then(val).unwrap_or("decimal").to_string(),
                    text: child(l, "lvlText").and_then(val).unwrap_or("").to_string(),
                    para: ParaProps::read(child(l, "pPr")),
                    run: RunProps::read(child(l, "rPr"), &self.theme),
                    suffix: match child(l, "suff").and_then(val) {
                        Some("space") => ' ',
                        Some("nothing") => '\0',
                        _ => '\t',
                    },
                });
            }
            self.numbering.abstracts.insert(id.to_string(), levels);
        }
        for n in children(root, "num") {
            let (Some(id), Some(abs)) = (attr(n, "numId"), child(n, "abstractNumId").and_then(val)) else { continue };
            let mut overrides = HashMap::new();
            for o in children(n, "lvlOverride") {
                let level = attr(o, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                if let Some(start) = child(o, "startOverride").and_then(val).and_then(|v| v.parse().ok()) {
                    overrides.insert(level, start);
                }
            }
            self.numbering.nums.insert(id.to_string(), (abs.to_string(), overrides));
        }
    }

    fn read_footnotes(&mut self) {
        let name = "word/footnotes.xml";
        let Some(xml) = self.pkg.text(name) else { return };
        let Some(doc) = parse_xml(&xml) else { return };
        let mut ctx = Ctx { rels: self.pkg.rels(name), header_footer: false, floating: false };
        for f in children(doc.root_element(), "footnote") {
            if matches!(attr(f, "type"), Some("separator" | "continuationSeparator" | "continuationNotice")) {
                continue;
            }
            let Some(id) = attr(f, "id") else { continue };
            let blocks = self.blocks(f, &mut ctx);
            self.footnotes.insert(id.to_string(), blocks);
        }
    }

    // MARK: Blocks

    fn blocks(&mut self, container: Node, ctx: &mut Ctx) -> Vec<Block> {
        let mut out = vec![];
        for n in container.children().filter(Node::is_element) {
            match n.tag_name().name() {
                "p" => self.paragraph(n, ctx, &mut out),
                "tbl" => {
                    if let Some(t) = self.table(n, ctx) {
                        out.push(Block::Table(t));
                    }
                }
                "sdt" => {
                    if let Some(content) = child(n, "sdtContent") {
                        out.extend(self.blocks(content, ctx));
                    }
                }
                "customXml" | "ins" | "moveTo" => out.extend(self.blocks(n, ctx)),
                _ => {}
            }
        }
        out
    }

    fn para_props(&self, p: Option<Node>) -> (ParaProps, RunProps, Option<String>) {
        let ppr = p.and_then(|p| child(p, "pPr"));
        let style = ppr.and_then(|n| child(n, "pStyle")).and_then(val).map(str::to_string).or_else(|| self.styles.default_para.clone());
        let mut props = self.styles.para.clone();
        let mut mark = self.styles.run.clone();
        for s in self.styles.chain(style.as_deref()) {
            props.apply(&s.para);
            mark.apply(&s.run);
        }
        let direct = ParaProps::read(ppr);
        // Numbering indents sit between the style and direct formatting.
        let num = direct.num.clone().or(props.num.clone());
        if let Some(level) = num.as_ref().and_then(|(id, l)| self.numbering.level(id, *l)) {
            props.apply(&level.para);
        }
        props.apply(&direct);
        props.num = num;
        mark.apply(&RunProps::read(ppr.and_then(|n| child(n, "rPr")), &self.theme));
        (props, mark, style)
    }

    fn paragraph(&mut self, p: Node, ctx: &mut Ctx, out: &mut Vec<Block>) {
        let (props, mark, style) = self.para_props(Some(p));
        let mut para = Para { floating: ctx.floating, props, mark, style, pieces: vec![] };
        if let Some((id, level)) = para.props.num.clone().filter(|(id, _)| id != "0") {
            if let Some((label, lvl)) = self.numbering.next(&id, level) {
                if !label.is_empty() {
                    let mut run = para.mark.clone();
                    run.underline = Some(false);
                    run.strike = Some(false);
                    run.background = Some(None);
                    run.apply(&lvl.run);
                    run.hidden = Some(false);
                    let mut text = label;
                    if lvl.suffix != '\0' {
                        text.push(lvl.suffix);
                    }
                    para.pieces.push(Piece::Text(text, run));
                }
            }
        }
        let mut extra = vec![];
        let mut fields = vec![];
        let paragraph_style = para.style.clone();
        self.inline(p, ctx, &mut para, &mut fields, &mut extra, paragraph_style.as_deref());
        out.push(Block::Para(para));
        out.extend(extra);
    }

    fn run_props(&self, r: Node, para_style: Option<&str>) -> RunProps {
        let mut props = self.styles.run.clone();
        for s in self.styles.chain(para_style) {
            props.apply(&s.run);
        }
        let rpr = child(r, "rPr");
        let char_style = rpr.and_then(|n| child(n, "rStyle")).and_then(val);
        for s in self.styles.chain(char_style) {
            props.apply(&s.run);
        }
        props.apply(&RunProps::read(rpr, &self.theme));
        props
    }

    /// Paragraph content: runs, hyperlinks, fields, tracked insertions; text boxes go to `extra`.
    fn inline(&mut self, n: Node, ctx: &mut Ctx, para: &mut Para, fields: &mut Vec<FieldState>, extra: &mut Vec<Block>, style: Option<&str>) {
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "r" => self.run(c, ctx, para, fields, extra, style),
                "hyperlink" | "ins" | "moveTo" | "smartTag" | "customXml" | "bdo" | "dir" => self.inline(c, ctx, para, fields, extra, style),
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.inline(content, ctx, para, fields, extra, style);
                    }
                }
                "fldSimple" => {
                    let instr = attr(c, "instr").unwrap_or("").to_string();
                    let field = special_field(&instr, ctx);
                    if let Some(f) = field {
                        let props = c.descendants().find(|d| d.tag_name().name() == "r").map(|r| self.run_props(r, style)).unwrap_or_else(|| para.mark.clone());
                        para.pieces.push(Piece::Field(f, props));
                    } else {
                        self.inline(c, ctx, para, fields, extra, style);
                    }
                }
                "oMath" | "oMathPara" => {
                    let text: String = c.descendants().filter(|d| d.tag_name().name() == "t").filter_map(|d| d.text()).collect();
                    if !text.is_empty() {
                        para.pieces.push(Piece::Text(text, para.mark.clone()));
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, r: Node, ctx: &mut Ctx, para: &mut Para, fields: &mut Vec<FieldState>, extra: &mut Vec<Block>, style: Option<&str>) {
        let props = self.run_props(r, style);
        if props.hidden == Some(true) {
            return;
        }
        for c in r.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            if name == "fldChar" {
                match attr(c, "fldCharType") {
                    Some("begin") => fields.push(FieldState { instr: String::new(), separated: false, replaced: false }),
                    Some("separate") => {
                        if let Some(f) = fields.last_mut() {
                            f.separated = true;
                            if let Some(kind) = special_field(&f.instr, ctx) {
                                f.replaced = true;
                                para.pieces.push(Piece::Field(kind, props.clone()));
                            }
                        }
                    }
                    Some("end") => {
                        if let Some(f) = fields.pop() {
                            if !f.separated {
                                if let Some(kind) = special_field(&f.instr, ctx) {
                                    para.pieces.push(Piece::Field(kind, props.clone()));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if name == "instrText" {
                if let Some(f) = fields.last_mut() {
                    f.instr.push_str(c.text().unwrap_or(""));
                }
                continue;
            }
            // Inside a field instruction, or a result we replace with a live page number.
            if fields.iter().any(|f| !f.separated || f.replaced) {
                continue;
            }
            match name {
                "t" => push_text(para, c.text().unwrap_or(""), &props),
                "tab" | "ptab" => push_text(para, "\t", &props),
                "br" => match attr(c, "type") {
                    Some("page") => para.pieces.push(Piece::Text("\u{000C}".into(), props.clone())),
                    Some("column") => {}
                    _ => push_text(para, "\u{2028}", &props),
                },
                "cr" => push_text(para, "\u{2028}", &props),
                "noBreakHyphen" => push_text(para, "-", &props),
                "sym" => {
                    let c = attr(c, "char").and_then(|v| u32::from_str_radix(v, 16).ok()).and_then(char::from_u32).map(bullet_char).unwrap_or('\u{2022}');
                    push_text(para, &c.to_string(), &props);
                }
                "drawing" => self.drawing(c, ctx, para, extra, &props),
                "pict" | "object" => self.vml(c, ctx, para, extra, &props),
                "AlternateContent" => {
                    let choice = child(c, "Choice").filter(|ch| ch.descendants().any(|d| d.tag_name().name() == "drawing"));
                    if let Some(branch) = choice.or_else(|| child(c, "Fallback")) {
                        for d in branch.children().filter(Node::is_element) {
                            match d.tag_name().name() {
                                "drawing" => self.drawing(d, ctx, para, extra, &props),
                                "pict" | "object" => self.vml(d, ctx, para, extra, &props),
                                _ => {}
                            }
                        }
                    }
                }
                "footnoteReference" => {
                    if let Some(id) = attr(c, "id") {
                        self.used_notes.push(id.to_string());
                        let mut sup = props.clone();
                        sup.vert = Some(1);
                        push_text(para, &self.used_notes.len().to_string(), &sup);
                    }
                }
                _ => {}
            }
        }
    }

    fn image(&self, rid: Option<&str>, ctx: &Ctx) -> Option<Vec<u8>> {
        let path = ctx.rels.get(rid?)?;
        let data = self.pkg.files.get(path)?;
        drawable(data).then(|| data.clone())
    }

    /// DrawingML pictures (inline or anchored) and text boxes.
    fn drawing(&mut self, d: Node, ctx: &mut Ctx, para: &mut Para, extra: &mut Vec<Block>, props: &RunProps) {
        for frame in d.children().filter(|n| matches!(n.tag_name().name(), "inline" | "anchor")) {
            let extent = child(frame, "extent");
            let size = |name| extent.and_then(|e| attr(e, name)).and_then(|v| v.parse::<f32>().ok()).map(|v| v / 12_700.0).unwrap_or(0.0);
            let (width, height) = (size("cx"), size("cy"));
            for txbx in frame.descendants().filter(|n| n.tag_name().name() == "txbxContent") {
                let saved = std::mem::replace(&mut ctx.floating, true);
                extra.extend(self.blocks(txbx, ctx));
                ctx.floating = saved;
            }
            let Some(blip) = frame.descendants().find(|n| n.tag_name().name() == "blip") else { continue };
            let Some(data) = self.image(attr(blip, "embed"), ctx) else { continue };
            let anchored = frame.tag_name().name() == "anchor";
            if anchored && ctx.header_footer && self.background.is_none() && attr(frame, "behindDoc").is_some_and(|v| v == "1" || v == "true") && width >= self.page[0] * 0.5 {
                let x = position(child(frame, "positionH"), width, self.page[0], self.margins[3], self.page[0] - self.margins[1] - self.margins[3]);
                let y = position(child(frame, "positionV"), height, self.page[1], self.margins[0], self.page[1] - self.margins[0] - self.margins[2]);
                self.background = Some(Background { data, rect: [x, y, width, height] });
                continue;
            }
            para.pieces.push(Piece::Image { data, width, height, props: props.clone() });
        }
    }

    /// Legacy VML pictures (`v:imagedata`) and text boxes.
    fn vml(&mut self, n: Node, ctx: &mut Ctx, para: &mut Para, extra: &mut Vec<Block>, props: &RunProps) {
        for txbx in n.descendants().filter(|d| d.tag_name().name() == "txbxContent") {
            let saved = std::mem::replace(&mut ctx.floating, true);
            extra.extend(self.blocks(txbx, ctx));
            ctx.floating = saved;
        }
        for img in n.descendants().filter(|d| d.tag_name().name() == "imagedata") {
            let Some(data) = self.image(attr(img, "id"), ctx) else { continue };
            let style = img.ancestors().find_map(|a| attr(a, "style")).unwrap_or("");
            let dim = |key: &str| {
                style.split(';').find_map(|kv| {
                    let (k, v) = kv.split_once(':')?;
                    (k.trim() == key).then(|| css_points(v.trim())).flatten()
                })
            };
            para.pieces.push(Piece::Image { data, width: dim("width").unwrap_or(0.0), height: dim("height").unwrap_or(0.0), props: props.clone() });
        }
    }

    // MARK: Tables

    fn table(&mut self, t: Node, ctx: &mut Ctx) -> Option<Table> {
        let tpr = child(t, "tblPr");
        let style = tpr.and_then(|n| child(n, "tblStyle")).and_then(val).map(str::to_string).or_else(|| self.styles.default_table.clone());
        let mut borders = Borders::default();
        for s in self.styles.chain(style.as_deref()) {
            borders.apply(&s.borders);
        }
        borders.apply(&Borders::read(tpr.and_then(|n| child(n, "tblBorders"))));
        let indent = tpr.and_then(|n| child(n, "tblInd")).and_then(|n| twips(attr(n, "w"))).unwrap_or(0.0);
        let mut grid: Vec<f32> = child(t, "tblGrid").map(|g| children(g, "gridCol").filter_map(|c| twips(attr(c, "w"))).collect()).unwrap_or_default();
        let mut rows = vec![];
        for tr in children(t, "tr") {
            let trpr = child(tr, "trPr");
            let before = trpr.and_then(|n| child(n, "gridBefore")).and_then(val).and_then(|v| v.parse().ok()).unwrap_or(0usize);
            let mut cells = vec![];
            if before > 0 {
                cells.push(Cell { span: before, blocks: vec![] });
            }
            for tc in tr.children().filter(Node::is_element) {
                let tc = match tc.tag_name().name() {
                    "tc" => tc,
                    "sdt" => match child(tc, "sdtContent").and_then(|c| child(c, "tc")) {
                        Some(c) => c,
                        None => continue,
                    },
                    _ => continue,
                };
                let tcpr = child(tc, "tcPr");
                let span = tcpr.and_then(|n| child(n, "gridSpan")).and_then(val).and_then(|v| v.parse().ok()).unwrap_or(1usize).max(1);
                // UDF has no vertical merge: continued cells stay empty.
                let continued = tcpr.and_then(|n| child(n, "vMerge")).is_some_and(|v| val(v) != Some("restart"));
                let blocks = if continued { vec![] } else { self.blocks(tc, ctx) };
                cells.push(Cell { span, blocks });
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        if rows.is_empty() {
            return None;
        }
        let columns = rows.iter().map(|r| r.iter().map(|c| c.span).sum::<usize>()).max().unwrap_or(1);
        if grid.len() < columns || grid.iter().any(|w| *w <= 0.0) {
            let width = self.page[0] - self.margins[1] - self.margins[3] - indent;
            grid = vec![width / columns as f32; columns];
        }
        Some(Table { grid, indent, border: borders.kind(), rows })
    }

    // MARK: Document

    fn part(&mut self, name: &str, header_footer: bool) -> Vec<Block> {
        let Some(xml) = self.pkg.text(name) else { return vec![] };
        let Some(doc) = parse_xml(&xml) else { return vec![] };
        let mut ctx = Ctx { rels: self.pkg.rels(name), header_footer, floating: false };
        let root = doc.root_element();
        let container = if header_footer { root } else { child(root, "body").unwrap_or(root) };
        self.blocks(container, &mut ctx)
    }
}

fn special_field(instr: &str, ctx: &Ctx) -> Option<Field> {
    if !ctx.header_footer {
        return None;
    }
    match instr.split_whitespace().next()?.to_ascii_uppercase().as_str() {
        "PAGE" => Some(Field::Page),
        "NUMPAGES" | "SECTIONPAGES" => Some(Field::Total),
        _ => None,
    }
}

fn push_text(para: &mut Para, text: &str, props: &RunProps) {
    if text.is_empty() {
        return;
    }
    let text = if props.caps == Some(true) { upper(text, props.turkish.unwrap_or(false)) } else { text.to_string() };
    if let Some(Piece::Text(last, p)) = para.pieces.last_mut() {
        if same_look(p, props) {
            last.push_str(&text);
            return;
        }
    }
    para.pieces.push(Piece::Text(text, props.clone()));
}

fn same_look(a: &RunProps, b: &RunProps) -> bool {
    a.font == b.font && a.size == b.size && a.bold == b.bold && a.italic == b.italic && a.underline == b.underline && a.strike == b.strike && a.color == b.color && a.background == b.background && a.vert == b.vert
}

fn upper(text: &str, turkish: bool) -> String {
    text.chars()
        .flat_map(|c| match (turkish, c) {
            (true, 'i') => vec!['İ'],
            (true, 'ı') => vec!['I'],
            _ => c.to_uppercase().collect(),
        })
        .collect()
}

fn css_points(v: &str) -> Option<f32> {
    let (num, unit) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len()));
    let n: f32 = num.trim().parse().ok()?;
    Some(match unit {
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "px" => n * 0.75,
        "emu" => n / 12_700.0,
        _ => n,
    })
}

/// Page-relative position of an anchored picture along one axis.
fn position(p: Option<Node>, size: f32, page: f32, margin: f32, area: f32) -> f32 {
    let Some(p) = p else { return 0.0 };
    let offset = child(p, "posOffset").and_then(|n| n.text()).and_then(|v| v.trim().parse::<f32>().ok()).map(|v| v / 12_700.0);
    let align = child(p, "align").and_then(|n| n.text()).map(str::trim);
    let relative_to_page = matches!(attr(p, "relativeFrom"), Some("page"));
    let (origin, extent) = if relative_to_page { (0.0, page) } else { (margin, area) };
    match (offset, align) {
        (Some(o), _) => origin + o,
        (None, Some("center")) => origin + (extent - size) / 2.0,
        (None, Some("right" | "bottom" | "outside")) => origin + extent - size,
        _ => origin,
    }
}

// MARK: - UDF writer

/// Java's signed ARGB integer for an opaque colour.
fn argb(rgb: u32) -> i32 {
    (0xFF00_0000u32 | rgb) as i32
}

fn num_text(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn attr_escape(text: &str) -> String {
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
            c => out.push(c),
        }
    }
    out
}

/// Characters UDF's XML can hold (form feeds become a page-break attribute instead).
fn clean(text: &str) -> String {
    text.chars().filter(|c| !((*c as u32) < 0x20 && !matches!(c, '\t' | '\n')) && !matches!(c, '\u{FFFE}' | '\u{FFFF}')).collect()
}

struct Writer {
    text: String,
    units: usize,
    out: String,
    content_width: f32,
    default_tab: f32,
}

impl Writer {
    fn add(&mut self, text: &str) -> (usize, usize) {
        let start = self.units;
        let len = text.encode_utf16().count();
        self.text.push_str(text);
        self.units += len;
        (start, len)
    }

    fn run_attrs(p: &RunProps) -> String {
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
        match p.background.flatten() {
            Some(bg) => write!(a, " background=\"{}\"", argb(bg)).ok(),
            None => write!(a, " background=\"\"").ok(),
        };
        a
    }

    fn paragraph(&mut self, p: &Para, next_style_same: bool, prev_contextual_same: bool, break_before: bool) {
        let props = &p.props;
        let size = p.mark.size.unwrap_or(10.0).max(1.0);
        // Word: first line at left + first, wrapped lines at left. UDF: first line at LeftIndent +
        // FirstLineIndent, wrapped lines at LeftIndent + Hanging.
        let left = props.left.unwrap_or(0.0).max(0.0);
        let first = props.first.unwrap_or(0.0);
        let (udf_left, udf_first, udf_hanging) = if first >= 0.0 { (left, first, 0.0) } else { let l = (left + first).max(0.0); (l, 0.0, left - l) };
        let mut stops: Vec<(f32, u8, u8)> = props.stops();
        if first < 0.0 && !stops.iter().any(|s| (s.0 - left).abs() < 0.5) {
            // Word's hanging indent doubles as a tab stop (list numbers tab to the text).
            stops.push((left, 0, 0));
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        // Past the last explicit stop Word continues on its default grid.
        let last = stops.last().map_or(0.0, |s| s.0);
        let mut k = (last / self.default_tab).floor() + 1.0;
        while k * self.default_tab < self.content_width + self.default_tab && stops.len() < 60 {
            stops.push((k * self.default_tab, 0, 0));
            k += 1.0;
        }
        let tabset: Vec<String> = stops
            .iter()
            .filter(|s| s.0 - udf_left > 0.25)
            .map(|(pos, a, l)| format!("{}:{a}:{l}", num_text(pos - udf_left)))
            .collect();
        let line = match props.line {
            Some(Line::Auto(k)) => k - 1.0,
            Some(Line::Exact(pt)) => pt / (size * 1.15) - 1.0,
            Some(Line::AtLeast(pt)) => (pt / (size * 1.15)).max(1.0) - 1.0,
            None => 0.0,
        };
        let contextual = props.contextual == Some(true);
        let before = if contextual && prev_contextual_same { 0.0 } else { props.before.unwrap_or(0.0) };
        let after = if contextual && next_style_same { 0.0 } else { props.after.unwrap_or(0.0) };
        let page_break = props.page_break == Some(true) || break_before || page_break_at_start(p);
        write!(
            self.out,
            "<paragraph Alignment=\"{}\" LeftIndent=\"{}\" RightIndent=\"{}\" FirstLineIndent=\"{}\" Hanging=\"{}\" LineSpacing=\"{}\" SpaceAbove=\"{}\" SpaceBelow=\"{}\" TabSet=\"{}\"{} {}>",
            props.align.unwrap_or(0),
            num_text(udf_left),
            num_text(props.right.unwrap_or(0.0).max(0.0)),
            num_text(udf_first),
            num_text(udf_hanging),
            num_text(line.max(-0.3)),
            num_text(before.max(0.0)),
            num_text(after.max(0.0)),
            tabset.join(","),
            if page_break { " pageBreakBefore=\"true\"" } else { "" },
            Self::run_attrs(&p.mark),
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
                    write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(props)).ok();
                    wrote_text = true;
                }
                Piece::Image { data, width, height, props } => {
                    let (start, len) = self.add(" ");
                    let mut size = String::new();
                    if *width > 0.0 && *height > 0.0 {
                        size = format!(" width=\"{}\" height=\"{}\"", num_text(*width), num_text(*height));
                    }
                    write!(
                        self.out,
                        "<image startOffset=\"{start}\" length=\"{len}\" imageData=\"{}\"{size} {}/>",
                        base64::engine::general_purpose::STANDARD.encode(data),
                        Self::run_attrs(props)
                    )
                    .ok();
                    if last_piece {
                        let (start, len) = self.add("\n");
                        write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(props)).ok();
                    }
                    wrote_text = true;
                }
                Piece::Field(..) => {}
            }
        }
        if !wrote_text || matches!(p.pieces.last(), Some(Piece::Field(..))) {
            let (start, len) = self.add("\n");
            write!(self.out, "<content startOffset=\"{start}\" length=\"{len}\" {}/>", Self::run_attrs(&p.mark)).ok();
        }
        self.out.push_str("</paragraph>");
    }

    fn blocks(&mut self, blocks: &[Block], width: f32) {
        let saved = self.content_width;
        self.content_width = width;
        // A page break inside a paragraph's text moves the following paragraph to a new page.
        let mut pending_break = false;
        for (i, block) in blocks.iter().enumerate() {
            match block {
                Block::Para(p) => {
                    let same = |other: Option<&Block>| matches!(other, Some(Block::Para(o)) if o.style == p.style);
                    let prev_same = i > 0 && same(blocks.get(i - 1)) && matches!(blocks.get(i - 1), Some(Block::Para(o)) if o.props.contextual == Some(true));
                    self.paragraph(p, same(blocks.get(i + 1)), prev_same, pending_break);
                    pending_break = !page_break_at_start(p) && p.pieces.iter().any(|x| matches!(x, Piece::Text(t, _) if t.contains('\u{000C}')));
                }
                Block::Table(t) => {
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
            "<table tableName=\"Tablo\" columnCount=\"{}\" columnSpans=\"{}\" border=\"{}\" LeftIndent=\"{}\" RightIndent=\"{}\">",
            t.grid.len(),
            join(&t.grid),
            t.border,
            num_text(left),
            num_text(right)
        )
        .ok();
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
        self.out.push_str("</table>");
    }
}

/// Whether a page break comes before any visible content of the paragraph.
fn page_break_at_start(p: &Para) -> bool {
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

/// Takes the paragraph holding a PAGE field out of a header/footer and describes it as UDF
/// page-number attributes. Also says whether it came from a floating shape (often a rotated box in
/// the margin), which UDF cannot place; those go to the bottom right instead.
fn page_number(blocks: &mut Vec<Block>) -> Option<(String, bool)> {
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
    let mut flags = 8;
    match p.props.align {
        Some(1) => flags |= 32,
        Some(2) => flags |= 64,
        _ => {}
    }
    if total {
        flags |= 2048;
    }
    let mut a = format!(
        " pageNumber-spec=\"BSP32_{flags}\" pageNumber-foreStr=\"{}\" pageNumber-afterStr=\"{}\" pageNumber-seperator=\"{}\" pageNumber-fontFace=\"{}\" pageNumber-fontSize=\"{}\"",
        attr_escape(fore.trim_start()),
        attr_escape(after.trim_end()),
        attr_escape(&sep),
        attr_escape(font.font.as_deref().unwrap_or("Times New Roman")),
        num_text(if floating { font.size.unwrap_or(10.0).min(11.0) } else { font.size.unwrap_or(10.0) }),
    );
    for (key, value) in [("Bold", font.bold), ("Italic", font.italic), ("Underline", font.underline)] {
        if value == Some(true) {
            write!(a, " pageNumber-font{key}=\"true\"").ok();
        }
    }
    if let Some(c) = font.color.flatten() {
        write!(a, " pageNumber-color=\"{}\"", argb(c)).ok();
    }
    Some((a, floating))
}

fn footnote_blocks(reader: &mut Reader) -> Vec<Block> {
    let ids = std::mem::take(&mut reader.used_notes);
    let mut out = vec![];
    for (n, id) in ids.iter().enumerate() {
        let Some(mut blocks) = reader.footnotes.remove(id) else { continue };
        if out.is_empty() {
            let mut mark = reader.styles.run.clone();
            mark.size = Some(9.0);
            out.push(Block::Para(Para { floating: false, props: ParaProps { before: Some(12.0), ..Default::default() }, mark: mark.clone(), style: None, pieces: vec![Piece::Text("______________________".into(), mark)] }));
        }
        if let Some(Block::Para(first)) = blocks.first_mut() {
            // Word puts the note's own reference mark first; renumber it to match the text.
            let mut sup = first.mark.clone();
            sup.vert = Some(1);
            first.pieces.retain(|p| !matches!(p, Piece::Text(t, _) if t.trim().is_empty()));
            if let Some(Piece::Text(t, _)) = first.pieces.first() {
                if t.chars().all(|c| c.is_ascii_digit()) {
                    first.pieces.remove(0);
                }
            }
            first.pieces.insert(0, Piece::Text(format!("{} ", n + 1), sup));
        }
        out.extend(blocks);
    }
    out
}

/// Converts .docx bytes to a UDF `content.xml`.
pub fn docx_to_content_xml(bytes: &[u8]) -> Result<String, String> {
    let pkg = Package::open(bytes)?;
    let mut reader = Reader::new(&pkg);

    // Page setup and header/footer come from the last section.
    let doc_xml = pkg.text("word/document.xml").unwrap_or_default();
    let doc = parse_xml(&doc_xml).ok_or("belge okunamadı (XML bozuk).")?;
    let body = doc.root_element().children().find(|n| n.tag_name().name() == "body").ok_or("belge gövdesi yok.")?;
    let sect = child(body, "sectPr").or_else(|| body.descendants().filter(|n| n.tag_name().name() == "sectPr").last());
    let mut header_offset = 35.4;
    let mut footer_offset = 35.4;
    if let Some(s) = sect {
        if let Some(sz) = child(s, "pgSz") {
            if let (Some(w), Some(h)) = (twips(attr(sz, "w")), twips(attr(sz, "h"))) {
                reader.page = [w, h];
            }
        }
        if let Some(m) = child(s, "pgMar") {
            let get = |k: &str, d: f32| twips(attr(m, k)).unwrap_or(d);
            reader.margins = [get("top", 72.0).abs(), get("right", 72.0), get("bottom", 72.0).abs(), get("left", 72.0)];
            header_offset = get("header", 35.4);
            footer_offset = get("footer", 35.4);
        }
    }
    let doc_rels = pkg.rels("word/document.xml");
    let reference = |kind: &str| -> Option<String> {
        let s = sect?;
        let refs: Vec<_> = children(s, kind).collect();
        let pick = refs.iter().find(|r| attr(**r, "type") == Some("default")).or_else(|| refs.first())?;
        doc_rels.get(attr(*pick, "id")?).cloned()
    };
    let header_part = reference("headerReference");
    let footer_part = reference("footerReference");

    reader.read_footnotes();
    let mut header = header_part.map(|p| reader.part(&p, true)).unwrap_or_default();
    let mut footer = footer_part.map(|p| reader.part(&p, true)).unwrap_or_default();
    let mut body_blocks = reader.part("word/document.xml", false);
    body_blocks.extend(footnote_blocks(&mut reader));
    let mut header_number = page_number(&mut header);
    let mut footer_number = page_number(&mut footer);
    if header_number.as_ref().is_some_and(|n| n.1) && footer_number.is_none() {
        footer_number = header_number.take();
    }
    let header_number = header_number.map(|n| n.0);
    let footer_number = footer_number.map(|n| n.0);
    // Headers and footers holding only empty paragraphs take no room.
    let empty = |blocks: &Vec<Block>| blocks.iter().all(|b| matches!(b, Block::Para(p) if p.pieces.iter().all(|x| matches!(x, Piece::Text(t, _) if t.trim().is_empty()))));
    if empty(&header) {
        header.clear();
    }
    if empty(&footer) {
        footer.clear();
    }

    let [w, h] = reader.page;
    let [mt, mr, mb, ml] = reader.margins;
    let content_width = (w - ml - mr).max(50.0);
    let mut writer = Writer { text: String::new(), units: 0, out: String::new(), content_width, default_tab: reader.default_tab };
    let mut elements = String::new();
    for (tag, blocks, number) in [("header", &header, &header_number), ("footer", &footer, &footer_number)] {
        if blocks.is_empty() && number.is_none() {
            continue;
        }
        writer.out.clear();
        writer.blocks(blocks, content_width);
        write!(elements, "<{tag} background=\"-1\" foreground=\"-16777216\"{}>{}</{tag}>", number.as_deref().unwrap_or(""), writer.out).ok();
    }
    writer.out.clear();
    if body_blocks.is_empty() {
        body_blocks.push(Block::Para(Para { floating: false, props: ParaProps::default(), mark: reader.styles.run.clone(), style: None, pieces: vec![] }));
    }
    writer.blocks(&body_blocks, content_width);
    elements.push_str(&writer.out);

    let landscape = w > h;
    let (pw, ph) = if landscape { (h, w) } else { (w, h) };
    let mut properties = format!(
        "<pageFormat mediaSizeName=\"1\" leftMargin=\"{}\" rightMargin=\"{}\" topMargin=\"{}\" bottomMargin=\"{}\" paperOrientation=\"{}\" headerFOffset=\"{}\" footerFOffset=\"{}\" pageWidth=\"{}\" pageHeight=\"{}\"/>",
        num_text(ml),
        num_text(mr),
        num_text(mt),
        num_text(mb),
        if landscape { 0 } else { 1 },
        num_text(header_offset),
        num_text(footer_offset),
        num_text(pw),
        num_text(ph),
    );
    if let Some(bg) = &reader.background {
        let [x, y, bw, bh] = bg.rect;
        write!(
            properties,
            "<bgImage bgImageData=\"{}\" bgImageUpMargin=\"{}\" bgImageLeftMargin=\"{}\" bgImageBottomMargin=\"{}\" bgImageRigtMargin=\"{}\"/>",
            base64::engine::general_purpose::STANDARD.encode(&bg.data),
            num_text(y),
            num_text(x),
            num_text(h - y - bh),
            num_text(w - x - bw)
        )
        .ok();
    }
    let font = reader.styles.run.font.clone().unwrap_or_else(|| "Times New Roman".into());
    let size = reader.styles.run.size.unwrap_or(10.0);
    let text = writer.text.replace("]]>", "]]]]><![CDATA[>");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<template format_id=\"1.8\" >\n<content><![CDATA[{text}]]></content>\n<properties>{properties}</properties>\n\
<elements resolver=\"hvl-default\" >{elements}</elements>\n<styles><style name=\"default\" description=\"Geçerli\" family=\"{f}\" size=\"{s}\"/>\
<style name=\"hvl-default\" family=\"{f}\" size=\"{s}\" description=\"Gövde\"/></styles>\n</template>\n",
        f = attr_escape(&font),
        s = num_text(size),
    ))
}

/// Wraps a `content.xml` into a .udf (ZIP) file.
pub fn udf_bytes(content_xml: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("content.xml", options).map_err(|e| e.to_string())?;
    zip.write_all(content_xml.as_bytes()).map_err(|e| e.to_string())?;
    zip.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Reads a .docx file and writes it as a .udf file.
pub fn docx_to_udf(input: &Path, output: &Path) -> Result<(), String> {
    let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = std::fs::read(input).map_err(|_| format!("\"{name}\" açılamadı."))?;
    let xml = docx_to_content_xml(&bytes).map_err(|e| format!("\"{name}\" {e}"))?;
    std::fs::write(output, udf_bytes(&xml)?).map_err(|_| format!("\"{}\" kaydedilemedi.", output.display()))
}
