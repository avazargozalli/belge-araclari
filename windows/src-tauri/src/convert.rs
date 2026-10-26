//! "PDF'e Dönüştür" and "Office'ten PDF'e": any supported file → PDF, one by one or merged.
//! PDFs are used as they are, images go through the image engine, UDF through WebView2 printing,
//! office files through an installed Microsoft Office / LibreOffice or the built-in Word reader.

use std::path::{Path, PathBuf};

use belge_core::office::{self, ExternalError};
use tauri::AppHandle;

use crate::engine::images::{self, ImageOptions, Margin, PageSize};
use crate::engine::output::{base_name, unique_file};
use crate::engine::{file_name, ops};
use crate::udf_print;

pub enum Outcome {
    /// A new PDF, its page count and how it was made (for office files).
    Made(PathBuf, u32, Option<String>),
    /// The input was already a PDF and stays as it is.
    Kept(PathBuf),
    Failed(String, String),
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Converts one office file; falls back to the built-in Word reader for .docx.
async fn office_to_pdf(app: &AppHandle, input: &Path, output: &Path, use_office: bool) -> Result<String, String> {
    let mut suite_error = None;
    if use_office {
        let (i, o) = (input.to_path_buf(), output.to_path_buf());
        let result = tauri::async_runtime::spawn_blocking(move || office::convert_external(&i, &o)).await.map_err(|e| e.to_string())?;
        match result {
            Ok(engine) => return Ok(engine.to_string()),
            Err(ExternalError::Unavailable) => {}
            Err(ExternalError::Failed(e)) => suite_error = Some(e),
        }
    }
    if !office::is_builtin(input) {
        return Err(suite_error.unwrap_or_else(|| {
            if use_office {
                format!("\"{}\" için Microsoft Office veya LibreOffice gerekir. Yerleşik dönüştürücü yalnızca Word (.docx) dosyalarını çevirebilir.", file_name(input))
            } else {
                format!("\"{}\" için kurulu Office kullanımı Ayarlar'da kapalı. Yerleşik dönüştürücü yalnızca Word (.docx) dosyalarını çevirebilir.", file_name(input))
            }
        }));
    }
    let temp = std::env::temp_dir().join(format!("belge-word-{}", crate::token()));
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let udf = temp.join(format!("{}.udf", base_name(input)));
    let result = async {
        let (i, u) = (input.to_path_buf(), udf.clone());
        tauri::async_runtime::spawn_blocking(move || belge_core::word::docx_to_udf(&i, &u)).await.map_err(|e| e.to_string())??;
        udf_print::convert(app, &udf, output).await
    }
    .await;
    let _ = std::fs::remove_dir_all(&temp);
    result?;
    Ok(if suite_error.is_some() { "yerleşik dönüştürücü (Office başarısız oldu)".into() } else { "yerleşik dönüştürücü".into() })
}

/// Writes one input as a PDF at `output`. Returns how it was made, for office files.
pub async fn to_pdf(app: &AppHandle, input: &Path, output: &Path, image_page: PageSize, use_office: bool) -> Result<Option<String>, String> {
    let ext = extension(input);
    if ext == "pdf" {
        std::fs::copy(input, output).map_err(|_| format!("\"{}\" kopyalanamadı.", file_name(input)))?;
        return Ok(None);
    }
    if images::is_supported(input) {
        let options = ImageOptions { page_size: image_page, margin: if image_page == PageSize::Fit { Margin::None } else { Margin::Small }, ..Default::default() };
        let (i, o) = (input.to_path_buf(), output.to_path_buf());
        tauri::async_runtime::spawn_blocking(move || images::images_to_pdf(&[i], &options, &o, &|_, _| {}))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        return Ok(None);
    }
    if ext == "udf" {
        udf_print::convert(app, input, output).await?;
        return Ok(None);
    }
    if office::is_office(input) {
        return office_to_pdf(app, input, output, use_office).await.map(Some);
    }
    Err(format!("\"{}\" desteklenmeyen bir dosya türü.", file_name(input)))
}

pub async fn convert_many(
    app: &AppHandle,
    inputs: &[PathBuf],
    merge: bool,
    merged: &Path,
    dir_for: impl Fn(&Path) -> PathBuf,
    image_page: PageSize,
    use_office: bool,
    progress: impl Fn(f64, &str),
) -> Vec<Outcome> {
    let temp = std::env::temp_dir().join(format!("belge-convert-{}", crate::token()));
    let _ = std::fs::create_dir_all(&temp);
    let mut results = vec![];
    let mut parts = vec![];
    for (i, input) in inputs.iter().enumerate() {
        progress(i as f64 / inputs.len().max(1) as f64 * if merge { 0.9 } else { 1.0 }, &format!("Dönüştürülüyor: {}", file_name(input)));
        if !merge && extension(input) == "pdf" {
            results.push(Outcome::Kept(input.clone()));
            continue;
        }
        let out = if merge { temp.join(format!("{i}.pdf")) } else { unique_file(&dir_for(input), &base_name(input), "pdf") };
        match to_pdf(app, input, &out, image_page, use_office).await {
            Ok(_) if merge => parts.push(out),
            Ok(how) => {
                let pages = ops::page_count(&out).map(|(p, _)| p).unwrap_or(0);
                results.push(Outcome::Made(out, pages, how));
            }
            Err(e) => results.push(Outcome::Failed(file_name(input), e)),
        }
    }
    if merge && !parts.is_empty() {
        progress(0.95, "Birleştiriliyor…");
        match ops::merge(&parts, merged, &|_, _| {}) {
            Ok(pages) => results.insert(0, Outcome::Made(merged.to_path_buf(), pages, Some(format!("{} dosya", parts.len())))),
            Err(e) => results.insert(0, Outcome::Failed(file_name(merged), e.to_string())),
        }
    }
    let _ = std::fs::remove_dir_all(&temp);
    results
}
