//! UYAP Doküman Formatı (.udf) → print-ready HTML.
//!
//! A UDF is a ZIP holding `content.xml`: the full text in one CDATA block plus `<elements>` that
//! describe paragraphs and runs by UTF-16 offset, using Java Swing style attributes. The macOS app
//! lays this out with TextKit; here the same rules become HTML that WebView2 prints, so text stays
//! vector and selectable. The letterhead, header, footer and page numbers are printed separately as
//! a "decoration" page per body page and placed underneath with [`underlay`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

use base64::Engine as _;
use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use super::{file_name, EngineError, Result};

type Attrs = HashMap<String, String>;

// MARK: - Model

enum RunKind {
    Text,
    Image { data: String, width: f32, height: f32 },
}

struct Run {
    kind: RunKind,
    start: usize,
    len: usize,
    attrs: Attrs,
}

struct Paragraph {
    attrs: Attrs,
    runs: Vec<Run>,
}

struct Row {
    attrs: Attrs,
    cells: Vec<Vec<Block>>,
}

enum Block {
    Paragraph(Paragraph),
    Table { attrs: Attrs, rows: Vec<Row> },
}

struct PageNumber {
    leading: String,
    trailing: String,
    separator: String,
    total: bool,
    align: &'static str,
    css: String,
    start: i64,
}

pub struct UdfDocument {
    text: Vec<u16>,
    pub page_width: f32,
    pub page_height: f32,
    /// top, right, bottom, left in points
    pub margins: [f32; 4],
    header_offset: f32,
    footer_offset: f32,
    background: Option<(String, [f32; 4])>,
    header: Vec<Block>,
    body: Vec<Block>,
    footer: Vec<Block>,
    header_number: Option<PageNumber>,
    footer_number: Option<PageNumber>,
}

impl UdfDocument {
    pub fn content_width(&self) -> f32 {
        self.page_width - self.margins[1] - self.margins[3]
    }
}

// MARK: - Parsing

fn num(value: Option<&String>, fallback: f32) -> f32 {
    value.and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(fallback)
}

fn attrs_of(node: roxmltree::Node) -> Attrs {
    node.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect()
}

fn resolved(styles: &HashMap<String, Attrs>, name: Option<&String>, depth: usize) -> Attrs {
    let Some(style) = name.and_then(|n| styles.get(n)) else { return Attrs::new() };
    if depth > 12 {
        return Attrs::new();
    }
    let mut out = resolved(styles, style.get("resolver"), depth + 1);
    out.extend(style.clone());
    out
}

fn merged(layers: &[&Attrs]) -> Attrs {
    let mut out = Attrs::new();
    for layer in layers {
        out.extend((*layer).clone());
    }
    out
}

pub fn parse_file(path: &Path) -> Result<UdfDocument> {
    let name = file_name(path);
    let file = std::fs::File::open(path).map_err(|_| EngineError::CannotOpen(name.clone()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| EngineError::Message(format!("\"{name}\" geçerli bir UDF değil.")))?;
    let mut xml = String::new();
    archive
        .by_name("content.xml")
        .map_err(|_| EngineError::Message(format!("\"{name}\" geçerli bir UDF değil (content.xml yok).")))?
        .take(256 << 20)
        .read_to_string(&mut xml)
        .map_err(|_| EngineError::CannotOpen(name.clone()))?;
    parse_xml(&xml).map_err(|e| EngineError::Message(format!("\"{name}\" okunamadı: {e}")))
}

pub fn parse_xml(xml: &str) -> std::result::Result<UdfDocument, String> {
    let options = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let tree = roxmltree::Document::parse_with_options(xml, options).map_err(|e| e.to_string())?;
    let root = tree.root_element();
    let child = |name: &str| root.children().find(|n| n.has_tag_name(name));

    let text: String = child("content").map(|n| n.children().filter_map(|c| c.text()).collect()).unwrap_or_default();
    let mut doc = UdfDocument {
        text: text.encode_utf16().collect(),
        page_width: 595.28,
        page_height: 841.89,
        margins: [42.5; 4],
        header_offset: 20.0,
        footer_offset: 20.0,
        background: None,
        header: vec![],
        body: vec![],
        footer: vec![],
        header_number: None,
        footer_number: None,
    };

    let mut styles: HashMap<String, Attrs> = HashMap::new();
    if let Some(node) = child("styles") {
        for style in node.children().filter(|n| n.has_tag_name("style")) {
            let a = attrs_of(style);
            if let Some(n) = a.get("name").cloned() {
                styles.insert(n, a);
            }
        }
    }

    if let Some(props) = child("properties") {
        if let Some(format) = props.children().find(|n| n.has_tag_name("pageFormat")) {
            let a = attrs_of(format);
            if a.get("paperOrientation").map(String::as_str) == Some("0") {
                std::mem::swap(&mut doc.page_width, &mut doc.page_height);
            }
            doc.margins = [num(a.get("topMargin"), 42.5), num(a.get("rightMargin"), 42.5), num(a.get("bottomMargin"), 42.5), num(a.get("leftMargin"), 42.5)];
            doc.header_offset = num(a.get("headerFOffset"), 20.0);
            doc.footer_offset = num(a.get("footerFOffset"), 20.0);
        }
        if let Some(bg) = props.children().find(|n| n.has_tag_name("bgImage")) {
            let a = attrs_of(bg);
            if let Some(data) = a.get("bgImageData").map(|d| clean_base64(d)).filter(|d| !d.is_empty()) {
                let insets = [num(a.get("bgImageUpMargin"), 0.0), num(a.get("bgImageRigtMargin"), 0.0), num(a.get("bgImageBottomMargin"), 0.0), num(a.get("bgImageLeftMargin"), 0.0)];
                doc.background = Some((data, insets));
            }
        }
    }

    let Some(elements) = child("elements") else { return Ok(doc) };
    let base_name = elements.attribute("resolver").unwrap_or("hvl-default").to_string();
    let base = merged(&[&resolved(&styles, Some(&"default".to_string()), 0), &resolved(&styles, Some(&base_name), 0)]);

    for node in elements.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "header" => {
                doc.header = blocks(node, &base, &styles);
                doc.header_number = page_number(&attrs_of(node));
            }
            "footer" => {
                doc.footer = blocks(node, &base, &styles);
                doc.footer_number = page_number(&attrs_of(node));
            }
            "paragraph" => doc.body.push(Block::Paragraph(paragraph(node, &base, &styles))),
            "table" => doc.body.push(table(node, &base, &styles)),
            _ => {}
        }
    }
    Ok(doc)
}

fn clean_base64(data: &str) -> String {
    data.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect()
}

fn blocks(node: roxmltree::Node, base: &Attrs, styles: &HashMap<String, Attrs>) -> Vec<Block> {
    node.children()
        .filter_map(|n| match n.tag_name().name() {
            "paragraph" => Some(Block::Paragraph(paragraph(n, base, styles))),
            "table" => Some(table(n, base, styles)),
            _ => None,
        })
        .collect()
}

fn paragraph(node: roxmltree::Node, base: &Attrs, styles: &HashMap<String, Attrs>) -> Paragraph {
    let own = attrs_of(node);
    let attrs = merged(&[base, &resolved(styles, own.get("resolver"), 0), &own]);
    let mut runs = Vec::new();
    for child in node.children().filter(|n| n.is_element()) {
        let a = attrs_of(child);
        let (Some(start), Some(len)) = (a.get("startOffset").and_then(|v| v.parse().ok()), a.get("length").and_then(|v| v.parse().ok())) else { continue };
        let run_attrs = merged(&[&attrs, &resolved(styles, a.get("resolver"), 0), &a]);
        let kind = match (child.tag_name().name(), a.get("imageData")) {
            ("image", Some(data)) if !data.is_empty() => RunKind::Image { data: clean_base64(data), width: num(a.get("width"), 0.0), height: num(a.get("height"), 0.0) },
            _ => RunKind::Text,
        };
        runs.push(Run { kind, start, len, attrs: run_attrs });
    }
    Paragraph { attrs, runs }
}

fn table(node: roxmltree::Node, base: &Attrs, styles: &HashMap<String, Attrs>) -> Block {
    let rows = node
        .children()
        .filter(|n| n.has_tag_name("row"))
        .map(|row| Row {
            attrs: attrs_of(row),
            cells: row.children().filter(|n| n.has_tag_name("cell")).map(|cell| blocks(cell, base, styles)).collect(),
        })
        .collect();
    Block::Table { attrs: attrs_of(node), rows }
}

fn page_number(a: &Attrs) -> Option<PageNumber> {
    let flags: i64 = a.get("pageNumber-spec")?.rsplit('_').next()?.parse().ok()?;
    if flags & 8 == 0 {
        return None;
    }
    let size = num(a.get("pageNumber-fontSize"), 11.0);
    let mut css = format!("font-family:{};font-size:{size}pt;color:{};", font_family(a.get("pageNumber-fontFace").map(String::as_str).unwrap_or("Arial")), color(a.get("pageNumber-color")).unwrap_or_else(|| "#000".into()));
    if a.get("pageNumber-fontBold").map(String::as_str) == Some("true") {
        css.push_str("font-weight:700;");
    }
    if a.get("pageNumber-fontItalic").map(String::as_str) == Some("true") {
        css.push_str("font-style:italic;");
    }
    if a.get("pageNumber-fontUnderline").map(String::as_str) == Some("true") {
        css.push_str("text-decoration:underline;");
    }
    Some(PageNumber {
        leading: a.get("pageNumber-foreStr").cloned().unwrap_or_default(),
        trailing: a.get("pageNumber-afterStr").cloned().unwrap_or_default(),
        separator: a.get("pageNumber-seperator").cloned().unwrap_or_else(|| "/".into()),
        total: flags & 2048 != 0,
        // Bits of BSP32_n: 8 shows the number, 32 centres it, 64 right-aligns it, 2048 adds "/total".
        align: if flags & 32 != 0 { "center" } else if flags & 64 != 0 { "right" } else { "left" },
        css,
        start: a.get("pageNumber-pageStartNumStr").and_then(|v| v.parse().ok()).unwrap_or(1),
    })
}

// MARK: - HTML

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn font_family(family: &str) -> String {
    match family {
        "Dialog" | "SansSerif" | "Arial" => "Arial, Helvetica, sans-serif".into(),
        "Serif" | "Times New Roman" => "'Times New Roman', Times, serif".into(),
        "Monospaced" | "DialogInput" => "'Courier New', monospace".into(),
        "Lucida Grande" => "'Lucida Grande', 'Segoe UI', sans-serif".into(),
        "Bookman Old Style" => "'Bookman Old Style', Georgia, serif".into(),
        "Segoe Script" => "'Segoe Script', cursive".into(),
        other => format!("'{}', 'Times New Roman', serif", other.replace('\'', "")),
    }
}

/// Java colours are signed ARGB integers.
fn color(value: Option<&String>) -> Option<String> {
    let argb = value?.trim().parse::<i64>().ok()? as u32;
    Some(format!("#{:06x}", argb & 0x00FF_FFFF))
}

fn font_css(a: &Attrs) -> String {
    let mut css = format!(
        "font-family:{};font-size:{}pt;",
        font_family(a.get("family").map(String::as_str).unwrap_or("Times New Roman")),
        num(a.get("size"), 12.0).max(1.0)
    );
    if a.get("bold").map(String::as_str) == Some("true") {
        css.push_str("font-weight:700;");
    }
    if a.get("italic").map(String::as_str) == Some("true") {
        css.push_str("font-style:italic;");
    }
    let underline = a.get("underline").map(String::as_str) == Some("true");
    let strike = a.get("strikethrough").map(String::as_str) == Some("true");
    match (underline, strike) {
        (true, true) => css.push_str("text-decoration:underline line-through;"),
        (true, false) => css.push_str("text-decoration:underline;"),
        (false, true) => css.push_str("text-decoration:line-through;"),
        _ => {}
    }
    if let Some(c) = color(a.get("foreground")) {
        write!(css, "color:{c};").ok();
    }
    if let Some(c) = a.get("background").filter(|v| !v.is_empty()).and_then(|v| color(Some(v))) {
        write!(css, "background-color:{c};").ok();
    }
    if a.get("superscript").map(String::as_str) == Some("true") {
        css.push_str("vertical-align:super;font-size:smaller;");
    }
    if a.get("subscript").map(String::as_str) == Some("true") {
        css.push_str("vertical-align:sub;font-size:smaller;");
    }
    css
}

/// UYAP list numbering. Each level's look comes from the paragraph's own `SecListTypeLevelN`
/// (first one seen per list), level 1 falls back to NumberType/BulletType, deeper levels to the
/// editor's defaults. Counters run per ListId; a shallower item restarts deeper levels and
/// `NumberSetted=n` makes the next item n+1.
#[derive(Default)]
struct ListPlan {
    secondary: HashMap<String, HashMap<u32, String>>,
    own: HashMap<String, HashMap<u32, String>>,
    counters: HashMap<String, std::collections::BTreeMap<u32, i64>>,
}

fn is_true(a: &Attrs, key: &str) -> bool {
    a.get(key).map(String::as_str) == Some("true")
}

fn list_id(a: &Attrs) -> Option<String> {
    (is_true(a, "Numbered") || is_true(a, "Bulleted")).then(|| a.get("ListId").cloned().unwrap_or_else(|| "0".into()))
}

fn list_level(a: &Attrs) -> u32 {
    a.get("ListLevel").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 9)
}

impl ListPlan {
    fn new(blocks: &[Block]) -> Self {
        let mut plan = Self::default();
        plan.visit(blocks);
        plan
    }

    fn visit(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Paragraph(p) => {
                    let Some(id) = list_id(&p.attrs) else { continue };
                    let level = list_level(&p.attrs);
                    if let Some(kind) = p.attrs.get(&format!("SecListTypeLevel{level}")).filter(|v| !v.is_empty()) {
                        self.secondary.entry(id.clone()).or_default().entry(level).or_insert_with(|| kind.clone());
                    }
                    let own = if is_true(&p.attrs, "Numbered") {
                        p.attrs.get("NumberType").cloned().unwrap_or_else(|| "NUMBER_TYPE_NUMBER_DOT".into())
                    } else {
                        p.attrs.get("BulletType").cloned().unwrap_or_else(|| "BULLET_TYPE_ELLIPSE".into())
                    };
                    self.own.entry(id).or_default().entry(level).or_insert(own);
                }
                Block::Table { rows, .. } => {
                    for row in rows {
                        for cell in &row.cells {
                            self.visit(cell);
                        }
                    }
                }
            }
        }
    }

    fn kind(&self, id: &str, level: u32) -> String {
        if let Some(kind) = self.secondary.get(id).and_then(|m| m.get(&level)) {
            return kind.clone();
        }
        let own = self.own.get(id);
        let first = own
            .and_then(|m| m.get(&1).cloned().or_else(|| m.iter().min_by_key(|(k, _)| **k).map(|(_, v)| v.clone())))
            .unwrap_or_else(|| "NUMBER_TYPE_NUMBER_DOT".into());
        if level == 1 {
            return first;
        }
        let bulleted = own.and_then(|m| m.get(&level)).unwrap_or(&first).starts_with("BULLET");
        if bulleted {
            return if level == 3 { "BULLET_TYPE_RECTANGLE_D" } else { "BULLET_TYPE_RECTANGLE" }.into();
        }
        match level {
            2 => "NUMBER_TYPE_CHAR_SMALL_DOT",
            3 => "NUMBER_TYPE_ROMAN_SMALL_DOT",
            4 => "NUMBER_TYPE_NUMBER_D_PARANTHESE",
            5 => "NUMBER_TYPE_CHAR_SMALL_D_PARANTHESE",
            6 => "NUMBER_TYPE_ROMAN_SMALL_D_PARANTHESE",
            _ => "NUMBER_TYPE_NUMBER_DOT",
        }
        .into()
    }

    /// The label for the next item of this paragraph's list, and whether it is a bullet.
    fn next(&mut self, a: &Attrs) -> Option<(String, bool)> {
        let id = list_id(a)?;
        let level = list_level(a);
        let kind = self.kind(&id, level);
        let counter = self.counters.entry(id).or_default();
        counter.retain(|k, _| *k <= level);
        if let Some(n) = a.get("NumberSetted").and_then(|v| v.trim().parse::<f64>().ok()) {
            counter.insert(level, n as i64);
        }
        let n = counter.entry(level).or_insert(0);
        *n += 1;
        Some(list_label(&kind, *n))
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

/// Label text for a list item; letters follow the 29-letter Turkish alphabet, then double (aa, bb…).
fn list_label(kind: &str, number: i64) -> (String, bool) {
    if let Some(bullet) = kind.strip_prefix("BULLET_TYPE_") {
        let symbol = match bullet {
            "ARROW" => "\u{27A2}",
            "DIAMOND" | "DIAMOND_2" => "\u{25C6}",
            "RECTANGLE" => "\u{25AA}",
            "RECTANGLE_D" => "\u{25AB}",
            "TRIANGLE" => "\u{25B6}",
            _ => "\u{2022}",
        };
        return (symbol.into(), true);
    }
    let n = number.max(1);
    let value = if kind.starts_with("NUMBER_TYPE_CHAR") {
        let letters: Vec<char> = if kind.contains("CHAR_BIG") { "ABCÇDEFGĞHIİJKLMNOÖPRSŞTUÜVYZ" } else { "abcçdefgğhıijklmnoöprsştuüvyz" }.chars().collect();
        let len = letters.len() as i64;
        letters[((n - 1) % len) as usize].to_string().repeat(((n - 1) / len + 1) as usize)
    } else if kind.starts_with("NUMBER_TYPE_ROMAN") {
        if kind.contains("ROMAN_BIG") { roman(n).to_uppercase() } else { roman(n) }
    } else {
        n.to_string()
    };
    let tail = kind.rsplit("_TYPE_").next().unwrap_or("");
    let text = if tail.ends_with("D_PARANTHESE") {
        format!("({value})")
    } else if tail.contains("PARANTHESE") || tail.contains("PARANTEZ") {
        format!("{value})")
    } else if tail.ends_with("TRE") {
        format!("{value}-")
    } else {
        format!("{value}.")
    };
    (text, false)
}

fn spans(value: Option<&String>) -> Vec<f32> {
    value.map(|s| s.split(',').filter_map(|v| v.trim().parse::<f32>().ok()).filter(|v| *v > 0.0).collect()).unwrap_or_default()
}

impl UdfDocument {
    fn slice(&self, start: usize, len: usize) -> String {
        let start = start.min(self.text.len());
        let end = (start + len).min(self.text.len());
        // Private Use Area characters show as empty boxes in standard fonts; UYAP shows nothing.
        String::from_utf16_lossy(&self.text[start..end])
            .chars()
            .filter(|c| !('\u{E000}'..='\u{F8FF}').contains(c) && *c != '\r')
            .collect()
    }

    fn paragraph_html(&self, p: &Paragraph, lists: &mut ListPlan, out: &mut String) {
        let a = &p.attrs;
        let left = num(a.get("LeftIndent"), 0.0).max(0.0);
        let right = num(a.get("RightIndent"), 0.0).max(0.0);
        let first = num(a.get("FirstLineIndent"), 0.0);
        let hanging = num(a.get("Hanging"), 0.0).max(0.0);
        let extra = num(a.get("LineSpacing"), 0.0);
        let line = 1.15 * (1.0 + if extra < -0.3 { 0.0 } else { extra }).max(0.7);
        let align = match a.get("Alignment").map(String::as_str) {
            Some("1") => "center",
            Some("2") => "right",
            Some("3") => "justify",
            _ => "left",
        };
        // TabSet = "position:alignment:leader,…" measured from LeftIndent. Alignment 0 left, 1 right,
        // 2 centre, 4 decimal, 5 bar; leader 1 dots, 2 dashes, 3 underscore, 4 heavy line.
        let stops: Vec<String> = a
            .get("TabSet")
            .map(|s| {
                s.split(',')
                    .filter_map(|t| {
                        let mut parts = t.split(':').map(str::trim);
                        let position = parts.next()?.parse::<f32>().ok()?;
                        let alignment = parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                        let leader = parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
                        Some(format!("{:.2}:{alignment}:{leader}", position + left))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let default_interval = if stops.is_empty() { 72.0 } else { 5.0 };

        // UYAP draws list numbers right-aligned about 6 pt before the text and bullets 18 pt before it.
        let label = lists.next(a);
        let size = num(a.get("size"), 12.0).max(1.0);
        let mut text_start = (left + first).max(0.0);
        if let Some((text, _)) = &label {
            text_start = text_start.max(text.chars().count() as f32 * size * 0.6 + 8.0);
        }
        write!(
            out,
            "<p data-tabs=\"{}\" data-def=\"{default_interval}\" style=\"position:relative;text-align:{align};padding-left:{:.2}pt;padding-right:{right:.2}pt;text-indent:{:.2}pt;margin:{:.2}pt 0 {:.2}pt 0;line-height:{line:.4};{}\">",
            stops.join(","),
            left + hanging,
            text_start - (left + hanging),
            num(a.get("SpaceAbove"), 0.0).max(0.0),
            num(a.get("SpaceBelow"), 0.0).max(0.0),
            font_css(a)
        )
        .ok();
        if let Some((text, bullet)) = label {
            let placement = if bullet {
                format!("left:{:.2}pt", (text_start - 18.0).max(0.0))
            } else {
                format!("left:0;width:{:.2}pt;text-align:right", (text_start - 6.0).max(0.0))
            };
            write!(out, "<span class=\"lbl\" style=\"position:absolute;top:0;text-indent:0;white-space:nowrap;text-decoration:none;{placement}\">{}</span>", esc(&text)).ok();
        }
        let mut visible = false;
        for run in &p.runs {
            match &run.kind {
                RunKind::Image { data, width, height } => {
                    let style = if *width > 0.0 && *height > 0.0 { format!("width:{width}pt;height:{height}pt;") } else { String::new() };
                    write!(out, "<img alt=\"\" style=\"{style}\" src=\"data:image/png;base64,{data}\">").ok();
                    visible = true;
                }
                RunKind::Text => {
                    let text = self.slice(run.start, run.len);
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        continue;
                    }
                    write!(out, "<span style=\"{}\">", font_css(&run.attrs)).ok();
                    for (i, piece) in text.split('\t').enumerate() {
                        if i > 0 {
                            // A figure space widened with letter-spacing: underlines continue across it, as in UYAP.
                            out.push_str("<span class=\"tab\">\u{2007}</span>");
                        }
                        out.push_str(&esc(piece).replace('\n', "<br>"));
                    }
                    out.push_str("</span>");
                    visible = true;
                }
            }
        }
        if !visible {
            out.push('\u{200B}');
        }
        out.push_str("</p>");
    }

    fn blocks_html(&self, blocks: &[Block], width: f32) -> String {
        let mut out = String::new();
        let mut lists = ListPlan::new(blocks);
        self.blocks_into(blocks, width, &mut lists, &mut out);
        out
    }

    fn blocks_into(&self, blocks: &[Block], width: f32, lists: &mut ListPlan, out: &mut String) {
        for block in blocks {
            match block {
                Block::Paragraph(p) => self.paragraph_html(p, lists, out),
                Block::Table { attrs, rows } => self.table_html(attrs, rows, width, lists, out),
            }
        }
    }

    /// `columnSpans` are ratios of the table width, per table and optionally per row (merged cells);
    /// all rows share one grid of column boundaries. Border types: borderCell (every line),
    /// borderTable (outline only), borderRow (outline and row lines), borderNone.
    fn table_html(&self, attrs: &Attrs, rows: &[Row], width: f32, lists: &mut ListPlan, out: &mut String) {
        if rows.is_empty() {
            return;
        }
        let left = num(attrs.get("LeftIndent"), 0.0).max(0.0);
        let right = num(attrs.get("RightIndent"), 0.0).max(0.0);
        let table_width = (width - left - right).max(40.0);
        let table_spans = spans(attrs.get("columnSpans"));
        let fractions: Vec<Vec<f32>> = rows
            .iter()
            .map(|row| {
                let count = row.cells.len().max(1);
                let mut ratios = spans(row.attrs.get("columnSpans"));
                if ratios.len() != count {
                    ratios = if table_spans.len() == count { table_spans.clone() } else { vec![1.0; count] };
                }
                let total: f32 = ratios.iter().sum();
                ratios.into_iter().map(|r| r / total).collect()
            })
            .collect();
        const SCALE: f32 = 10_000.0;
        let mut bounds = std::collections::BTreeSet::from([0i64, SCALE as i64]);
        for row in &fractions {
            let mut sum = 0.0;
            for f in row {
                sum += f;
                bounds.insert((sum * SCALE).round() as i64);
            }
        }
        let grid: Vec<i64> = bounds.into_iter().collect();
        let widths: Vec<f32> = grid.windows(2).map(|w| (w[1] - w[0]) as f32 / SCALE * table_width).collect();
        let border = attrs.get("border").map(String::as_str).unwrap_or("borderCell");
        let line = "0.5pt solid #000";

        write!(out, "<table style=\"width:{table_width:.2}pt;margin-left:{left:.2}pt\"><colgroup>").ok();
        for w in &widths {
            write!(out, "<col style=\"width:{w:.2}pt\">").ok();
        }
        out.push_str("</colgroup>");
        let last_row = rows.len() - 1;
        for (r, row) in rows.iter().enumerate() {
            out.push_str("<tr>");
            let mut sum = 0.0;
            for (c, cell) in row.cells.iter().enumerate() {
                let start = grid.iter().position(|g| *g == (sum * SCALE).round() as i64).unwrap_or(0);
                sum += fractions[r].get(c).copied().unwrap_or(0.0);
                let end = grid.iter().position(|g| *g == (sum * SCALE).round() as i64).unwrap_or(grid.len() - 1).max(start + 1);
                let edge = |on: bool| if on { line } else { "none" };
                let all = border == "borderCell";
                let outline = border == "borderTable" || border == "borderRow";
                write!(
                    out,
                    "<td colspan=\"{}\" style=\"border-top:{};border-bottom:{};border-left:{};border-right:{}\">",
                    end - start,
                    edge(all || border == "borderRow" || (border == "borderTable" && r == 0)),
                    edge(all || border == "borderRow" || (border == "borderTable" && r == last_row)),
                    edge(all || (outline && start == 0)),
                    edge(all || (outline && end == grid.len() - 1)),
                )
                .ok();
                if cell.is_empty() {
                    out.push_str("<p>\u{200B}</p>");
                } else {
                    self.blocks_into(cell, widths[start..end].iter().sum(), lists, out);
                }
                out.push_str("</td>");
            }
            out.push_str("</tr>");
        }
        out.push_str("</table>");
    }
}

const BASE_CSS: &str = "html,body{margin:0;padding:0;background:transparent}\
p{margin:0;white-space:pre-wrap;overflow-wrap:break-word;orphans:1;widows:1}\
.tab{white-space:pre}\
img{vertical-align:baseline}\
table{border-collapse:collapse;table-layout:fixed}\
td{vertical-align:top;padding:1pt}";

/// Widens every tab so the following text lands on the paragraph's next tab stop (left, right,
/// centre or decimal), and draws the stop's leader (dots, dashes, line) as a styled underline.
const TAB_SCRIPT: &str = "function layoutTabs(root){for(const p of root.querySelectorAll('p[data-tabs]')){\
const stops=p.dataset.tabs?p.dataset.tabs.split(',').map(s=>{const v=s.split(':').map(Number);return{l:v[0],a:v[1]||0,d:v[2]||0};}):[];\
const def=Number(p.dataset.def)||72;const left=p.getBoundingClientRect().left;const tabs=[...p.querySelectorAll('.tab')];\
tabs.forEach((t,i)=>{t.style.letterSpacing='0';const r=t.getBoundingClientRect();const x=(r.left-left)*0.75,w0=r.width*0.75;\
const s=stops.find(s=>s.l>x+0.5);let target;if(s){let seg=0;if(s.a===1||s.a===2||s.a===4){const g=document.createRange();\
g.setStartAfter(t);const n=tabs[i+1];if(n)g.setEndBefore(n);else g.setEndAfter(p.lastChild);seg=g.getBoundingClientRect().width*0.75;}\
target=s.a===2?s.l-seg/2:(s.a===1||s.a===4)?s.l-seg:s.l;target=Math.max(target,x+w0);\
if(s.d){t.style.textDecorationLine='underline';t.style.textDecorationStyle=s.d===1?'dotted':s.d===2?'dashed':'solid';\
t.style.textDecorationThickness=s.d===4?'2px':s.d===1?'1.5px':'1px';}}\
else target=(Math.floor(x/def+1e-6)+1)*def;t.style.letterSpacing=(target-x-w0)+'pt';});}}";

/// Body pages. A script measures the header and footer, then sets the page margins so the text
/// flows between them. The resulting margins are also published in `location.hash` for the printer.
pub fn body_html(doc: &UdfDocument) -> String {
    let width = doc.content_width();
    let [mt, mr, mb, ml] = doc.margins;
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{BASE_CSS}\
@page{{size:{w}pt {h}pt;margin:{mt}pt {mr}pt {mb}pt {ml}pt}}body{{width:{width:.2}pt}}\
#measure{{position:absolute;left:0;top:0;visibility:hidden;width:{width:.2}pt}}</style></head><body>\
<div id=\"measure\"><div id=\"mh\">{header}</div><div id=\"mf\">{footer}</div></div>{body}\
<script>(()=>{{{TAB_SCRIPT}layoutTabs(document);\
const hh={has_header}?document.getElementById('mh').getBoundingClientRect().height*0.75:0;\
const fh={has_footer}?document.getElementById('mf').getBoundingClientRect().height*0.75:0;\
document.getElementById('measure').remove();\
const mt=Math.max({mt},{ho}+hh),mb=Math.max({mb},{fo}+fh);\
const s=document.createElement('style');s.textContent='@page{{size:{w}pt {h}pt;margin:'+mt+'pt {mr}pt '+mb+'pt {ml}pt}}';\
document.head.appendChild(s);location.hash='ready='+mt.toFixed(3)+','+mb.toFixed(3);}})();</script></body></html>",
        w = doc.page_width,
        h = doc.page_height,
        header = doc.blocks_html(&doc.header, width),
        footer = doc.blocks_html(&doc.footer, width),
        body = doc.blocks_html(&doc.body, width),
        has_header = !doc.header.is_empty(),
        has_footer = !doc.footer.is_empty(),
        ho = doc.header_offset,
        fo = doc.footer_offset,
    )
}

/// One decoration page per body page: letterhead image, header, footer and page number.
pub fn decoration_html(doc: &UdfDocument, pages: usize) -> String {
    let width = doc.content_width();
    let [_, _, _, ml] = doc.margins;
    let (w, h) = (doc.page_width, doc.page_height);
    let header = doc.blocks_html(&doc.header, width);
    let footer = doc.blocks_html(&doc.footer, width);
    let mut out = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>{BASE_CSS}@page{{size:{w}pt {h}pt;margin:0}}\
.pg{{position:relative;width:{w}pt;height:{hh:.2}pt;overflow:hidden;break-after:page}}.pg:last-child{{break-after:auto}}\
.bg{{position:absolute}}.hdr,.ftr,.pn{{position:absolute;left:{ml}pt;width:{width:.2}pt}}</style></head><body>",
        hh = h - 0.6
    );
    for page in 0..pages.max(1) {
        out.push_str("<div class=\"pg\">");
        if let Some((data, [t, r, b, l])) = &doc.background {
            write!(out, "<img class=\"bg\" alt=\"\" style=\"left:{l}pt;top:{t}pt;width:{:.2}pt;height:{:.2}pt\" src=\"data:image/png;base64,{data}\">", w - l - r, h - t - b).ok();
        }
        if !doc.header.is_empty() {
            write!(out, "<div class=\"hdr\" style=\"top:{}pt\">{header}</div>", doc.header_offset).ok();
        }
        if !doc.footer.is_empty() {
            write!(out, "<div class=\"ftr\" style=\"bottom:{}pt\">{footer}</div>", doc.footer_offset).ok();
        }
        for (number, top) in [(&doc.header_number, true), (&doc.footer_number, false)] {
            if let Some(n) = number {
                let current = n.start + page as i64;
                let mut text = format!("{}{current}", n.leading);
                if n.total {
                    write!(text, "{}{}", n.separator, n.start + pages as i64 - 1).ok();
                }
                text.push_str(&n.trailing);
                let place = if top { format!("top:{}pt", doc.header_offset) } else { format!("bottom:{}pt", doc.footer_offset) };
                write!(out, "<div class=\"pn\" style=\"{place};text-align:{};{}\">{}</div>", n.align, n.css, esc(&text)).ok();
            }
        }
        out.push_str("</div>");
    }
    out.push_str(&format!("<script>(()=>{{{TAB_SCRIPT}layoutTabs(document);location.hash='ready=1';}})();</script></body></html>"));
    out
}

/// Parses the margins published by [`body_html`] in the page URL (`#ready=top,bottom`).
pub fn ready_margins(fragment: &str) -> Option<(f32, f32)> {
    let rest = fragment.trim_start_matches('#').strip_prefix("ready=")?;
    let (top, bottom) = rest.split_once(',')?;
    Some((top.parse().ok()?, bottom.parse().ok()?))
}

// MARK: - Underlay

fn page_content(doc: &Document, page: ObjectId) -> Vec<u8> {
    doc.get_page_content(page).unwrap_or_default()
}

fn page_resources(doc: &Document, page: ObjectId) -> Object {
    let mut current = doc.get_dictionary(page).ok();
    for _ in 0..32 {
        let Some(dict) = current else { break };
        if let Ok(res) = dict.get(b"Resources") {
            return match res {
                Object::Reference(id) => doc.get_object(*id).cloned().unwrap_or(Object::Dictionary(dictionary! {})),
                other => other.clone(),
            };
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok().and_then(|p| doc.get_dictionary(p).ok());
    }
    Object::Dictionary(dictionary! {})
}

/// Draws page *n* of `decoration` underneath page *n* of `body` (letterhead behind the text).
pub fn underlay(body: &mut Document, mut decoration: Document) -> Result<()> {
    decoration.renumber_objects_with(body.max_id + 1);
    let deco_pages = decoration.get_pages();
    let mut forms: HashMap<u32, (ObjectId, Vec<u8>, Object, Vec<f32>)> = HashMap::new();
    for (&n, &id) in &deco_pages {
        let media = decoration
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"MediaBox").ok())
            .and_then(|m| m.as_array().ok())
            .map(|a| a.iter().filter_map(|v| v.as_float().ok()).collect())
            .unwrap_or_else(|| vec![0.0, 0.0, 595.28, 841.89]);
        forms.insert(n, (id, page_content(&decoration, id), page_resources(&decoration, id), media));
    }
    body.max_id = decoration.max_id;
    body.objects.extend(std::mem::take(&mut decoration.objects));

    for (n, page_id) in body.get_pages() {
        let Some((_, content, resources, media)) = forms.remove(&n) else { continue };
        let mut form = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => media.into_iter().map(Object::Real).collect::<Vec<_>>(),
                "Resources" => resources,
            },
            content,
        );
        let _ = form.compress();
        let form_id = body.add_object(form);
        let prefix_id = body.add_object(Stream::new(dictionary! {}, b"q /BelgeAntet Do Q\n".to_vec()));

        let mut resources = page_resources(body, page_id);
        if let Object::Dictionary(dict) = &mut resources {
            let mut xobjects = match dict.get(b"XObject") {
                Ok(Object::Reference(id)) => body.get_dictionary(*id).cloned().unwrap_or_default(),
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => dictionary! {},
            };
            xobjects.set("BelgeAntet", form_id);
            dict.set("XObject", xobjects);
        }
        let page = body.get_dictionary_mut(page_id).map_err(|_| EngineError::NoPages)?;
        let mut contents = vec![Object::Reference(prefix_id)];
        match page.get(b"Contents") {
            Ok(Object::Array(items)) => contents.extend(items.iter().cloned()),
            Ok(other) => contents.push(other.clone()),
            Err(_) => {}
        }
        page.set("Contents", contents);
        page.set("Resources", resources);
    }
    body.prune_objects();
    Ok(())
}

pub fn decode_base64(data: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(clean_base64(data)).ok()
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub fn label(kind: &str, n: i64) -> String {
        super::list_label(kind, n).0
    }
}
