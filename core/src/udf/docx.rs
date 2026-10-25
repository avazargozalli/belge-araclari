//! UDF → Word (.docx, Office Open XML).
//!
//! Each UDF becomes one section with its own page size, margins, header and footer, so several
//! documents can be merged into one file. The letterhead is a picture anchored behind the text in
//! the header (it repeats on every page as in UYAP), page numbers are live PAGE / SECTIONPAGES
//! fields, and tables keep their merged-cell grid. List labels are written as text: Word's own
//! letter numbering uses the English alphabet, while UYAP counts a, b, c, ç, d…

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::{color, decode_base64, num, Attrs, Block, ListPlan, PageNumber, Paragraph, ParagraphLayout, Row, RunKind, TableLayout, UdfDocument};

const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const REL_HEADER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const REL_FOOTER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";

/// Points → twentieths of a point.
fn tw(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

/// Points → English Metric Units.
fn emu(pt: f32) -> i64 {
    (pt * 12_700.0).round() as i64
}

/// Escapes text for XML and drops characters XML 1.0 cannot hold.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Java logical fonts become the fonts UYAP shows for them.
fn word_font(family: &str) -> &str {
    match family {
        "Dialog" | "SansSerif" => "Arial",
        "Serif" => "Times New Roman",
        "Monospaced" | "DialogInput" => "Courier New",
        other => other,
    }
}

fn hex(value: Option<&String>) -> Option<String> {
    color(value).map(|c| c.trim_start_matches('#').to_uppercase())
}

/// Run properties in the order the schema requires.
fn run_props(a: &Attrs, decorations: bool) -> String {
    let font = word_font(a.get("family").map(String::as_str).unwrap_or("Times New Roman"));
    let half_points = (num(a.get("size"), 12.0).max(1.0) * 2.0).round() as i64;
    let on = |key: &str| a.get(key).map(String::as_str) == Some("true");
    let mut out = format!("<w:rPr><w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:cs=\"{0}\" w:eastAsia=\"{0}\"/>", xml(font));
    if on("bold") {
        out.push_str("<w:b/><w:bCs/>");
    }
    if on("italic") {
        out.push_str("<w:i/><w:iCs/>");
    }
    if decorations && on("strikethrough") {
        out.push_str("<w:strike/>");
    }
    if let Some(c) = hex(a.get("foreground")) {
        write!(out, "<w:color w:val=\"{c}\"/>").ok();
    }
    write!(out, "<w:sz w:val=\"{half_points}\"/><w:szCs w:val=\"{half_points}\"/>").ok();
    if decorations && on("underline") {
        out.push_str("<w:u w:val=\"single\"/>");
    }
    if decorations {
        if let Some(c) = a.get("background").filter(|v| !v.is_empty()).and_then(|v| hex(Some(v))) {
            write!(out, "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{c}\"/>").ok();
        }
    }
    if on("superscript") {
        out.push_str("<w:vertAlign w:val=\"superscript\"/>");
    } else if on("subscript") {
        out.push_str("<w:vertAlign w:val=\"subscript\"/>");
    }
    out.push_str("</w:rPr>");
    out
}

/// Text as runs: tabs become `<w:tab/>`, line breaks `<w:br/>`.
fn text_runs(text: &str, props: &str, out: &mut String) {
    write!(out, "<w:r>{props}").ok();
    let mut piece = String::new();
    let flush = |piece: &mut String, out: &mut String| {
        if !piece.is_empty() {
            write!(out, "<w:t xml:space=\"preserve\">{}</w:t>", xml(piece)).ok();
            piece.clear();
        }
    };
    for c in text.chars() {
        match c {
            '\t' => {
                flush(&mut piece, out);
                out.push_str("<w:tab/>");
            }
            '\n' => {
                flush(&mut piece, out);
                out.push_str("<w:br/>");
            }
            c => piece.push(c),
        }
    }
    flush(&mut piece, out);
    out.push_str("</w:r>");
}

/// Pixel size of a PNG, JPEG, GIF or BMP, and the file extension to store it under.
fn image_info(bytes: &[u8]) -> Option<(&'static str, u32, u32)> {
    let be = |i: usize| -> Option<u32> { Some(u32::from_be_bytes(bytes.get(i..i + 4)?.try_into().ok()?)) };
    let be16 = |i: usize| -> Option<u32> { Some(u16::from_be_bytes(bytes.get(i..i + 2)?.try_into().ok()?) as u32) };
    let le16 = |i: usize| -> Option<u32> { Some(u16::from_le_bytes(bytes.get(i..i + 2)?.try_into().ok()?) as u32) };
    if bytes.starts_with(b"\x89PNG") {
        return Some(("png", be(16)?, be(20)?));
    }
    if bytes.starts_with(b"GIF8") {
        return Some(("gif", le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"BM") {
        let w = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?).unsigned_abs();
        let h = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?).unsigned_abs();
        return Some(("bmp", w, h));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                return Some(("jpeg", be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

struct Rel {
    id: String,
    kind: &'static str,
    target: String,
}

#[derive(Default)]
struct Package {
    /// word/media/<name>
    media: Vec<(String, Vec<u8>)>,
    /// word/<name>, its XML and its relationships
    parts: Vec<(String, String, Vec<Rel>)>,
    drawings: u32,
}

/// One XML part being written (document body, header or footer) with its relationships.
struct Part<'p> {
    pkg: &'p mut Package,
    doc: &'p UdfDocument,
    rels: Vec<Rel>,
    out: String,
}

impl<'p> Part<'p> {
    fn new(pkg: &'p mut Package, doc: &'p UdfDocument) -> Self {
        Part { pkg, doc, rels: vec![], out: String::new() }
    }

    fn rel(&mut self, kind: &'static str, target: String) -> String {
        let id = format!("rId{}", self.rels.len() + 1);
        self.rels.push(Rel { id: id.clone(), kind, target });
        id
    }

    /// Stores an image and returns its relationship id and natural size in points (96 dpi).
    fn image(&mut self, data: &str) -> Option<(String, f32, f32)> {
        let bytes = decode_base64(data)?;
        let (ext, w, h) = image_info(&bytes)?;
        let name = format!("image{}.{ext}", self.pkg.media.len() + 1);
        self.pkg.media.push((name.clone(), bytes));
        let id = self.rel(REL_IMAGE, format!("media/{name}"));
        Some((id, w as f32 * 0.75, h as f32 * 0.75))
    }

    fn drawing_id(&mut self) -> u32 {
        self.pkg.drawings += 1;
        self.pkg.drawings
    }

    fn picture(&mut self, rid: &str, w: f32, h: f32) -> String {
        let id = self.drawing_id();
        format!(
            "<a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic>\
<pic:nvPicPr><pic:cNvPr id=\"{id}\" name=\"Resim {id}\"/><pic:cNvPicPr/></pic:nvPicPr>\
<pic:blipFill><a:blip r:embed=\"{rid}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
<pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr>\
</pic:pic></a:graphicData></a:graphic>",
            cx = emu(w),
            cy = emu(h)
        )
    }

    fn inline_image(&mut self, data: &str, width: f32, height: f32) {
        let Some((rid, nw, nh)) = self.image(data) else { return };
        let (w, h) = if width > 0.0 && height > 0.0 { (width, height) } else { (nw, nh) };
        let id = self.pkg.drawings + 1;
        let graphic = self.picture(&rid, w, h);
        write!(
            self.out,
            "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{}\" cy=\"{}\"/>\
<wp:docPr id=\"{id}\" name=\"Resim {id}\"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>{graphic}</wp:inline></w:drawing></w:r>",
            emu(w),
            emu(h)
        )
        .ok();
    }

    /// The letterhead: a picture behind the text, placed on the page like UYAP's background image.
    fn letterhead(&mut self) {
        let Some((data, [t, r, b, l])) = &self.doc.background else { return };
        let Some((rid, _, _)) = self.image(data) else { return };
        let (w, h) = ((self.doc.page_width - l - r).max(1.0), (self.doc.page_height - t - b).max(1.0));
        let id = self.pkg.drawings + 1;
        let graphic = self.picture(&rid, w, h);
        write!(
            self.out,
            "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\" w:line=\"20\" w:lineRule=\"exact\"/><w:rPr><w:sz w:val=\"2\"/></w:rPr></w:pPr><w:r><w:rPr><w:sz w:val=\"2\"/></w:rPr><w:drawing>\
<wp:anchor distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" simplePos=\"0\" relativeHeight=\"0\" behindDoc=\"1\" locked=\"1\" layoutInCell=\"0\" allowOverlap=\"1\">\
<wp:simplePos x=\"0\" y=\"0\"/><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH>\
<wp:positionV relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionV><wp:extent cx=\"{}\" cy=\"{}\"/>\
<wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:wrapNone/><wp:docPr id=\"{id}\" name=\"Antet\"/><wp:cNvGraphicFramePr/>{graphic}</wp:anchor></w:drawing></w:r></w:p>",
            emu(*l),
            emu(*t),
            emu(w),
            emu(h)
        )
        .ok();
    }

    /// Paragraph properties; `sect` closes a section on this paragraph.
    fn paragraph(&mut self, p: &Paragraph, lists: &mut ListPlan, sect: Option<&str>) {
        let l = ParagraphLayout::new(&p.attrs, lists);
        let wrap = l.wrap_start();
        let mut stops: Vec<(f32, &str, &str)> = l
            .stops
            .iter()
            .map(|(pos, align, leader)| {
                let align = match align {
                    1 => "right",
                    2 => "center",
                    4 => "decimal",
                    5 => "bar",
                    _ => "left",
                };
                let leader = match leader {
                    1 => "dot",
                    2 => "hyphen",
                    3 => "underscore",
                    4 => "heavy",
                    _ => "none",
                };
                (*pos, align, leader)
            })
            .collect();
        // UYAP advances 5 pt per tab past the last custom stop (Word would use its 72 pt default).
        if let Some(&(last, _, _)) = l.stops.iter().max_by(|a, b| a.0.total_cmp(&b.0)) {
            let text: String = p.runs.iter().filter(|r| matches!(r.kind, RunKind::Text)).map(|r| self.doc.slice(r.start, r.len)).collect();
            if let Some(end) = text.rfind('\t') {
                // Enough stops for every tab plus the (estimated) width of the text between them.
                let size = num(p.attrs.get("size"), 12.0).max(1.0);
                let reach = text[..end].chars().count() as f32 * size * 0.6 + 5.0 * text.matches('\t').count() as f32;
                let count = ((reach / 5.0).ceil() as usize).min(62usize.saturating_sub(stops.len()));
                for k in 1..=count {
                    stops.push((last + 5.0 * k as f32, "left", "none"));
                }
            }
        }
        // A list label sits on its own tab stops: numbers end 6 pt before the text, bullets start
        // 18 pt before it. Word treats a hanging indent as a tab stop too, so the first line only
        // starts left of the wrap position when the label has to.
        let mut first_line = l.text_start;
        let mut label_runs = String::new();
        if let Some((text, bullet)) = &l.label {
            let props = run_props(&p.attrs, false);
            if *bullet {
                first_line = (l.text_start - 18.0).max(0.0);
            } else {
                let end = l.text_start - 6.0;
                first_line = if wrap < end { wrap } else { 0.0 };
                if end > first_line + 1.0 {
                    stops.push((end, "right", "none"));
                    write!(label_runs, "<w:r>{props}<w:tab/></w:r>").ok();
                }
            }
            text_runs(text, &props, &mut label_runs);
            stops.push((l.text_start, "left", "none"));
            write!(label_runs, "<w:r>{props}<w:tab/></w:r>").ok();
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        stops.dedup_by(|a, b| (a.0 - b.0).abs() < 0.05);

        self.out.push_str("<w:p><w:pPr>");
        if !stops.is_empty() {
            self.out.push_str("<w:tabs>");
            for (pos, align, leader) in &stops {
                write!(self.out, "<w:tab w:val=\"{align}\" w:leader=\"{leader}\" w:pos=\"{}\"/>", tw(*pos)).ok();
            }
            self.out.push_str("</w:tabs>");
        }
        write!(
            self.out,
            "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{}\" w:lineRule=\"auto\"/>",
            tw(l.space_above),
            tw(l.space_below),
            (240.0 * l.line).round() as i64
        )
        .ok();
        let indent = first_line - wrap;
        let first = if indent >= 0.0 { format!("w:firstLine=\"{}\"", tw(indent)) } else { format!("w:hanging=\"{}\"", tw(-indent)) };
        write!(self.out, "<w:ind w:left=\"{}\" w:right=\"{}\" {first}/>", tw(wrap), tw(l.right)).ok();
        write!(self.out, "<w:jc w:val=\"{}\"/>", ["left", "center", "right", "both"][l.align]).ok();
        self.out.push_str(&run_props(&p.attrs, false));
        if let Some(sect) = sect {
            self.out.push_str(sect);
        }
        self.out.push_str("</w:pPr>");
        self.out.push_str(&label_runs);
        for run in &p.runs {
            match &run.kind {
                RunKind::Image { data, width, height } => self.inline_image(data, *width, *height),
                RunKind::Text => {
                    let text = self.doc.slice(run.start, run.len);
                    let text = text.trim_end_matches('\n');
                    if !text.is_empty() {
                        text_runs(text, &run_props(&run.attrs, true), &mut self.out);
                    }
                }
            }
        }
        self.out.push_str("</w:p>");
    }

    fn table(&mut self, attrs: &Attrs, rows: &[Row], width: f32, lists: &mut ListPlan) {
        if rows.is_empty() {
            return;
        }
        let layout = TableLayout::new(attrs, rows, width);
        write!(
            self.out,
            "<w:tbl><w:tblPr><w:tblW w:w=\"{}\" w:type=\"dxa\"/><w:tblInd w:w=\"{}\" w:type=\"dxa\"/><w:tblLayout w:type=\"fixed\"/>\
<w:tblCellMar><w:top w:w=\"20\" w:type=\"dxa\"/><w:left w:w=\"20\" w:type=\"dxa\"/><w:bottom w:w=\"20\" w:type=\"dxa\"/><w:right w:w=\"20\" w:type=\"dxa\"/></w:tblCellMar>\
<w:tblLook w:val=\"0000\" w:firstRow=\"0\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"1\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
            tw(layout.width),
            tw(layout.left)
        )
        .ok();
        for w in &layout.widths {
            write!(self.out, "<w:gridCol w:w=\"{}\"/>", tw(*w)).ok();
        }
        self.out.push_str("</w:tblGrid>");
        for (r, row) in rows.iter().enumerate() {
            self.out.push_str("<w:tr>");
            for (c, cell) in row.cells.iter().enumerate() {
                let (start, end) = layout.cells[r][c];
                let cell_width: f32 = layout.widths[start..end].iter().sum();
                write!(self.out, "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>", tw(cell_width)).ok();
                if end - start > 1 {
                    write!(self.out, "<w:gridSpan w:val=\"{}\"/>", end - start).ok();
                }
                self.out.push_str("<w:tcBorders>");
                let [top, bottom, left, right] = layout.edges(r, start, end);
                for (side, on) in [("top", top), ("left", left), ("bottom", bottom), ("right", right)] {
                    if on {
                        write!(self.out, "<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>").ok();
                    } else {
                        write!(self.out, "<w:{side} w:val=\"nil\"/>").ok();
                    }
                }
                self.out.push_str("</w:tcBorders></w:tcPr>");
                self.blocks(cell, cell_width - 2.0, lists, None);
                self.out.push_str("</w:tc>");
            }
            self.out.push_str("</w:tr>");
        }
        self.out.push_str("</w:tbl>");
    }

    /// Writes blocks; the last paragraph carries `sect`. Word needs a paragraph after a final table
    /// (and in every table cell), so one is added when missing.
    fn blocks(&mut self, blocks: &[Block], width: f32, lists: &mut ListPlan, sect: Option<&str>) {
        for (i, block) in blocks.iter().enumerate() {
            let last = i + 1 == blocks.len();
            match block {
                Block::Paragraph(p) => self.paragraph(p, lists, if last { sect } else { None }),
                Block::Table { attrs, rows } => self.table(attrs, rows, width, lists),
            }
        }
        if !matches!(blocks.last(), Some(Block::Paragraph(_))) {
            write!(self.out, "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/>{}</w:pPr></w:p>", sect.unwrap_or("")).ok();
        }
    }

    /// The page number line: leading text, PAGE, separator, total pages, trailing text.
    fn page_number(&mut self, n: &PageNumber) {
        let a = &n.attrs;
        let mut font = Attrs::new();
        for (from, to) in [("pageNumber-fontFace", "family"), ("pageNumber-fontSize", "size"), ("pageNumber-color", "foreground")] {
            if let Some(v) = a.get(from) {
                font.insert(to.into(), v.clone());
            }
        }
        font.entry("family".into()).or_insert_with(|| "Arial".into());
        font.entry("size".into()).or_insert_with(|| "11".into());
        for (from, to) in [("pageNumber-fontBold", "bold"), ("pageNumber-fontItalic", "italic"), ("pageNumber-fontUnderline", "underline")] {
            if let Some(v) = a.get(from) {
                font.insert(to.into(), v.clone());
            }
        }
        let props = run_props(&font, true);
        let jc = match n.align {
            "center" => "center",
            "right" => "right",
            _ => "left",
        };
        write!(self.out, "<w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/><w:jc w:val=\"{jc}\"/>{}</w:pPr>", run_props(&font, false)).ok();
        if !n.leading.is_empty() {
            text_runs(&n.leading, &props, &mut self.out);
        }
        field(&mut self.out, &props, "PAGE", &n.start.max(1).to_string());
        if n.total {
            text_runs(&n.separator, &props, &mut self.out);
            if n.start == 1 {
                field(&mut self.out, &props, "SECTIONPAGES", "1");
            } else {
                // { = { SECTIONPAGES } + start - 1 }: UYAP's total is the last page's number.
                let fld = |kind: &str| format!("<w:r>{props}<w:fldChar w:fldCharType=\"{kind}\"/></w:r>");
                let instr = |text: &str| format!("<w:r>{props}<w:instrText xml:space=\"preserve\">{text}</w:instrText></w:r>");
                self.out.push_str(&fld("begin"));
                self.out.push_str(&instr(" = "));
                field(&mut self.out, &props, "SECTIONPAGES", "1");
                self.out.push_str(&instr(&format!(" + {} ", n.start - 1)));
                self.out.push_str(&fld("separate"));
                text_runs(&n.start.to_string(), &props, &mut self.out);
                self.out.push_str(&fld("end"));
            }
        }
        if !n.trailing.is_empty() {
            text_runs(&n.trailing, &props, &mut self.out);
        }
        self.out.push_str("</w:p>");
    }

    fn finish(self, name: String, root: &str) {
        let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<{root} {NS}>{}</{root}>", self.out);
        self.pkg.parts.push((name, xml, self.rels));
    }
}

fn field(out: &mut String, props: &str, instr: &str, shown: &str) {
    write!(
        out,
        "<w:r>{props}<w:fldChar w:fldCharType=\"begin\"/></w:r><w:r>{props}<w:instrText xml:space=\"preserve\"> {instr} </w:instrText></w:r>\
<w:r>{props}<w:fldChar w:fldCharType=\"separate\"/></w:r><w:r>{props}<w:t>{shown}</w:t></w:r><w:r>{props}<w:fldChar w:fldCharType=\"end\"/></w:r>"
    )
    .ok();
}

fn header_part(pkg: &mut Package, doc: &UdfDocument, index: usize) -> String {
    let mut part = Part::new(pkg, doc);
    part.letterhead();
    if let Some(n) = &doc.header_number {
        part.page_number(n);
    }
    let mut lists = ListPlan::new(&doc.header);
    if !doc.header.is_empty() {
        part.blocks(&doc.header, doc.content_width(), &mut lists, None);
    }
    if part.out.is_empty() {
        part.out.push_str("<w:p/>");
    }
    let name = format!("header{index}.xml");
    part.finish(name.clone(), "w:hdr");
    name
}

fn footer_part(pkg: &mut Package, doc: &UdfDocument, index: usize) -> String {
    let mut part = Part::new(pkg, doc);
    let mut lists = ListPlan::new(&doc.footer);
    if !doc.footer.is_empty() {
        part.blocks(&doc.footer, doc.content_width(), &mut lists, None);
    }
    if let Some(n) = &doc.footer_number {
        part.page_number(n);
    }
    if part.out.is_empty() {
        part.out.push_str("<w:p/>");
    }
    let name = format!("footer{index}.xml");
    part.finish(name.clone(), "w:ftr");
    name
}

fn section(doc: &UdfDocument, header: &str, footer: &str) -> String {
    let [mt, mr, mb, ml] = doc.margins;
    let start = doc.header_number.as_ref().or(doc.footer_number.as_ref()).map(|n| n.start).unwrap_or(1);
    let orient = if doc.page_width > doc.page_height { " w:orient=\"landscape\"" } else { "" };
    format!(
        "<w:sectPr><w:headerReference w:type=\"default\" r:id=\"{header}\"/><w:footerReference w:type=\"default\" r:id=\"{footer}\"/>\
<w:type w:val=\"nextPage\"/><w:pgSz w:w=\"{}\" w:h=\"{}\"{orient}/>\
<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>\
<w:pgNumType w:start=\"{start}\"/><w:cols w:space=\"720\"/></w:sectPr>",
        tw(doc.page_width),
        tw(doc.page_height),
        tw(mt),
        tw(mr),
        tw(mb),
        tw(ml),
        tw(doc.header_offset),
        tw(doc.footer_offset),
    )
}

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:cs=\"Times New Roman\" w:eastAsia=\"Times New Roman\"/>\
<w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/><w:lang w:val=\"tr-TR\" w:eastAsia=\"tr-TR\" w:bidi=\"ar-SA\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\"><w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/><w:semiHidden/></w:style>\
<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:semiHidden/>\
<w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>\
<w:style w:type=\"numbering\" w:default=\"1\" w:styleId=\"NoList\"><w:name w:val=\"No List\"/><w:semiHidden/></w:style>\
</w:styles>";

const SETTINGS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:defaultTabStop w:val=\"1440\"/><w:characterSpacingControl w:val=\"doNotCompress\"/>\
<w:compat><w:compatSetting w:name=\"compatibilityMode\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat>\
</w:settings>";

fn rels_xml(rels: &[Rel]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
    for r in rels {
        write!(out, "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>", r.id, r.kind, xml(&r.target)).ok();
    }
    out.push_str("</Relationships>");
    out
}

/// Builds a .docx from one or more UDF documents, one section each.
pub fn to_docx(docs: &[UdfDocument]) -> Result<Vec<u8>, String> {
    let mut pkg = Package::default();
    let mut body = String::new();
    let mut doc_rels = vec![
        Rel { id: "rIdStyles".into(), kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles", target: "styles.xml".into() },
        Rel { id: "rIdSettings".into(), kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings", target: "settings.xml".into() },
    ];
    let mut final_section = String::new();
    for (i, doc) in docs.iter().enumerate() {
        let header = header_part(&mut pkg, doc, i + 1);
        let footer = footer_part(&mut pkg, doc, i + 1);
        let (hid, fid) = (format!("rIdH{}", i + 1), format!("rIdF{}", i + 1));
        doc_rels.push(Rel { id: hid.clone(), kind: REL_HEADER, target: header });
        doc_rels.push(Rel { id: fid.clone(), kind: REL_FOOTER, target: footer });
        let sect = section(doc, &hid, &fid);

        let mut part = Part::new(&mut pkg, doc);
        // Image relationships of the body continue after the fixed ones.
        part.rels = std::mem::take(&mut doc_rels);
        let mut lists = ListPlan::new(&doc.body);
        let last = i + 1 == docs.len();
        part.blocks(&doc.body, doc.content_width(), &mut lists, (!last).then_some(sect.as_str()));
        if last {
            final_section = sect;
        }
        doc_rels = std::mem::take(&mut part.rels);
        body.push_str(&part.out);
    }
    let document = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document {NS}><w:body>{body}{final_section}</w:body></w:document>");

    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Default Extension=\"png\" ContentType=\"image/png\"/><Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
<Default Extension=\"gif\" ContentType=\"image/gif\"/><Default Extension=\"bmp\" ContentType=\"image/bmp\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>",
    );
    for (name, _, _) in &pkg.parts {
        let kind = if name.starts_with("header") { "header" } else { "footer" };
        write!(types, "<Override PartName=\"/word/{name}\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.{kind}+xml\"/>").ok();
    }
    types.push_str("</Types>");

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, data: &[u8]| -> Result<(), String> {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(data).map_err(|e| e.to_string())
    };
    put("[Content_Types].xml", types.as_bytes())?;
    put(
        "_rels/.rels",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/></Relationships>",
    )?;
    put(
        "docProps/app.xml",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>Belge Ara\xc3\xa7lar\xc4\xb1</Application></Properties>",
    )?;
    put("word/document.xml", document.as_bytes())?;
    put("word/_rels/document.xml.rels", rels_xml(&doc_rels).as_bytes())?;
    put("word/styles.xml", STYLES.as_bytes())?;
    put("word/settings.xml", SETTINGS.as_bytes())?;
    for (name, xml, rels) in &pkg.parts {
        put(&format!("word/{name}"), xml.as_bytes())?;
        if !rels.is_empty() {
            put(&format!("word/_rels/{name}.rels"), rels_xml(rels).as_bytes())?;
        }
    }
    for (name, data) in &pkg.media {
        put(&format!("word/media/{name}"), data)?;
    }
    drop(put);
    zip.finish().map(|c| c.into_inner()).map_err(|e| e.to_string())
}

/// Converts UDF files into one .docx at `output` (one section per input, in order).
pub fn convert(inputs: &[PathBuf], output: &Path) -> Result<(), String> {
    let docs = inputs.iter().map(|p| super::parse_file(p)).collect::<Result<Vec<_>, _>>()?;
    let bytes = to_docx(&docs)?;
    let name = output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = output.with_extension("docx.part");
    std::fs::write(&tmp, bytes)
        .and_then(|_| std::fs::rename(&tmp, output))
        .map_err(|_| {
            let _ = std::fs::remove_file(&tmp);
            format!("\"{name}\" kaydedilemedi. Klasöre yazma izni olduğundan emin olun.")
        })
}
