//! Engine tests. Fixtures are generated on the fly so the suite runs the same on Windows CI and locally.

use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use lopdf::{dictionary, Document, Object, Stream};

use super::compress::{self, Level};
use super::images::{self, ImageOptions, Margin, Orientation, PageSize, Quality};
use super::ops::{self, SplitMode};
use super::{EngineError, Progress};

const NO_PROGRESS: Progress = &|_, _| {};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("belge-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self, name: &str) -> PathBuf { self.0.join(name) }
}

impl Drop for TempDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

/// Noisy photo-like image so compression has something real to do.
fn photo(width: u32, height: u32) -> RgbImage {
    let mut seed = 0x1234_5678u32;
    RgbImage::from_fn(width, height, |x, y| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let noise = (seed % 40) as u8;
        Rgb([((x * 255 / width) as u8).saturating_add(noise), ((y * 255 / height) as u8).saturating_add(noise / 2), 120u8.saturating_add(noise)])
    })
}

/// PDF with `pages` pages, each a big lossless (Flate) image plus a line of text.
/// The page tree carries an inherited /Rotate 90 on page 2's parent to exercise inheritance.
fn heavy_pdf(path: &Path, pages: usize) {
    let mut doc = Document::with_version("1.7");
    let root_pages = doc.new_object_id();
    let rotated_pages = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let mut root_kids = Vec::new();
    let mut rotated_kids = Vec::new();
    for i in 0..pages {
        let img = photo(1600, 2000);
        let mut stream = Stream::new(
            dictionary! {
                "Type" => "XObject", "Subtype" => "Image", "Width" => 1600, "Height" => 2000,
                "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
            },
            img.into_raw(),
        );
        stream.compress().unwrap();
        let image_id = doc.add_object(stream);
        let content = format!("q 515 0 0 600 40 200 cm /Im0 Do Q BT /F1 24 Tf 40 100 Td (Sayfa {} metni) Tj ET", i + 1);
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let parent = if i == 1 { rotated_pages } else { root_pages };
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => parent, "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => image_id }, "Font" => dictionary! { "F1" => font } },
        });
        if i == 1 { rotated_kids.push(Object::Reference(page)) } else { root_kids.push(Object::Reference(page)) }
    }
    doc.objects.insert(rotated_pages, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Parent" => root_pages, "Kids" => rotated_kids.clone(), "Count" => rotated_kids.len() as i64, "Rotate" => 90,
    }));
    // Keep page order: page 2 lives in the rotated subtree, inserted at index 1.
    let mut kids = root_kids;
    if pages > 1 {
        kids.insert(1, Object::Reference(rotated_pages));
    }
    doc.objects.insert(root_pages, Object::Dictionary(dictionary! {
        "Type" => "Pages", "Kids" => kids, "Count" => pages as i64, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_pages });
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

fn rotation_of(doc: &Document, page: u32) -> i64 {
    let id = doc.get_pages()[&page];
    doc.get_dictionary(id).unwrap().get(b"Rotate").and_then(Object::as_i64).unwrap_or(0)
}

fn page_text(path: &Path, page: u32) -> String {
    Document::load(path).unwrap().extract_text(&[page]).unwrap_or_default()
}

#[test]
fn compress_shrinks_and_keeps_text_and_rotation() {
    let dir = TempDir::new("compress");
    let input = dir.path("agir.pdf");
    heavy_pdf(&input, 3);
    for level in [Level::Balanced, Level::Smaller, Level::Smallest] {
        let outcome = compress::compress(&input, level, false, &dir.0, NO_PROGRESS).unwrap();
        let out = outcome.output.expect("should be smaller");
        assert!(outcome.new_size < outcome.original_size / 3, "{level:?}: {} -> {}", outcome.original_size, outcome.new_size);
        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 3);
        assert!(page_text(&out, 1).contains("Sayfa 1"), "text kept");
        let rotate = doc.get_dictionary(doc.get_pages()[&2]).unwrap();
        let inherited = rotate.get(b"Rotate").and_then(Object::as_i64).ok()
            .or_else(|| doc.get_dictionary(rotate.get(b"Parent").unwrap().as_reference().unwrap()).unwrap().get(b"Rotate").and_then(Object::as_i64).ok());
        assert_eq!(inherited, Some(90));
        std::fs::remove_file(out).unwrap();
    }
}

#[test]
fn compress_does_not_enlarge_small_files() {
    let dir = TempDir::new("tiny");
    let input = dir.path("kucuk.pdf");
    let doc = images::image_pdf(&[images::JpegPage { jpeg: tiny_jpeg(), width: 8, height: 8, gray: false, page: (100.0, 100.0), rect: (0.0, 0.0, 100.0, 100.0) }], "x").unwrap();
    let mut doc = doc;
    ops::save(&mut doc, &input).unwrap();
    let outcome = compress::compress(&input, Level::Balanced, false, &dir.0, NO_PROGRESS).unwrap();
    assert!(outcome.output.is_none());
}

fn tiny_jpeg() -> Vec<u8> {
    images::encode_jpeg(DynamicImage::ImageRgb8(photo(8, 8)), 90, None).unwrap().0
}

#[test]
fn scan_mode_builds_image_pdf() {
    let dir = TempDir::new("scan");
    let input = dir.path("agir.pdf");
    heavy_pdf(&input, 2);
    let page = images::encode_jpeg(DynamicImage::ImageRgb8(photo(1240, 1754)), 55, None).unwrap().0;
    let outcome = compress::scan_pdf(&input, vec![(page.clone(), 595.0, 842.0), (page, 842.0, 595.0)], &dir.0).unwrap();
    let doc = Document::load(outcome.output.unwrap()).unwrap();
    assert_eq!(doc.get_pages().len(), 2);
}

#[test]
fn images_to_pdf_handles_formats_alpha_and_tiff_frames() {
    let dir = TempDir::new("images");
    let jpg = dir.path("foto.jpg");
    DynamicImage::ImageRgb8(photo(3000, 2000)).save(&jpg).unwrap();
    let png = dir.path("seffaf.png");
    RgbaImage::from_fn(600, 800, |x, _| Rgba([200, 30, 30, if x < 300 { 0 } else { 255 }])).save(&png).unwrap();
    let webp = dir.path("resim.webp");
    DynamicImage::ImageRgb8(photo(500, 500)).save(&webp).unwrap();
    let tif = dir.path("tarama.tiff");
    {
        let file = std::fs::File::create(&tif).unwrap();
        let mut encoder = tiff::encoder::TiffEncoder::new(file).unwrap();
        for _ in 0..3 {
            let frame = photo(400, 560);
            encoder.write_image::<tiff::encoder::colortype::RGB8>(400, 560, frame.as_raw()).unwrap();
        }
    }
    let out = dir.path("resimler.pdf");
    let options = ImageOptions { page_size: PageSize::A4, orientation: Orientation::Auto, margin: Margin::Small, quality: Quality::Balanced };
    let pages = images::images_to_pdf(&[jpg, png.clone(), webp, tif], &options, &out, NO_PROGRESS).unwrap();
    assert_eq!(pages, 6);
    let doc = Document::load(&out).unwrap();
    assert_eq!(doc.get_pages().len(), 6);
    let first = doc.get_dictionary(doc.get_pages()[&1]).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
    assert!(first[2].as_float().unwrap() > first[3].as_float().unwrap(), "landscape photo → landscape A4");

    // Transparent half of the PNG must come out white, not black.
    let thumb = images::thumbnail(&png, 64).unwrap();
    let decoded = image::load_from_memory(&thumb).unwrap().to_rgb8();
    assert!(decoded.get_pixel(2, 30)[0] > 240 && decoded.get_pixel(2, 30)[1] > 240);
}

#[test]
fn image_layout_fit_and_orientation() {
    let fit = ImageOptions { page_size: PageSize::Fit, margin: Margin::None, ..Default::default() };
    let (page, rect) = images::layout((1000.0, 500.0), &fit);
    assert!((page.0 - 841.89).abs() < 0.1 && (page.1 - 420.9).abs() < 0.2);
    assert_eq!(rect.0, 0.0);
    let portrait = ImageOptions { orientation: Orientation::Portrait, ..Default::default() };
    let (page, _) = images::layout((1000.0, 500.0), &portrait);
    assert!(page.1 > page.0);
}

#[test]
fn merge_split_and_organize() {
    let dir = TempDir::new("pages");
    let a = dir.path("a.pdf");
    let b = dir.path("b.pdf");
    heavy_pdf(&a, 3);
    heavy_pdf(&b, 2);

    let merged = dir.path("birlesik.pdf");
    assert_eq!(ops::merge(&[a.clone(), b.clone()], &merged, NO_PROGRESS).unwrap(), 5);
    let doc = Document::load(&merged).unwrap();
    assert_eq!(doc.get_pages().len(), 5);
    assert_eq!(rotation_of(&doc, 2), 90, "inherited rotation copied onto page");
    assert_eq!(rotation_of(&doc, 5), 90);
    assert!(page_text(&merged, 4).contains("Sayfa 1"));

    let (folder, count, is_folder) = ops::split(&merged, SplitMode::EachPage, &dir.0, NO_PROGRESS).unwrap();
    assert!(is_folder && count == 5);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 5);

    let (file, count, is_folder) = ops::split(&merged, SplitMode::Ranges { text: "1-2, 4".into(), separate: false }, &dir.0, NO_PROGRESS).unwrap();
    assert!(!is_folder && count == 1);
    assert_eq!(Document::load(&file).unwrap().get_pages().len(), 3);
    assert!(file.file_name().unwrap().to_string_lossy().contains("3 sayfa"));

    let (_, count, _) = ops::split(&merged, SplitMode::Ranges { text: "1-2; 4-".into(), separate: true }, &dir.0, NO_PROGRESS).unwrap();
    assert_eq!(count, 2);
    let (_, count, _) = ops::split(&merged, SplitMode::Every(2), &dir.0, NO_PROGRESS).unwrap();
    assert_eq!(count, 3);

    let organized = dir.path("duzen.pdf");
    ops::organize(&a, &[(3, 90), (1, 0), (2, 270)], &organized).unwrap();
    let doc = Document::load(&organized).unwrap();
    assert_eq!(doc.get_pages().len(), 3);
    assert_eq!(rotation_of(&doc, 1), 90);
    assert_eq!(rotation_of(&doc, 3), 0, "inherited 90 + 270");
    assert!(page_text(&organized, 1).contains("Sayfa 3"));
}

#[test]
fn ranges_are_validated() {
    assert_eq!(ops::parse_ranges("1-3, 5, 8-", 9).unwrap(), vec![vec![1, 2, 3], vec![5], vec![8, 9]]);
    assert_eq!(ops::parse_ranges("–2", 5).unwrap(), vec![vec![1, 2]]);
    for bad in ["0-3", "4-2", "x", "7", ""] {
        assert!(matches!(ops::parse_ranges(bad, 5), Err(EngineError::InvalidRange(_))), "{bad}");
    }
}

#[test]
fn encrypt_then_decrypt() {
    let dir = TempDir::new("crypto");
    let input = dir.path("dilekce.pdf");
    heavy_pdf(&input, 2);
    let locked = dir.path("sifreli.pdf");
    ops::encrypt(&input, "1234", "", &locked).unwrap();

    let probe = Document::load(&locked).unwrap();
    assert!(probe.is_encrypted() && probe.encryption_state.is_none(), "needs password to open");
    assert!(matches!(ops::open(&locked), Err(EngineError::Locked(_))));
    assert!(matches!(compress::compress(&locked, Level::Balanced, false, &dir.0, NO_PROGRESS), Err(EngineError::Locked(_))));
    assert!(ops::page_count(&locked).unwrap().1, "reported as locked");

    assert!(matches!(ops::decrypt(&locked, "yanlis", &dir.path("x.pdf")), Err(EngineError::WrongPassword(_))));
    let plain = dir.path("sifresiz.pdf");
    ops::decrypt(&locked, "1234", &plain).unwrap();
    let doc = Document::load(&plain).unwrap();
    assert!(!doc.is_encrypted());
    assert_eq!(doc.get_pages().len(), 2);
    assert!(page_text(&plain, 1).contains("Sayfa 1"));
    assert!(matches!(ops::decrypt(&plain, "1234", &dir.path("y.pdf")), Err(EngineError::Message(_))));
}

#[test]
fn creator_is_written_as_unicode() {
    let dir = TempDir::new("creator");
    let input = dir.path("a.pdf");
    heavy_pdf(&input, 1);
    let out = dir.path("b.pdf");
    ops::organize(&input, &[(1, 0)], &out).unwrap();
    let doc = Document::load(&out).unwrap();
    let info = doc.get_dictionary(doc.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap();
    let bytes = info.get(b"Creator").unwrap().as_str().unwrap();
    let units: Vec<u16> = bytes[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    assert_eq!(String::from_utf16(&units).unwrap(), super::CREATOR);
}

/// Real-world files from a local folder: `BELGE_FIXTURES=/path cargo test -- --ignored`
#[test]
#[ignore]
fn real_files_from_fixture_folder() {
    let Some(folder) = std::env::var_os("BELGE_FIXTURES") else { return };
    let out = TempDir::new("real");
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "pdf").unwrap_or(false) {
            let started = std::time::Instant::now();
            match compress::compress(&path, Level::Balanced, false, &out.0, NO_PROGRESS) {
                Ok(o) => println!("{:?}: {} -> {} ({:?}) {:?}", path.file_name().unwrap(), o.original_size, o.new_size, started.elapsed(), o.output.is_some()),
                Err(e) => println!("{:?}: {e}", path.file_name().unwrap()),
            }
        }
    }
}
