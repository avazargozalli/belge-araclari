//! Page numbers stamped onto a finished PDF ("Sayfa 3 / 12" in a chosen corner).
//!
//! The number is drawn in Helvetica, one of the fonts every PDF reader has, so nothing is
//! embedded, on a small white backing so it stays readable over a coloured letterhead. Each page's
//! own drawing is wrapped in q … Q first, so whatever state it leaves cannot
//! move the number, and rotated pages get the number upright where the reader shows that corner.

use std::path::Path;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vertical {
    Top,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Horizontal {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// 3
    Number,
    /// 3 / 12
    OfTotal,
    /// Sayfa 3
    Page,
    /// Sayfa 3 / 12
    PageOfTotal,
    /// - 3 -
    Dashes,
}

#[derive(Clone, Copy, Debug)]
pub struct Numbering {
    pub vertical: Vertical,
    pub horizontal: Horizontal,
    pub format: Format,
    /// Number of the first page.
    pub start: u32,
}

impl Numbering {
    /// From the apps' option strings: position "bottom-right", "top-center"…; format "n",
    /// "n/N", "sayfa-n", "sayfa-n/N", "dash".
    pub fn parse(position: &str, format: &str, start: u32) -> Option<Numbering> {
        let (v, h) = position.split_once('-')?;
        Some(Numbering {
            vertical: match v {
                "top" => Vertical::Top,
                "bottom" => Vertical::Bottom,
                _ => return None,
            },
            horizontal: match h {
                "left" => Horizontal::Left,
                "center" => Horizontal::Center,
                "right" => Horizontal::Right,
                _ => return None,
            },
            format: match format {
                "n" => Format::Number,
                "n/N" => Format::OfTotal,
                "sayfa-n" => Format::Page,
                "sayfa-n/N" => Format::PageOfTotal,
                "dash" => Format::Dashes,
                _ => return None,
            },
            start: start.max(1),
        })
    }

    pub fn text(&self, n: u32, total: u32) -> String {
        match self.format {
            Format::Number => n.to_string(),
            Format::OfTotal => format!("{n} / {total}"),
            Format::Page => format!("Sayfa {n}"),
            Format::PageOfTotal => format!("Sayfa {n} / {total}"),
            Format::Dashes => format!("- {n} -"),
        }
    }
}

const SIZE: f32 = 10.0;
const SIDE: f32 = 36.0;
const EDGE: f32 = 24.0;

/// Helvetica advance widths (1/1000 em) of the characters page numbers use.
fn width(text: &str) -> f32 {
    text.chars()
        .map(|c| match c {
            '0'..='9' => 556.0,
            ' ' | '/' | 'f' => 278.0,
            '-' => 333.0,
            'S' => 667.0,
            'a' => 556.0,
            'y' => 500.0,
            _ => 556.0,
        })
        .sum::<f32>()
        * SIZE
        / 1000.0
}

fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut current = doc.get_dictionary(page).ok();
    for _ in 0..32 {
        let dict = current?;
        if let Ok(v) = dict.get(key) {
            return Some(doc.dereference(v).map(|(_, o)| o).unwrap_or(v));
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok().and_then(|p| doc.get_dictionary(p).ok());
    }
    None
}

fn number(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// Where the text goes (baseline start) and how it is turned, in the page's own coordinates.
fn placement(n: &Numbering, text_width: f32, bx: [f32; 4], rotate: i64) -> [f32; 6] {
    let (x0, y0, x1, y1) = (bx[0], bx[1], bx[2], bx[3]);
    let (w, h) = (x1 - x0, y1 - y0);
    let (vw, vh) = if rotate % 180 == 0 { (w, h) } else { (h, w) };
    // Position on the page as the reader shows it (origin bottom-left).
    let vx = match n.horizontal {
        Horizontal::Left => SIDE,
        Horizontal::Center => (vw - text_width) / 2.0,
        Horizontal::Right => vw - SIDE - text_width,
    };
    let vy = match n.vertical {
        Vertical::Bottom => EDGE,
        Vertical::Top => vh - EDGE - SIZE * 0.72,
    };
    match rotate {
        90 => [0.0, 1.0, -1.0, 0.0, x0 + w - vy, y0 + vx],
        180 => [-1.0, 0.0, 0.0, -1.0, x1 - vx, y1 - vy],
        270 => [0.0, -1.0, 1.0, 0.0, x0 + vy, y1 - vx],
        _ => [1.0, 0.0, 0.0, 1.0, x0 + vx, y0 + vy],
    }
}

fn resources(doc: &Document, page: ObjectId) -> Dictionary {
    match inherited(doc, page, b"Resources") {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    }
}

/// Adds page numbers to every page of `doc`.
pub fn stamp_document(doc: &mut Document, n: &Numbering) -> Result<u32, String> {
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    if pages.is_empty() {
        return Err("PDF'te sayfa yok.".into());
    }
    let total = n.start - 1 + pages.len() as u32;
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
    let open = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
    for (i, page) in pages.iter().enumerate() {
        let bx = inherited(doc, *page, b"CropBox")
            .or_else(|| inherited(doc, *page, b"MediaBox"))
            .and_then(|o| o.as_array().ok())
            .map(|a| a.iter().filter_map(number).collect::<Vec<f32>>())
            .filter(|v| v.len() == 4)
            .map(|v| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
            .unwrap_or([0.0, 0.0, 595.28, 841.89]);
        let rotate = inherited(doc, *page, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0).rem_euclid(360);
        let text = n.text(n.start + i as u32, total);
        let m = placement(n, width(&text), bx, rotate);
        // A small white backing keeps the number readable on a coloured letterhead band.
        let content = format!(
            "Q\nq {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} cm 1 1 1 rg -3 -3 {:.2} {:.2} re f BT /BelgeNo {SIZE} Tf 0 0 0 rg 0 0 Td ({}) Tj ET Q\n",
            m[0], m[1], m[2], m[3], m[4], m[5], width(&text) + 6.0, SIZE + 3.0, text
        );
        let close = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));

        let mut res = resources(doc, *page);
        let mut fonts = match res.get(b"Font") {
            Ok(Object::Reference(id)) => doc.get_dictionary(*id).cloned().unwrap_or_default(),
            Ok(Object::Dictionary(d)) => d.clone(),
            _ => Dictionary::new(),
        };
        fonts.set("BelgeNo", font);
        res.set("Font", fonts);
        let dict = doc.get_dictionary_mut(*page).map_err(|e| e.to_string())?;
        let mut contents = vec![Object::Reference(open)];
        match dict.get(b"Contents") {
            Ok(Object::Array(items)) => contents.extend(items.iter().cloned()),
            Ok(other) => contents.push(other.clone()),
            Err(_) => {}
        }
        contents.push(Object::Reference(close));
        dict.set("Contents", contents);
        dict.set("Resources", res);
    }
    Ok(pages.len() as u32)
}

/// Adds page numbers to a PDF file in place. Returns the page count.
pub fn stamp(path: &Path, n: &Numbering) -> Result<u32, String> {
    let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut doc = Document::load(path).map_err(|_| format!("\"{name}\" açılamadı."))?;
    if doc.is_encrypted() {
        return Err(format!("\"{name}\" şifreli; sayfa numarası eklenemez."));
    }
    let pages = stamp_document(&mut doc, n)?;
    let tmp = path.with_extension("numara.part");
    doc.save(&tmp).map_err(|_| format!("\"{name}\" kaydedilemedi."))?;
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        format!("\"{name}\" kaydedilemedi.")
    })?;
    Ok(pages)
}
