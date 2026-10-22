//! PDF compression. Images are decoded, downscaled and re-encoded as JPEG in parallel; text and
//! vector content are left untouched. Optionally Ghostscript (if installed) is tried as well and the
//! smallest valid result wins. If nothing is smaller than the original, no file is written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use image::{DynamicImage, GrayImage, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use rayon::prelude::*;
use serde::Deserialize;

use super::images::{encode_jpeg, image_pdf, jpeg_info, JpegPage};
use super::ops::{open, stamp_creator, to_bytes};
use super::output::{base_name, unique_file, write_bytes};
use super::{file_size, Progress, Result};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Level { Balanced, Smaller, Smallest }

impl Level {
    pub fn jpeg_quality(self) -> u8 {
        match self {
            Level::Balanced => 72,
            Level::Smaller => 55,
            Level::Smallest => 40,
        }
    }
    /// Longest side of any embedded image after compression.
    pub fn max_pixel(self) -> u32 {
        match self {
            Level::Balanced => 2000,
            Level::Smaller => 1500,
            Level::Smallest => 1100,
        }
    }
    /// Resolution for scanned-document mode (pages rendered by the UI).
    pub fn scan_dpi(self) -> u32 {
        match self {
            Level::Balanced => 150,
            Level::Smaller => 120,
            Level::Smallest => 96,
        }
    }
    fn ghostscript_args(self) -> Vec<String> {
        let mut args = vec![format!("-dPDFSETTINGS={}", if self == Level::Balanced { "/ebook" } else { "/screen" })];
        if self != Level::Balanced {
            let (color, mono) = if self == Level::Smallest { (72, 150) } else { (110, 200) };
            args.push(format!("-dColorImageResolution={color}"));
            args.push(format!("-dGrayImageResolution={color}"));
            args.push(format!("-dMonoImageResolution={mono}"));
        }
        args
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub output: Option<PathBuf>,
    pub original_size: u64,
    pub new_size: u64,
    pub method: String,
}

pub fn compress(input: &Path, level: Level, use_ghostscript: bool, dir: &Path, progress: Progress) -> Result<Outcome> {
    let original_size = file_size(input);
    let mut doc = open(input)?;
    let page_count = doc.get_pages().len();
    let mut candidates: Vec<(Vec<u8>, &str)> = Vec::new();

    progress(0.05, "Görseller sıkıştırılıyor…");
    recompress_images(&mut doc, level, progress);
    progress(0.75, "Dosya yapısı optimize ediliyor…");
    doc.prune_objects();
    doc.delete_zero_length_streams();
    stamp_creator(&mut doc);
    if let Ok(bytes) = to_bytes(&mut doc, true) {
        candidates.push((bytes, "Yerleşik motor"));
    }
    drop(doc);

    if use_ghostscript {
        if let Some(gs) = ghostscript_path() {
            progress(0.8, "Ghostscript ile sıkıştırılıyor…");
            if let Some(bytes) = run_ghostscript(&gs, input, level) {
                candidates.push((bytes, "Ghostscript"));
            }
        }
    }

    progress(0.95, "Sonuç doğrulanıyor…");
    let best = candidates
        .into_iter()
        .filter(|(bytes, _)| Document::load_mem(bytes).map(|d| d.get_pages().len() == page_count).unwrap_or(false))
        .min_by_key(|(bytes, _)| bytes.len());

    finish(input, best, original_size, dir, "küçültülmüş")
}

/// Scanned-document mode: the UI renders each page to JPEG; this builds the PDF from them.
/// `pages` are (jpeg bytes, page width pt, page height pt).
pub fn scan_pdf(input: &Path, pages: Vec<(Vec<u8>, f32, f32)>, dir: &Path) -> Result<Outcome> {
    let original_size = file_size(input);
    let mut jpeg_pages = Vec::with_capacity(pages.len());
    for (jpeg, w, h) in pages {
        let (width, height, gray) = jpeg_info(&jpeg)?;
        jpeg_pages.push(JpegPage { jpeg, width, height, gray, page: (w, h), rect: (0.0, 0.0, w, h) });
    }
    let mut doc = image_pdf(&jpeg_pages, &base_name(input))?;
    stamp_creator(&mut doc);
    let bytes = to_bytes(&mut doc, true)?;
    finish(input, Some((bytes, "Taranmış belge modu")), original_size, dir, "küçültülmüş-taranmış")
}

fn finish(input: &Path, best: Option<(Vec<u8>, &str)>, original_size: u64, dir: &Path, suffix: &str) -> Result<Outcome> {
    match best {
        Some((bytes, method)) if (bytes.len() as u64) < original_size => {
            let out = unique_file(dir, &format!("{} - {suffix}", base_name(input)), "pdf");
            write_bytes(&out, &bytes)?;
            Ok(Outcome { output: Some(out), original_size, new_size: bytes.len() as u64, method: method.into() })
        }
        _ => Ok(Outcome { output: None, original_size, new_size: original_size, method: String::new() }),
    }
}

// MARK: - Image recompression

struct ImageJob {
    id: ObjectId,
    dict: Dictionary,
    content: Vec<u8>,
    is_jpeg: bool,
    width: u32,
    height: u32,
    components: u8,
}

fn filter_names(dict: &Dictionary) -> Vec<Vec<u8>> {
    match dict.get(b"Filter") {
        Ok(Object::Name(name)) => vec![name.clone()],
        Ok(Object::Array(items)) => items.iter().filter_map(|o| o.as_name().ok().map(|n| n.to_vec())).collect(),
        _ => vec![],
    }
}

fn components(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let mut space = dict.get(b"ColorSpace").ok()?;
    if let Ok(id) = space.as_reference() {
        space = doc.get_object(id).ok()?;
    }
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceRGB" | b"CalRGB" => Some(3),
            b"DeviceGray" | b"CalGray" => Some(1),
            _ => None,
        },
        Object::Array(items) => {
            let kind = items.first()?.as_name().ok()?;
            match kind {
                b"CalRGB" => Some(3),
                b"CalGray" => Some(1),
                b"ICCBased" => {
                    let stream = doc.get_object(items.get(1)?.as_reference().ok()?).ok()?.as_stream().ok()?;
                    match stream.dict.get(b"N").and_then(Object::as_i64).ok()? {
                        1 => Some(1),
                        3 => Some(3),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn collect_jobs(doc: &Document) -> Vec<ImageJob> {
    let soft_masks: HashSet<ObjectId> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter_map(|s| s.dict.get(b"SMask").and_then(Object::as_reference).ok())
        .collect();

    doc.objects
        .iter()
        .filter_map(|(&id, object)| {
            let stream = object.as_stream().ok()?;
            let dict = &stream.dict;
            if dict.get(b"Subtype").and_then(Object::as_name).ok()? != b"Image" || soft_masks.contains(&id) {
                return None;
            }
            if dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) || dict.has(b"Decode") {
                return None;
            }
            if dict.get(b"BitsPerComponent").and_then(Object::as_i64).ok()? != 8 {
                return None;
            }
            let filters = filter_names(dict);
            let is_jpeg = filters == [b"DCTDecode".to_vec()];
            let lossless = filters.is_empty() || filters == [b"FlateDecode".to_vec()] || filters == [b"LZWDecode".to_vec()];
            if !is_jpeg && !lossless {
                return None;
            }
            let width = dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
            let height = dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
            // Tiny images (icons, logos) are not worth touching.
            if width.max(height) < 300 || stream.content.len() < 20_000 {
                return None;
            }
            Some(ImageJob {
                id,
                dict: dict.clone(),
                content: stream.content.clone(),
                is_jpeg,
                width,
                height,
                components: components(doc, dict)?,
            })
        })
        .collect()
}

fn recompress(job: &ImageJob, level: Level) -> Option<(Vec<u8>, u32, u32)> {
    let image = if job.is_jpeg {
        let decoded = image::load_from_memory_with_format(&job.content, image::ImageFormat::Jpeg).ok()?;
        let channels = decoded.color().channel_count();
        if (job.components == 1) != (channels == 1) {
            return None;
        }
        decoded
    } else {
        let raw = Stream::new(job.dict.clone(), job.content.clone()).decompressed_content().ok()?;
        let needed = job.width as usize * job.height as usize * job.components as usize;
        if raw.len() < needed {
            return None;
        }
        let pixels = raw[..needed].to_vec();
        if job.components == 1 {
            DynamicImage::ImageLuma8(GrayImage::from_raw(job.width, job.height, pixels)?)
        } else {
            DynamicImage::ImageRgb8(RgbImage::from_raw(job.width, job.height, pixels)?)
        }
    };
    let (jpeg, width, height, _) = encode_jpeg(image, level.jpeg_quality(), Some(level.max_pixel())).ok()?;
    // Keep the original unless the gain is meaningful.
    if jpeg.len() as f64 > job.content.len() as f64 * 0.9 {
        return None;
    }
    Some((jpeg, width, height))
}

/// Returns how many images were replaced.
pub fn recompress_images(doc: &mut Document, level: Level, progress: Progress) -> usize {
    let jobs = collect_jobs(doc);
    let total = jobs.len().max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<(ObjectId, Vec<u8>, u32, u32)> = jobs
        .par_iter()
        .filter_map(|job| {
            let result = recompress(job, level).map(|(jpeg, w, h)| (job.id, jpeg, w, h));
            let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress(0.05 + 0.7 * n as f64 / total as f64, &format!("Görsel {n} / {total}"));
            result
        })
        .collect();

    let replaced = results.len();
    for (id, jpeg, width, height) in results {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
            stream.dict.set("Filter", "DCTDecode");
            stream.dict.remove(b"DecodeParms");
            stream.dict.set("Width", width as i64);
            stream.dict.set("Height", height as i64);
            stream.dict.set("BitsPerComponent", 8);
            stream.set_content(jpeg);
            stream.allows_compression = false;
        }
    }
    replaced
}

// MARK: - Ghostscript (optional, only if the user installed it)

pub fn ghostscript_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut found: Vec<PathBuf> = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            let Some(root) = std::env::var_os(var) else { continue };
            let Ok(entries) = std::fs::read_dir(PathBuf::from(root).join("gs")) else { continue };
            for entry in entries.flatten() {
                for exe in ["gswin64c.exe", "gswin32c.exe"] {
                    let candidate = entry.path().join("bin").join(exe);
                    if candidate.is_file() {
                        found.push(candidate);
                    }
                }
            }
        }
        found.sort();
        found.pop()
    }
    #[cfg(not(windows))]
    {
        ["/opt/homebrew/bin/gs", "/usr/local/bin/gs", "/usr/bin/gs"].iter().map(PathBuf::from).find(|p| p.is_file())
    }
}

fn run_ghostscript(gs: &Path, input: &Path, level: Level) -> Option<Vec<u8>> {
    let out = std::env::temp_dir().join(format!("belge-araclari-gs-{}-{}.pdf", std::process::id(), rand_suffix()));
    let mut command = Command::new(gs);
    command
        .args(["-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.5", "-dDetectDuplicateImages=true"])
        .args(["-dCompressFonts=true", "-dSubsetFonts=true", "-dSAFER", "-dNOPAUSE", "-dQUIET", "-dBATCH"])
        .args(level.ghostscript_args())
        .arg(format!("-sOutputFile={}", out.display()))
        .arg(input);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let ok = command.output().map(|o| o.status.success()).unwrap_or(false);
    let bytes = if ok { std::fs::read(&out).ok() } else { None };
    let _ = std::fs::remove_file(&out);
    bytes.filter(|b| !b.is_empty())
}

fn rand_suffix() -> u64 {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    u64::from_le_bytes(bytes)
}

