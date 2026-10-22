//! UDF → PDF on Windows: the HTML from `engine::udf` is loaded into an off-screen WebView2 window
//! and printed with `ICoreWebView2_7::PrintToPdf`. Text stays vector and selectable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::engine::udf::{self, UdfDocument};
use crate::engine::{file_name, ops};

/// HTML pages waiting to be printed, served through the `belgeprint` protocol.
#[derive(Default)]
pub struct PrintPages(pub Mutex<HashMap<String, String>>);

pub const SCHEME: &str = "belgeprint";

pub fn serve(app: &AppHandle, path: &str) -> Option<String> {
    let key = path.trim_start_matches('/').trim_end_matches(".html").to_string();
    app.state::<PrintPages>().0.lock().ok()?.remove(&key)
}

fn page_url(key: &str) -> String {
    // WebView2 serves custom schemes as http://<scheme>.localhost
    if cfg!(windows) { format!("http://{SCHEME}.localhost/{key}.html") } else { format!("{SCHEME}://localhost/{key}.html") }
}

/// Loads `html` in a hidden window, waits until its script reports `#ready=…`, prints it and returns
/// the margins the page reported (top, bottom) in points.
async fn print_html(app: &AppHandle, html: String, output: &Path, doc: &UdfDocument, margins: Option<(f32, f32)>) -> Result<(f32, f32), String> {
    let key = crate::token();
    app.state::<PrintPages>().0.lock().map_err(|e| e.to_string())?.insert(key.clone(), html);
    let url = tauri::Url::parse(&page_url(&key)).map_err(|e| e.to_string())?;
    let label = format!("print-{key}");
    let window = tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External(url))
        .title("Belge Araçları")
        .inner_size(900.0, 700.0)
        .position(-32000.0, -32000.0)
        .skip_taskbar(true)
        .decorations(false)
        .focused(false)
        .visible(true)
        .build()
        .map_err(|e| e.to_string())?;

    let result = async {
        let started = Instant::now();
        let ready = loop {
            if let Ok(url) = window.url() {
                if let Some(fragment) = url.fragment() {
                    if fragment.starts_with("ready=") {
                        break udf::ready_margins(fragment);
                    }
                }
            }
            if started.elapsed() > Duration::from_secs(60) {
                return Err("Belge hazırlanırken zaman aşımı oluştu.".to_string());
            }
            tokio_sleep(Duration::from_millis(100)).await;
        };
        let (top, bottom) = margins.or(ready).unwrap_or((doc.margins[0], doc.margins[2]));
        let page = (doc.page_width, doc.page_height);
        let side = (doc.margins[3], doc.margins[1]);
        print_to_pdf(&window, output, page, (top, bottom), side).await?;
        Ok((top, bottom))
    }
    .await;
    let _ = window.destroy();
    result
}

async fn tokio_sleep(duration: Duration) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(duration)).await;
}

#[cfg(windows)]
async fn print_to_pdf(window: &tauri::WebviewWindow, output: &Path, page: (f32, f32), vertical: (f32, f32), horizontal: (f32, f32)) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment6, ICoreWebView2_2, ICoreWebView2_7, COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT,
    };
    use webview2_com::PrintToPdfCompletedHandler;
    use windows::core::{Interface, HSTRING};

    let inch = |pt: f32| f64::from(pt) / 72.0;
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let path = HSTRING::from(output.as_os_str());
    let failed = tx.clone();
    window
        .with_webview(move |platform| unsafe {
            let run = || -> windows::core::Result<()> {
                let core = platform.controller().CoreWebView2()?;
                let environment = core.cast::<ICoreWebView2_2>()?.Environment()?;
                let settings = environment.cast::<ICoreWebView2Environment6>()?.CreatePrintSettings()?;
                settings.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
                settings.SetPageWidth(inch(page.0))?;
                settings.SetPageHeight(inch(page.1))?;
                settings.SetMarginTop(inch(vertical.0))?;
                settings.SetMarginBottom(inch(vertical.1))?;
                settings.SetMarginLeft(inch(horizontal.0))?;
                settings.SetMarginRight(inch(horizontal.1))?;
                settings.SetScaleFactor(1.0)?;
                settings.SetShouldPrintBackgrounds(true.into())?;
                settings.SetShouldPrintHeaderAndFooter(false.into())?;
                settings.SetShouldPrintSelectionOnly(false.into())?;
                let done = tx.clone();
                let handler = PrintToPdfCompletedHandler::create(Box::new(move |result, ok| {
                    let _ = done.send(match result {
                        Ok(()) if ok => Ok(()),
                        Ok(()) => Err("PDF yazdırılamadı.".to_string()),
                        Err(e) => Err(e.message().to_string()),
                    });
                    Ok(())
                }));
                core.cast::<ICoreWebView2_7>()?.PrintToPdf(&path, &settings, &handler)
            };
            if let Err(e) = run() {
                let _ = failed.send(Err(format!("WebView2 yazdırma hatası: {}", e.message())));
            }
        })
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| Err("PDF yazdırma zaman aşımına uğradı.".into())))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(not(windows))]
async fn print_to_pdf(_: &tauri::WebviewWindow, _: &Path, _: (f32, f32), _: (f32, f32), _: (f32, f32)) -> Result<(), String> {
    Err("UDF dönüştürme bu sürümde yalnızca Windows'ta desteklenir; macOS için yerel uygulamayı kullanın.".into())
}

/// Converts one UDF file to `output`. Returns the page count.
pub async fn convert(app: &AppHandle, input: &Path, output: &Path) -> Result<u32, String> {
    let doc = {
        let input = input.to_path_buf();
        tauri::async_runtime::spawn_blocking(move || udf::parse_file(&input)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?
    };
    let temp = std::env::temp_dir().join(format!("belge-udf-{}", crate::token()));
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let result = async {
        let body_path = temp.join("body.pdf");
        print_html(app, udf::body_html(&doc), &body_path, &doc, None).await?;
        let mut body = lopdf::Document::load(&body_path).map_err(|_| "Oluşan PDF okunamadı.".to_string())?;
        let pages = body.get_pages().len();
        let deco_path = temp.join("deco.pdf");
        print_html(app, udf::decoration_html(&doc, pages), &deco_path, &doc, Some((0.0, 0.0))).await?;
        let deco = lopdf::Document::load(&deco_path).map_err(|_| "Antet sayfası okunamadı.".to_string())?;
        udf::underlay(&mut body, deco).map_err(|e| e.to_string())?;
        ops::save(&mut body, output).map_err(|e| e.to_string())?;
        Ok(pages as u32)
    }
    .await;
    let _ = std::fs::remove_dir_all(&temp);
    result.map_err(|e: String| format!("\"{}\": {e}", file_name(input)))
}

/// Converts several UDFs; with `merge` they become one PDF in the given order.
pub async fn convert_many(app: &AppHandle, inputs: &[PathBuf], merge: bool, merged: &Path, dir_for: impl Fn(&Path) -> PathBuf, progress: impl Fn(f64, &str)) -> Vec<Result<(PathBuf, u32), (String, String)>> {
    let temp = std::env::temp_dir().join(format!("belge-udf-many-{}", crate::token()));
    let _ = std::fs::create_dir_all(&temp);
    let mut results = Vec::new();
    let mut rendered = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        progress(i as f64 / inputs.len() as f64, &format!("Dönüştürülüyor: {}", file_name(input)));
        let out = if merge {
            temp.join(format!("{i}.pdf"))
        } else {
            crate::engine::output::unique_file(&dir_for(input), &crate::engine::output::base_name(input), "pdf")
        };
        match convert(app, input, &out).await {
            Ok(_) if merge => rendered.push(out),
            Ok(pages) => results.push(Ok((out, pages))),
            Err(e) => results.push(Err((file_name(input), e))),
        }
    }
    if merge && !rendered.is_empty() {
        progress(0.95, "Birleştiriliyor…");
        match ops::merge(&rendered, merged, &|_, _| {}) {
            Ok(pages) => results.insert(0, Ok((merged.to_path_buf(), pages))),
            Err(e) => results.insert(0, Err((file_name(merged), e.to_string()))),
        }
    }
    let _ = std::fs::remove_dir_all(&temp);
    results
}
