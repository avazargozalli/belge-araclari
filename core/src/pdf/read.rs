//! Reads what a PDF page draws: every visible character with its position, font, size and colour;
//! straight lines and filled rectangles (table borders, underlines, shading); and images with the
//! box they are painted in. Coordinates are returned top-down in points from the page's top-left.

use std::collections::HashMap;
use std::io::Read as _;
use std::rc::Rc;

use lopdf::content::Content;
use lopdf::{Dictionary, Document as Pdf, Object, ObjectId};

/// A 2-D affine matrix [a b c d e f].
type Matrix = [f32; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn apply(m: &Matrix, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn number(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn numbers(ops: &[Object]) -> Vec<f32> {
    ops.iter().filter_map(number).collect()
}

// MARK: - Output

#[derive(Clone, Debug)]
pub struct Glyph {
    pub ch: char,
    /// Left edge and baseline, top-down.
    pub x: f32,
    pub y: f32,
    /// Advance width.
    pub w: f32,
    pub size: f32,
    pub font: Rc<FontStyle>,
    pub color: u32,
}

#[derive(Debug, PartialEq)]
pub struct FontStyle {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// An axis-aligned line or filled rectangle, top-down: x0 ≤ x1, y0 ≤ y1.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub color: u32,
    /// A filled area (shading) rather than a line.
    pub fill: bool,
}

#[derive(Clone, Debug)]
pub struct Picture {
    /// PNG or JPEG bytes.
    pub data: Vec<u8>,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

#[derive(Default, Debug)]
pub struct Page {
    pub width: f32,
    pub height: f32,
    pub glyphs: Vec<Glyph>,
    pub rules: Vec<Rule>,
    pub pictures: Vec<Picture>,
}

// MARK: - Fonts

/// Maps character codes to text, from a ToUnicode CMap.
#[derive(Default)]
struct UnicodeMap {
    map: HashMap<(u8, u32), String>,
    /// Code byte lengths from codespacerange, as (length, low, high).
    spaces: Vec<(u8, u32, u32)>,
}

fn hex_bytes(token: &str) -> Vec<u8> {
    let digits: Vec<u8> = token.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0) * if c.len() == 1 { 16 } else { 1 }).collect()
}

fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks(2).map(|c| if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { c[0] as u16 }).collect();
    String::from_utf16_lossy(&units)
}

fn code_value(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |v, b| (v << 8) | *b as u32)
}

impl UnicodeMap {
    fn parse(data: &[u8]) -> UnicodeMap {
        let text = String::from_utf8_lossy(data);
        // Tokens: <hex>, [ ], and words.
        let mut tokens: Vec<String> = vec![];
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '<' => {
                    let mut t = String::from("<");
                    for d in chars.by_ref() {
                        t.push(d);
                        if d == '>' {
                            break;
                        }
                    }
                    tokens.push(t);
                }
                '[' | ']' => tokens.push(c.to_string()),
                '%' => {
                    for d in chars.by_ref() {
                        if d == '\n' || d == '\r' {
                            break;
                        }
                    }
                }
                c if c.is_whitespace() => {}
                c => {
                    let mut t = c.to_string();
                    while let Some(&d) = chars.peek() {
                        if d.is_whitespace() || "<[]/".contains(d) {
                            break;
                        }
                        t.push(d);
                        chars.next();
                    }
                    tokens.push(t);
                }
            }
        }
        let mut m = UnicodeMap::default();
        let mut i = 0;
        let hex = |t: &str| t.starts_with('<');
        while i < tokens.len() {
            match tokens[i].as_str() {
                "begincodespacerange" => {
                    i += 1;
                    while i + 1 < tokens.len() && tokens[i] != "endcodespacerange" {
                        let (lo, hi) = (hex_bytes(&tokens[i]), hex_bytes(&tokens[i + 1]));
                        m.spaces.push((lo.len().max(1) as u8, code_value(&lo), code_value(&hi)));
                        i += 2;
                    }
                }
                "beginbfchar" => {
                    i += 1;
                    while i + 1 < tokens.len() && tokens[i] != "endbfchar" {
                        if hex(&tokens[i]) && hex(&tokens[i + 1]) {
                            let src = hex_bytes(&tokens[i]);
                            m.map.insert((src.len() as u8, code_value(&src)), utf16_text(&hex_bytes(&tokens[i + 1])));
                        }
                        i += 2;
                    }
                }
                "beginbfrange" => {
                    i += 1;
                    while i + 2 < tokens.len() && tokens[i] != "endbfrange" {
                        let (lo, hi) = (hex_bytes(&tokens[i]), hex_bytes(&tokens[i + 1]));
                        let len = lo.len() as u8;
                        let (lo, hi) = (code_value(&lo), code_value(&hi));
                        if tokens[i + 2] == "[" {
                            let mut j = i + 3;
                            let mut code = lo;
                            while j < tokens.len() && tokens[j] != "]" {
                                m.map.insert((len, code), utf16_text(&hex_bytes(&tokens[j])));
                                code += 1;
                                j += 1;
                            }
                            i = j + 1;
                        } else {
                            let dst = hex_bytes(&tokens[i + 2]);
                            if hi >= lo && hi - lo < 65_536 {
                                let base = code_value(&dst);
                                for (k, code) in (lo..=hi).enumerate() {
                                    // The last UTF-16 unit increments across the range.
                                    let mut bytes = dst.clone();
                                    let n = bytes.len();
                                    if n >= 2 {
                                        let last = u16::from_be_bytes([bytes[n - 2], bytes[n - 1]]).wrapping_add(k as u16);
                                        bytes[n - 2..].copy_from_slice(&last.to_be_bytes());
                                    } else {
                                        bytes = ((base as usize + k) as u16).to_be_bytes().to_vec();
                                    }
                                    m.map.insert((len, code), utf16_text(&bytes));
                                }
                            }
                            i += 3;
                        }
                    }
                }
                _ => i += 1,
            }
        }
        m
    }
}

/// How a font turns string bytes into characters and widths.
struct Font<'d> {
    style: Rc<FontStyle>,
    /// Two-byte codes (Type0 fonts with Identity or other 2-byte CMaps).
    two_byte: bool,
    unicode: Option<UnicodeMap>,
    simple: Option<lopdf::Encoding<'d>>,
    widths: HashMap<u32, f32>,
    default_width: f32,
    /// Type3 fonts measure glyphs with their FontMatrix instead of 1/1000.
    scale: f32,
}

/// "ABCDEF+TimesNewRomanPS-BoldItalicMT" → ("Times New Roman", bold, italic).
pub fn font_style(base: &str, flags: i64, weight: f32) -> FontStyle {
    let name = base.split_once('+').map_or(base, |(prefix, rest)| if prefix.len() == 6 { rest } else { base });
    let lower = name.to_ascii_lowercase();
    let bold = lower.contains("bold") || lower.contains("black") || lower.contains("heavy") || lower.contains("semibold") || lower.contains("demi") || flags & (1 << 18) != 0 || weight >= 600.0;
    let italic = lower.contains("italic") || lower.contains("oblique") || flags & (1 << 6) != 0;
    let mut family = name.split(['-', ',']).next().unwrap_or(name).to_string();
    for suffix in ["PSMT", "PS", "MT", "Bold", "Italic", "Regular", "Roman"] {
        if family.len() > suffix.len() + 2 && family.ends_with(suffix) && !(suffix == "Roman" && family.ends_with("NewRoman")) {
            family.truncate(family.len() - suffix.len());
        }
    }
    // CamelCase → words: "TimesNewRoman" → "Times New Roman".
    let mut spaced = String::new();
    let chars: Vec<char> = family.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() && (chars[i - 1].is_lowercase() || chars.get(i + 1).is_some_and(|n| n.is_lowercase()) && chars[i - 1].is_uppercase()) {
            spaced.push(' ');
        }
        spaced.push(*c);
    }
    let family = match spaced.as_str() {
        "Times" | "Times Roman" | "Times-Roman" => "Times New Roman".to_string(),
        "Helvetica" | "Arial MT" => "Arial".to_string(),
        "Courier" => "Courier New".to_string(),
        "" => "Times New Roman".to_string(),
        other => other.to_string(),
    };
    FontStyle { family, bold, italic }
}

impl<'d> Font<'d> {
    fn load(pdf: &'d Pdf, dict: &'d Dictionary) -> Font<'d> {
        let get = |d: &'d Dictionary, key: &[u8]| -> Option<&'d Object> { d.get(key).ok().map(|o| pdf.dereference(o).map(|(_, o)| o).unwrap_or(o)) };
        let subtype = dict.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
        let base = get(dict, b"BaseFont").and_then(|o| o.as_name().ok()).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
        let descendant = if subtype == b"Type0" {
            get(dict, b"DescendantFonts").and_then(|o| o.as_array().ok()).and_then(|a| a.first()).and_then(|o| pdf.dereference(o).ok()).and_then(|(_, o)| o.as_dict().ok())
        } else {
            None
        };
        let descriptor = get(descendant.unwrap_or(dict), b"FontDescriptor").and_then(|o| o.as_dict().ok());
        let flags = descriptor.and_then(|d| get(d, b"Flags")).and_then(|o| o.as_i64().ok()).unwrap_or(0);
        let weight = descriptor.and_then(|d| get(d, b"FontWeight")).and_then(number).unwrap_or(0.0);
        let style = Rc::new(font_style(&base, flags, weight));

        let unicode = get(dict, b"ToUnicode").and_then(|o| o.as_stream().ok()).and_then(|s| s.decompressed_content().ok().or_else(|| Some(s.content.clone()))).map(|d| UnicodeMap::parse(&d));
        let mut widths = HashMap::new();
        let default_width;
        let mut scale = 0.001;
        if let Some(desc) = descendant {
            default_width = get(desc, b"DW").and_then(number).unwrap_or(1000.0);
            if let Some(w) = get(desc, b"W").and_then(|o| o.as_array().ok()) {
                let mut i = 0;
                while i < w.len() {
                    let Some(first) = number(&w[i]) else { break };
                    match w.get(i + 1).map(|o| pdf.dereference(o).map(|(_, o)| o).unwrap_or(o)) {
                        Some(Object::Array(list)) => {
                            for (k, v) in list.iter().enumerate() {
                                if let Some(v) = number(v) {
                                    widths.insert(first as u32 + k as u32, v);
                                }
                            }
                            i += 2;
                        }
                        Some(last) => {
                            if let (Some(last), Some(v)) = (number(last), w.get(i + 2).and_then(number)) {
                                for code in first as u32..=last as u32 {
                                    widths.insert(code, v);
                                }
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
        } else {
            let first = get(dict, b"FirstChar").and_then(number).unwrap_or(0.0) as u32;
            if let Some(w) = get(dict, b"Widths").and_then(|o| o.as_array().ok()) {
                for (k, v) in w.iter().enumerate() {
                    if let Some(v) = number(pdf.dereference(v).map(|(_, o)| o).unwrap_or(v)) {
                        widths.insert(first + k as u32, v);
                    }
                }
            }
            default_width = descriptor.and_then(|d| get(d, b"MissingWidth")).and_then(number).filter(|w| *w > 0.0).unwrap_or(500.0);
            if subtype == b"Type3" {
                scale = get(dict, b"FontMatrix").and_then(|o| o.as_array().ok()).and_then(|a| a.first()).and_then(number).unwrap_or(0.001);
            }
        }
        let simple = if descendant.is_none() { dict.get_font_encoding(pdf).ok() } else { None };
        Font { style, two_byte: descendant.is_some(), unicode, simple, widths, default_width, scale }
    }

    /// Splits string bytes into (code, code length).
    fn codes(&self, bytes: &[u8]) -> Vec<(u32, u8, Vec<u8>)> {
        let mut out = vec![];
        let mut i = 0;
        while i < bytes.len() {
            let mut len = if self.two_byte { 2 } else { 1 };
            if let Some(map) = &self.unicode {
                if !map.spaces.is_empty() {
                    // The shortest code length whose range contains these bytes.
                    for l in 1..=4u8 {
                        if i + l as usize > bytes.len() {
                            break;
                        }
                        let v = code_value(&bytes[i..i + l as usize]);
                        if map.spaces.iter().any(|(sl, lo, hi)| *sl == l && v >= *lo && v <= *hi) {
                            len = l as usize;
                            break;
                        }
                    }
                }
            }
            let len = len.min(bytes.len() - i);
            let slice = bytes[i..i + len].to_vec();
            out.push((code_value(&slice), len as u8, slice));
            i += len;
        }
        out
    }

    fn text(&self, code: u32, len: u8, bytes: &[u8]) -> String {
        if let Some(t) = self.unicode.as_ref().and_then(|m| m.map.get(&(len, code))) {
            return t.clone();
        }
        if let Some(enc) = &self.simple {
            if let Ok(t) = enc.bytes_to_string(bytes) {
                return t;
            }
        }
        if !self.two_byte {
            return (code as u8 as char).to_string();
        }
        String::new()
    }

    fn width(&self, code: u32) -> f32 {
        self.widths.get(&code).copied().unwrap_or(self.default_width) * self.scale
    }
}

// MARK: - Images

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in data {
        crc ^= *b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Encodes 8-bit RGB or RGBA pixels as a PNG.
pub fn png(width: u32, height: u32, channels: u8, pixels: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let row = width as usize * channels as usize;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for r in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&pixels[r * row..(r + 1) * row]);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = z.write_all(&raw);
    let idat = z.finish().unwrap_or_default();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, if channels == 4 { 6 } else { 2 }, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}

fn filters(dict: &Dictionary) -> Vec<Vec<u8>> {
    match dict.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => vec![],
    }
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![];
    flate2::read::ZlibDecoder::new(data).take(512 << 20).read_to_end(&mut out).ok()?;
    Some(out)
}

/// 8-bit samples of an image XObject as RGB, with the colour space resolved.
fn samples_rgb(pdf: &Pdf, dict: &Dictionary, raw: &[u8], width: usize, height: usize) -> Option<Vec<u8>> {
    let bits = dict.get(b"BitsPerComponent").ok().and_then(number).unwrap_or(8.0) as usize;
    let space = dict.get(b"ColorSpace").ok().map(|o| pdf.dereference(o).map(|(_, o)| o.clone()).unwrap_or_else(|_| o.clone()));
    // (components, palette)
    let (components, palette): (usize, Option<(usize, Vec<u8>)>) = match &space {
        Some(Object::Name(n)) if n == b"DeviceGray" || n == b"CalGray" => (1, None),
        Some(Object::Name(n)) if n == b"DeviceCMYK" => (4, None),
        Some(Object::Name(_)) => (3, None),
        Some(Object::Array(a)) => match a.first().and_then(|o| o.as_name().ok()) {
            Some(b"ICCBased") => {
                let n = a.get(1).and_then(|o| pdf.dereference(o).ok()).and_then(|(_, o)| o.as_stream().ok()).and_then(|s| s.dict.get(b"N").ok()).and_then(number).unwrap_or(3.0);
                (n as usize, None)
            }
            Some(b"Indexed") => {
                let base = a.get(1).and_then(|o| pdf.dereference(o).ok()).map(|(_, o)| o.clone());
                let base_n = match &base {
                    Some(Object::Name(n)) if n == b"DeviceGray" => 1,
                    Some(Object::Name(n)) if n == b"DeviceCMYK" => 4,
                    Some(Object::Array(b)) if b.first().and_then(|o| o.as_name().ok()) == Some(b"ICCBased") => {
                        b.get(1).and_then(|o| pdf.dereference(o).ok()).and_then(|(_, o)| o.as_stream().ok()).and_then(|s| s.dict.get(b"N").ok()).and_then(number).unwrap_or(3.0) as usize
                    }
                    _ => 3,
                };
                let lookup = match a.get(3).map(|o| pdf.dereference(o).map(|(_, o)| o).unwrap_or(o)) {
                    Some(Object::String(s, _)) => s.clone(),
                    Some(Object::Stream(s)) => s.decompressed_content().unwrap_or_else(|_| s.content.clone()),
                    _ => return None,
                };
                (1, Some((base_n, lookup)))
            }
            Some(b"DeviceGray") | Some(b"CalGray") => (1, None),
            Some(b"DeviceCMYK") => (4, None),
            _ => (3, None),
        },
        _ => (1, None),
    };
    let row_bits = width * components * bits;
    let row_bytes = row_bits.div_ceil(8);
    if raw.len() < row_bytes * height {
        return None;
    }
    let sample = |row: &[u8], i: usize| -> u8 {
        match bits {
            8 => row[i],
            16 => row[i * 2],
            1 | 2 | 4 => {
                let bit = i * bits;
                let v = (row[bit / 8] >> (8 - bits - bit % 8)) & ((1 << bits) - 1);
                if palette.is_some() { v } else { (v as u32 * 255 / ((1 << bits) - 1) as u32) as u8 }
            }
            _ => 0,
        }
    };
    let mut out = Vec::with_capacity(width * height * 3);
    for r in 0..height {
        let row = &raw[r * row_bytes..(r + 1) * row_bytes];
        for x in 0..width {
            let px: Vec<u8> = (0..components).map(|c| sample(row, x * components + c)).collect();
            let rgb = |px: &[u8], n: usize| -> [u8; 3] {
                match n {
                    1 => [px[0]; 3],
                    4 => {
                        let k = px[3] as f32 / 255.0;
                        let f = |c: u8| (255.0 * (1.0 - c as f32 / 255.0) * (1.0 - k)) as u8;
                        [f(px[0]), f(px[1]), f(px[2])]
                    }
                    _ => [px[0], *px.get(1).unwrap_or(&0), *px.get(2).unwrap_or(&0)],
                }
            };
            let c = match &palette {
                Some((n, lookup)) => {
                    let i = px[0] as usize * n;
                    if i + n <= lookup.len() { rgb(&lookup[i..i + n], *n) } else { [0; 3] }
                }
                None => rgb(&px, components),
            };
            out.extend_from_slice(&c);
        }
    }
    Some(out)
}

/// An image XObject as PNG or JPEG bytes. Masks, JPEG 2000 and JBIG2 are skipped.
fn picture_bytes(pdf: &Pdf, stream: &lopdf::Stream) -> Option<Vec<u8>> {
    let dict = &stream.dict;
    if dict.get(b"ImageMask").ok().and_then(|o| o.as_bool().ok()) == Some(true) {
        return None;
    }
    let width = dict.get(b"Width").ok().and_then(number)? as usize;
    let height = dict.get(b"Height").ok().and_then(number)? as usize;
    if width == 0 || height == 0 || width * height > 60_000_000 {
        return None;
    }
    let f = filters(dict);
    if f.last().map(Vec::as_slice) == Some(b"DCTDecode") {
        let mut data = stream.content.clone();
        for filter in &f[..f.len() - 1] {
            data = if filter == b"FlateDecode" { inflate(&data)? } else { return None };
        }
        return Some(data);
    }
    if f.iter().any(|x| x == b"JPXDecode" || x == b"JBIG2Decode" || x == b"CCITTFaxDecode") {
        return None;
    }
    let raw = if f.is_empty() { stream.content.clone() } else { stream.decompressed_content().ok()? };
    let rgb = samples_rgb(pdf, dict, &raw, width, height)?;
    // Soft mask → alpha.
    let alpha = dict
        .get(b"SMask")
        .ok()
        .and_then(|o| pdf.dereference(o).ok())
        .and_then(|(_, o)| o.as_stream().ok())
        .and_then(|m| {
            let (w, h) = (m.dict.get(b"Width").ok().and_then(number)? as usize, m.dict.get(b"Height").ok().and_then(number)? as usize);
            let raw = m.decompressed_content().ok().unwrap_or_else(|| m.content.clone());
            (w == width && h == height && raw.len() >= w * h).then(|| raw[..w * h].to_vec())
        });
    Some(match alpha {
        Some(a) => {
            let mut rgba = Vec::with_capacity(width * height * 4);
            for (i, px) in rgb.chunks(3).enumerate() {
                rgba.extend_from_slice(px);
                rgba.push(a[i]);
            }
            png(width as u32, height as u32, 4, &rgba)
        }
        None => png(width as u32, height as u32, 3, &rgb),
    })
}

// MARK: - Interpreter

#[derive(Clone)]
struct GraphicsState {
    ctm: Matrix,
    fill: u32,
    stroke: u32,
    fill_space: u8,
    stroke_space: u8,
    line_width: f32,
}

#[derive(Clone)]
struct TextState {
    font: Option<usize>,
    size: f32,
    char_spacing: f32,
    word_spacing: f32,
    scale: f32,
    leading: f32,
    rise: f32,
    render: i64,
}

fn rgb(components: &[f32]) -> u32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    match components.len() {
        1 => c(components[0]) * 0x010101,
        3 => (c(components[0]) << 16) | (c(components[1]) << 8) | c(components[2]),
        4 => {
            let k = components[3];
            let f = |v: f32| c((1.0 - v) * (1.0 - k));
            (f(components[0]) << 16) | (f(components[1]) << 8) | f(components[2])
        }
        _ => 0,
    }
}

struct Interpreter<'d> {
    pdf: &'d Pdf,
    fonts: Vec<Font<'d>>,
    font_ids: HashMap<ObjectId, usize>,
    page: Page,
    /// Page box origin and height, to flip to top-down coordinates.
    origin: (f32, f32),
    top: f32,
    depth: usize,
}

fn resources<'d>(pdf: &'d Pdf, dict: Option<&'d Object>) -> Option<&'d Dictionary> {
    let obj = dict?;
    pdf.dereference(obj).ok().and_then(|(_, o)| o.as_dict().ok())
}

impl<'d> Interpreter<'d> {
    fn flip(&self, x: f32, y: f32) -> (f32, f32) {
        (x - self.origin.0, self.top - y)
    }

    fn font_for(&mut self, res: Option<&'d Dictionary>, name: &[u8]) -> Option<usize> {
        let fonts = resources(self.pdf, res?.get(b"Font").ok())?;
        let obj = fonts.get(name).ok()?;
        let id = obj.as_reference().ok();
        if let Some(i) = id.and_then(|id| self.font_ids.get(&id)) {
            return Some(*i);
        }
        let dict = self.pdf.dereference(obj).ok()?.1.as_dict().ok()?;
        self.fonts.push(Font::load(self.pdf, dict));
        let i = self.fonts.len() - 1;
        if let Some(id) = id {
            self.font_ids.insert(id, i);
        }
        Some(i)
    }

    fn rule(&mut self, (x0, y0): (f32, f32), (x1, y1): (f32, f32), color: u32, fill: bool) {
        let (a, b) = (self.flip(x0, y0), self.flip(x1, y1));
        self.page.rules.push(Rule { x0: a.0.min(b.0), y0: a.1.min(b.1), x1: a.0.max(b.0), y1: a.1.max(b.1), color, fill });
    }

    /// Runs a content stream with its resources.
    fn run(&mut self, content: &[u8], res: Option<&'d Dictionary>, gs: GraphicsState) {
        if self.depth > 12 {
            return;
        }
        self.depth += 1;
        let Ok(content) = Content::decode(content) else {
            self.depth -= 1;
            return;
        };
        let mut gs = gs;
        let mut stack: Vec<(GraphicsState, TextState)> = vec![];
        let mut ts = TextState { font: None, size: 12.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0, rise: 0.0, render: 0 };
        let mut tm = IDENTITY;
        let mut tlm = IDENTITY;
        // Current path: rectangles and line segments in user space.
        let mut path_rects: Vec<(f32, f32, f32, f32)> = vec![];
        let mut path_lines: Vec<((f32, f32), (f32, f32))> = vec![];
        let mut current = (0.0f32, 0.0f32);
        let mut start = (0.0f32, 0.0f32);
        let mut curved = false;

        for op in &content.operations {
            let o = &op.operands;
            let n = numbers(o);
            match op.operator.as_str() {
                "q" => stack.push((gs.clone(), ts.clone())),
                "Q" => {
                    if let Some((g, t)) = stack.pop() {
                        gs = g;
                        ts = TextState { font: t.font, size: t.size, ..ts };
                    }
                }
                "cm" if n.len() == 6 => gs.ctm = mul(&[n[0], n[1], n[2], n[3], n[4], n[5]], &gs.ctm),
                "w" if !n.is_empty() => gs.line_width = n[0],
                // Colours
                "g" if !n.is_empty() => {
                    gs.fill = rgb(&n[..1]);
                    gs.fill_space = 1;
                }
                "G" if !n.is_empty() => {
                    gs.stroke = rgb(&n[..1]);
                    gs.stroke_space = 1;
                }
                "rg" if n.len() >= 3 => {
                    gs.fill = rgb(&n[..3]);
                    gs.fill_space = 3;
                }
                "RG" if n.len() >= 3 => {
                    gs.stroke = rgb(&n[..3]);
                    gs.stroke_space = 3;
                }
                "k" if n.len() >= 4 => {
                    gs.fill = rgb(&n[..4]);
                    gs.fill_space = 4;
                }
                "K" if n.len() >= 4 => {
                    gs.stroke = rgb(&n[..4]);
                    gs.stroke_space = 4;
                }
                "cs" | "CS" => {
                    let space = match o.first().and_then(|x| x.as_name().ok()) {
                        Some(b"DeviceGray") | Some(b"CalGray") => 1,
                        Some(b"DeviceCMYK") => 4,
                        Some(b"Pattern") => 0,
                        _ => 3,
                    };
                    if op.operator == "cs" {
                        gs.fill_space = space;
                        gs.fill = 0;
                    } else {
                        gs.stroke_space = space;
                        gs.stroke = 0;
                    }
                }
                "sc" | "scn" if !n.is_empty() => {
                    let k = if n.len() >= 4 && gs.fill_space == 4 { 4 } else if n.len() >= 3 { 3 } else { 1 };
                    gs.fill = rgb(&n[..k]);
                }
                "SC" | "SCN" if !n.is_empty() => {
                    let k = if n.len() >= 4 && gs.stroke_space == 4 { 4 } else if n.len() >= 3 { 3 } else { 1 };
                    gs.stroke = rgb(&n[..k]);
                }
                // Paths
                "m" if n.len() >= 2 => {
                    current = (n[0], n[1]);
                    start = current;
                }
                "l" if n.len() >= 2 => {
                    path_lines.push((current, (n[0], n[1])));
                    current = (n[0], n[1]);
                }
                "c" | "v" | "y" => {
                    curved = true;
                    if n.len() >= 2 {
                        current = (n[n.len() - 2], n[n.len() - 1]);
                    }
                }
                "h" => {
                    if current != start {
                        path_lines.push((current, start));
                    }
                    current = start;
                }
                "re" if n.len() >= 4 => {
                    path_rects.push((n[0], n[1], n[2], n[3]));
                    current = (n[0], n[1]);
                    start = current;
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                    let paint = op.operator.as_str();
                    let fill = matches!(paint, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
                    let stroke = matches!(paint, "S" | "s" | "B" | "B*" | "b" | "b*");
                    if paint != "n" && !curved {
                        let ctm = gs.ctm;
                        let width_user = gs.line_width * (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                        for (x, y, w, h) in path_rects.clone() {
                            let a = apply(&ctm, x, y);
                            let b = apply(&ctm, x + w, y + h);
                            if fill && gs.fill != 0xFFFFFF {
                                self.rule(a, b, gs.fill, (a.0 - b.0).abs() > 2.5 && (a.1 - b.1).abs() > 2.5);
                            }
                            if stroke {
                                let c = apply(&ctm, x + w, y);
                                let d = apply(&ctm, x, y + h);
                                for (p, q) in [(a, c), (c, b), (b, d), (d, a)] {
                                    self.rule(p, q, gs.stroke, false);
                                }
                            }
                        }
                        // A filled polygon of straight, axis-aligned edges (Word draws lines and
                        // shading this way): its bounding box.
                        if fill && gs.fill != 0xFFFFFF && !path_lines.is_empty() {
                            let pts: Vec<(f32, f32)> = path_lines.iter().flat_map(|(p, q)| [apply(&ctm, p.0, p.1), apply(&ctm, q.0, q.1)]).collect();
                            let straight = path_lines.iter().all(|(p, q)| (p.0 - q.0).abs() < 0.01 || (p.1 - q.1).abs() < 0.01);
                            if straight {
                                let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
                                let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
                                self.rule((x0, y0), (x1, y1), gs.fill, x1 - x0 > 2.5 && y1 - y0 > 2.5);
                            }
                        }
                        if stroke && width_user < 4.0 {
                            for (p, q) in path_lines.clone() {
                                let (p, q) = (apply(&ctm, p.0, p.1), apply(&ctm, q.0, q.1));
                                if (p.0 - q.0).abs() < 0.8 || (p.1 - q.1).abs() < 0.8 {
                                    self.rule(p, q, gs.stroke, false);
                                }
                            }
                        }
                    }
                    path_rects.clear();
                    path_lines.clear();
                    curved = false;
                }
                // Text
                "BT" => {
                    tm = IDENTITY;
                    tlm = IDENTITY;
                }
                "Tf" if o.len() >= 2 => {
                    if let Ok(name) = o[0].as_name() {
                        ts.font = self.font_for(res, name);
                    }
                    ts.size = number(&o[1]).unwrap_or(ts.size);
                }
                "Tc" if !n.is_empty() => ts.char_spacing = n[0],
                "Tw" if !n.is_empty() => ts.word_spacing = n[0],
                "Tz" if !n.is_empty() => ts.scale = n[0] / 100.0,
                "TL" if !n.is_empty() => ts.leading = n[0],
                "Ts" if !n.is_empty() => ts.rise = n[0],
                "Tr" if !n.is_empty() => ts.render = n[0] as i64,
                "Td" if n.len() >= 2 => {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, n[0], n[1]], &tlm);
                    tm = tlm;
                }
                "TD" if n.len() >= 2 => {
                    ts.leading = -n[1];
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, n[0], n[1]], &tlm);
                    tm = tlm;
                }
                "Tm" if n.len() >= 6 => {
                    tlm = [n[0], n[1], n[2], n[3], n[4], n[5]];
                    tm = tlm;
                }
                "T*" => {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading], &tlm);
                    tm = tlm;
                }
                "Tj" | "'" | "\"" | "TJ" => {
                    if op.operator == "'" || op.operator == "\"" {
                        if op.operator == "\"" && n.len() >= 2 {
                            ts.word_spacing = n[0];
                            ts.char_spacing = n[1];
                        }
                        tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading], &tlm);
                        tm = tlm;
                    }
                    let items: Vec<&Object> = if op.operator == "TJ" { o.first().and_then(|a| a.as_array().ok()).map(|a| a.iter().collect()).unwrap_or_default() } else { o.last().into_iter().collect() };
                    for item in items {
                        match item {
                            Object::String(bytes, _) => self.show(bytes, &ts, &mut tm, &gs),
                            other => {
                                if let Some(adj) = number(other) {
                                    let tx = -adj / 1000.0 * ts.size * ts.scale;
                                    tm = mul(&[1.0, 0.0, 0.0, 1.0, tx, 0.0], &tm);
                                }
                            }
                        }
                    }
                }
                // External objects
                "Do" => {
                    let Some(name) = o.first().and_then(|x| x.as_name().ok()) else { continue };
                    let Some(xobjects) = res.and_then(|r| resources(self.pdf, r.get(b"XObject").ok())) else { continue };
                    let Some((_, obj)) = xobjects.get(name).ok().and_then(|x| self.pdf.dereference(x).ok()) else { continue };
                    let Ok(stream) = obj.as_stream() else { continue };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"") {
                        b"Image" => {
                            let corners = [apply(&gs.ctm, 0.0, 0.0), apply(&gs.ctm, 1.0, 0.0), apply(&gs.ctm, 0.0, 1.0), apply(&gs.ctm, 1.0, 1.0)];
                            let pts: Vec<(f32, f32)> = corners.iter().map(|(x, y)| self.flip(*x, *y)).collect();
                            let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
                            let (y0, y1) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
                            if x1 - x0 < 3.0 || y1 - y0 < 3.0 {
                                continue;
                            }
                            if let Some(data) = picture_bytes(self.pdf, stream) {
                                self.page.pictures.push(Picture { data, x0, y0, x1, y1 });
                            }
                        }
                        b"Form" => {
                            let matrix = stream.dict.get(b"Matrix").ok().and_then(|m| m.as_array().ok()).map(|a| numbers(a)).filter(|m| m.len() == 6);
                            let mut inner = gs.clone();
                            if let Some(m) = matrix {
                                inner.ctm = mul(&[m[0], m[1], m[2], m[3], m[4], m[5]], &gs.ctm);
                            }
                            let form_res = resources(self.pdf, stream.dict.get(b"Resources").ok()).or(res);
                            let data = stream.decompressed_content().unwrap_or_else(|_| stream.content.clone());
                            self.run(&data, form_res, inner);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        self.depth -= 1;
    }

    fn show(&mut self, bytes: &[u8], ts: &TextState, tm: &mut Matrix, gs: &GraphicsState) {
        let Some(fi) = ts.font else { return };
        let codes = self.fonts[fi].codes(bytes);
        for (code, len, raw) in codes {
            let (text, w0, style) = {
                let font = &self.fonts[fi];
                (font.text(code, len, &raw), font.width(code), font.style.clone())
            };
            let trm = mul(&mul(&[ts.size * ts.scale, 0.0, 0.0, ts.size, 0.0, ts.rise], tm), &gs.ctm);
            let space = if len == 1 && code == 32 { ts.word_spacing } else { 0.0 };
            let advance = (w0 * ts.size + ts.char_spacing + space) * ts.scale;
            let (x, y) = apply(&trm, 0.0, 0.0);
            let horizontal = trm[1].abs() < 0.05 * trm[0].abs().max(0.001) && trm[0] > 0.0;
            let size = (trm[2] * trm[2] + trm[3] * trm[3]).sqrt();
            // Advance in user space along the baseline.
            let scale_x = ((tm[0] * gs.ctm[0] + tm[1] * gs.ctm[2]).powi(2) + (tm[0] * gs.ctm[1] + tm[1] * gs.ctm[3]).powi(2)).sqrt();
            if ts.render != 3 && ts.render != 7 && horizontal && size > 0.5 {
                let (fx, fy) = self.flip(x, y);
                let mut chars = text.chars().filter(|c| !c.is_control() || *c == ' ');
                let first = chars.next();
                let rest: Vec<char> = chars.collect();
                // Character spacing (often used to justify a line) belongs to the glyph's width,
                // so letters do not look like separate words; word spacing does not.
                let total_w = ((w0 * ts.size + ts.char_spacing) * ts.scale * scale_x).max(0.0);
                if let Some(c) = first {
                    // Ligatures ("ﬁ" → "fi") share the glyph's width.
                    let parts = 1 + rest.len();
                    let each = total_w / parts as f32;
                    for (k, ch) in std::iter::once(c).chain(rest).enumerate() {
                        self.page.glyphs.push(Glyph { ch, x: fx + each * k as f32, y: fy, w: each, size, font: style.clone(), color: gs.fill });
                    }
                }
            }
            *tm = mul(&[1.0, 0.0, 0.0, 1.0, advance, 0.0], tm);
        }
    }
}

/// Reads every page of a PDF.
pub fn read(bytes: &[u8]) -> Result<Vec<Page>, String> {
    let mut pdf = Pdf::load_mem(bytes).map_err(|_| "geçerli bir PDF değil veya bozuk.".to_string())?;
    if pdf.is_encrypted() && pdf.decrypt("").is_err() {
        return Err("şifreli. Önce \"PDF Şifrele\" aracından şifresini kaldırın.".into());
    }
    let mut pages = vec![];
    for (_, page_id) in pdf.get_pages() {
        let page_dict = pdf.get_dictionary(page_id).map_err(|e| e.to_string())?;
        let inherited = |key: &[u8]| -> Option<Vec<f32>> {
            let mut current = Some(page_dict);
            for _ in 0..32 {
                let d = current?;
                if let Ok(v) = d.get(key) {
                    return pdf.dereference(v).ok().and_then(|(_, o)| o.as_array().ok()).map(|a| numbers(a)).filter(|v| v.len() == 4);
                }
                current = d.get(b"Parent").and_then(Object::as_reference).ok().and_then(|p| pdf.get_dictionary(p).ok());
            }
            None
        };
        let bx = inherited(b"CropBox").or_else(|| inherited(b"MediaBox")).unwrap_or(vec![0.0, 0.0, 595.28, 841.89]);
        let (x0, y0, x1, y1) = (bx[0].min(bx[2]), bx[1].min(bx[3]), bx[0].max(bx[2]), bx[1].max(bx[3]));
        let (res_dict, res_ids) = pdf.get_page_resources(page_id).unwrap_or((None, vec![]));
        let res = res_dict.or_else(|| res_ids.first().and_then(|id| pdf.get_dictionary(*id).ok()));
        let content = pdf.get_page_content(page_id).unwrap_or_default();
        let mut it = Interpreter { pdf: &pdf, fonts: vec![], font_ids: HashMap::new(), page: Page { width: x1 - x0, height: y1 - y0, ..Default::default() }, origin: (x0, y0), top: y1, depth: 0 };
        let gs = GraphicsState { ctm: IDENTITY, fill: 0, stroke: 0, fill_space: 1, stroke_space: 1, line_width: 1.0 };
        it.run(&content, res, gs);
        pages.push(it.page);
    }
    if pages.is_empty() {
        return Err("sayfası yok.".into());
    }
    Ok(pages)
}
