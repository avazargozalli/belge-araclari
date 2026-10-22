//! Images → PDF, and the image-page PDF writer shared with the scanned-document compressor.

use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, GrayImage, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use rayon::prelude::*;
use serde::Deserialize;

use super::ops::{save, text_string};
use super::output::base_name;
use super::{file_name, EngineError, Progress, Result};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PageSize { Fit, A4, A5, Letter, Legal }

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Orientation { Auto, Portrait, Landscape }

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Margin { None, Small, Normal }

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Quality { Original, High, Balanced, Small }

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOptions {
    pub page_size: PageSize,
    pub orientation: Orientation,
    pub margin: Margin,
    pub quality: Quality,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self { page_size: PageSize::A4, orientation: Orientation::Auto, margin: Margin::None, quality: Quality::Balanced }
    }
}

impl PageSize {
    /// Portrait size in points; None follows the image.
    fn points(self) -> Option<(f32, f32)> {
        match self {
            PageSize::Fit => None,
            PageSize::A4 => Some((595.28, 841.89)),
            PageSize::A5 => Some((419.53, 595.28)),
            PageSize::Letter => Some((612.0, 792.0)),
            PageSize::Legal => Some((612.0, 1008.0)),
        }
    }
}

impl Margin {
    fn points(self) -> f32 {
        match self {
            Margin::None => 0.0,
            Margin::Small => 18.0,
            Margin::Normal => 36.0,
        }
    }
}

impl Quality {
    fn jpeg(self) -> u8 {
        match self {
            Quality::Original => 95,
            Quality::High => 85,
            Quality::Balanced => 75,
            Quality::Small => 60,
        }
    }
    fn max_pixel(self) -> Option<u32> {
        match self {
            Quality::Original => None,
            Quality::High => Some(3200),
            Quality::Balanced => Some(2200),
            Quality::Small => Some(1600),
        }
    }
}

/// One finished PDF page holding a single JPEG image.
pub struct JpegPage {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub gray: bool,
    pub page: (f32, f32),
    pub rect: (f32, f32, f32, f32),
}

// MARK: - Decoding

pub const SUPPORTED: &[&str] = &["jpg", "jpeg", "png", "webp", "tif", "tiff", "gif", "bmp"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .map(|e| SUPPORTED.contains(&e.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Decodes every page of an image file: all frames for TIFF, one otherwise. EXIF orientation is applied.
pub fn load_frames(path: &Path) -> Result<Vec<DynamicImage>> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "tif" || ext == "tiff" {
        if let Ok(frames) = tiff_frames(path) {
            if !frames.is_empty() {
                return Ok(frames);
            }
        }
    }
    let unreadable = || EngineError::CannotOpen(file_name(path));
    let mut decoder = ImageReader::open(path)
        .map_err(|_| unreadable())?
        .with_guessed_format()
        .map_err(|_| unreadable())?
        .into_decoder()
        .map_err(|_| unreadable())?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| unreadable())?;
    image.apply_orientation(orientation);
    Ok(vec![image])
}

fn tiff_frames(path: &Path) -> std::result::Result<Vec<DynamicImage>, Box<dyn std::error::Error>> {
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::tags::Tag;
    use tiff::ColorType;

    let mut decoder = Decoder::new(BufReader::new(File::open(path)?))?;
    let mut frames = Vec::new();
    loop {
        let (width, height) = decoder.dimensions()?;
        let color = decoder.colortype()?;
        let white_is_zero = decoder.get_tag_u32(Tag::PhotometricInterpretation).map(|v| v == 0).unwrap_or(false);
        let data = decoder.read_image()?;
        let frame = match (color, data) {
            (ColorType::Gray(8), DecodingResult::U8(buf)) => {
                let buf = if white_is_zero { buf.into_iter().map(|v| 255 - v).collect() } else { buf };
                GrayImage::from_raw(width, height, buf).map(DynamicImage::ImageLuma8)
            }
            (ColorType::Gray(1), DecodingResult::U8(buf)) => {
                let row_bytes = (width as usize).div_ceil(8);
                let mut pixels = Vec::with_capacity((width * height) as usize);
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        let bit = buf.get(y * row_bytes + x / 8).map(|b| (b >> (7 - x % 8)) & 1).unwrap_or(1);
                        let white = if white_is_zero { bit == 0 } else { bit == 1 };
                        pixels.push(if white { 255 } else { 0 });
                    }
                }
                GrayImage::from_raw(width, height, pixels).map(DynamicImage::ImageLuma8)
            }
            (ColorType::RGB(8), DecodingResult::U8(buf)) => RgbImage::from_raw(width, height, buf).map(DynamicImage::ImageRgb8),
            (ColorType::RGBA(8), DecodingResult::U8(buf)) => RgbaImage::from_raw(width, height, buf).map(DynamicImage::ImageRgba8),
            _ => None,
        };
        frames.push(frame.ok_or("unsupported tiff frame")?);
        if !decoder.more_images() {
            break;
        }
        decoder.next_image()?;
    }
    Ok(frames)
}

// MARK: - Encoding

/// Downscales, flattens transparency onto white and encodes as JPEG (grayscale stays grayscale).
pub fn encode_jpeg(image: DynamicImage, quality: u8, max_pixel: Option<u32>) -> Result<(Vec<u8>, u32, u32, bool)> {
    let mut image = image;
    if let Some(max) = max_pixel {
        if image.width().max(image.height()) > max {
            image = image.resize(max, max, FilterType::CatmullRom);
        }
    }
    let gray = matches!(image.color(), image::ColorType::L8 | image::ColorType::L16 | image::ColorType::La8 | image::ColorType::La16);
    let flat: DynamicImage = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let mut rgb = RgbImage::new(rgba.width(), rgba.height());
        for (out, px) in rgb.pixels_mut().zip(rgba.pixels()) {
            let a = px[3] as u32;
            for c in 0..3 {
                out[c] = ((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
        }
        if gray { DynamicImage::ImageRgb8(rgb).grayscale() } else { DynamicImage::ImageRgb8(rgb) }
    } else if gray {
        DynamicImage::ImageLuma8(image.to_luma8())
    } else {
        DynamicImage::ImageRgb8(image.to_rgb8())
    };
    let (width, height) = (flat.width(), flat.height());
    // Encode the raw buffer with an explicit colour type. `encode_image(&DynamicImage)` would always
    // write 3 components (DynamicImage's pixel type is RGBA), which contradicts /DeviceGray in the PDF.
    let (pixels, color) = match flat {
        DynamicImage::ImageLuma8(img) => (img.into_raw(), ExtendedColorType::L8),
        other => (other.to_rgb8().into_raw(), ExtendedColorType::Rgb8),
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), quality)
        .write_image(&pixels, width, height, color)
        .map_err(|e| EngineError::Message(format!("Resim kodlanamadı: {e}")))?;
    Ok((jpeg, width, height, color == ExtendedColorType::L8))
}

pub fn layout(image: (f32, f32), options: &ImageOptions) -> ((f32, f32), (f32, f32, f32, f32)) {
    let margin = options.margin.points();
    let (iw, ih) = image;
    let page = match options.page_size.points() {
        Some((w, h)) => {
            let landscape = match options.orientation {
                Orientation::Auto => iw > ih,
                Orientation::Portrait => false,
                Orientation::Landscape => true,
            };
            if landscape { (h, w) } else { (w, h) }
        }
        None => {
            let scale = 841.89 / iw.max(ih);
            (iw * scale + margin * 2.0, ih * scale + margin * 2.0)
        }
    };
    let (cw, ch) = (page.0 - margin * 2.0, page.1 - margin * 2.0);
    let scale = (cw / iw).min(ch / ih);
    let (dw, dh) = (iw * scale, ih * scale);
    (page, (margin + (cw - dw) / 2.0, margin + (ch - dh) / 2.0, dw, dh))
}

// MARK: - PDF writing

pub fn image_pdf(pages: &[JpegPage], title: &str) -> Result<Document> {
    if pages.is_empty() {
        return Err(EngineError::NoPages);
    }
    let mut doc = Document::with_version("1.7");
    let pages_id: ObjectId = doc.new_object_id();
    let mut kids = Vec::with_capacity(pages.len());
    for page in pages {
        let mut image = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => page.width as i64,
                "Height" => page.height as i64,
                "ColorSpace" => if page.gray { "DeviceGray" } else { "DeviceRGB" },
                "BitsPerComponent" => 8,
                "Filter" => "DCTDecode",
            },
            page.jpeg.clone(),
        );
        image.allows_compression = false;
        let image_id = doc.add_object(image);
        let (x, y, w, h) = page.rect;
        let content = format!("q {w:.3} 0 0 {h:.3} {x:.3} {y:.3} cm /Im0 Do Q");
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(page.page.0), Object::Real(page.page.1)],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id } },
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let info = doc.add_object(dictionary! { "Title" => text_string(title) });
    doc.trailer.set("Info", info);
    Ok(doc)
}

/// Converts images (in order) into one PDF. Returns the page count.
pub fn images_to_pdf(inputs: &[PathBuf], options: &ImageOptions, output: &Path, progress: Progress) -> Result<u32> {
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = inputs.len().max(1);
    let per_file: Vec<Result<Vec<JpegPage>>> = inputs
        .par_iter()
        .map(|path| {
            let frames = load_frames(path)?;
            let mut pages = Vec::with_capacity(frames.len());
            for frame in frames {
                let (jpeg, width, height, gray) = encode_jpeg(frame, options.quality.jpeg(), options.quality.max_pixel())?;
                let (page, rect) = layout((width as f32, height as f32), options);
                pages.push(JpegPage { jpeg, width, height, gray, page, rect });
            }
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(n as f64 / total as f64 * 0.9, &format!("İşlendi: {}", file_name(path)));
            Ok(pages)
        })
        .collect();

    let mut pages = Vec::new();
    for result in per_file {
        pages.extend(result?);
    }
    progress(0.95, "Kaydediliyor…");
    let mut doc = image_pdf(&pages, &base_name(output))?;
    save(&mut doc, output)?;
    Ok(pages.len() as u32)
}

/// Reads JPEG dimensions and component count without decoding pixels.
pub fn jpeg_info(jpeg: &[u8]) -> Result<(u32, u32, bool)> {
    let decoder = image::codecs::jpeg::JpegDecoder::new(Cursor::new(jpeg))
        .map_err(|_| EngineError::Message("Sayfa görüntüsü okunamadı.".into()))?;
    let (w, h) = decoder.dimensions();
    let gray = matches!(decoder.color_type(), image::ColorType::L8);
    Ok((w, h, gray))
}

/// Small preview for the file list (long side `max`), as JPEG bytes.
pub fn thumbnail(path: &Path, max: u32) -> Result<Vec<u8>> {
    let frame = load_frames(path)?.into_iter().next().ok_or(EngineError::NoPages)?;
    let small = frame.thumbnail(max, max);
    Ok(encode_jpeg(small, 80, None)?.0)
}
