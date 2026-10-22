//! Belge Araçları for Windows — Tauri shell around the local PDF engine.
//! No command here performs network access; files are only read from and written to disk.

pub mod engine;
mod udf_print;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Emitter, Manager, State};

use engine::compress::{self, Level};
use engine::images::{self, ImageOptions};
use engine::ops::{self, SplitMode};
use engine::output::{base_name, directory_for, unique_file, unique_folder};
use engine::{file_name, file_size, EngineError};

type CmdResult<T> = Result<T, String>;

fn err(e: EngineError) -> String {
    e.to_string()
}

// MARK: - Shared types

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    path: String,
    name: String,
    size: u64,
    pages: u32,
    locked: bool,
    kind: &'static str,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ResultItem {
    title: String,
    detail: String,
    path: Option<String>,
    is_folder: bool,
    success: bool,
    original_size: Option<u64>,
    new_size: Option<u64>,
}

impl ResultItem {
    fn ok(path: &Path, detail: String) -> Self {
        ResultItem { title: file_name(path), detail, path: Some(path.display().to_string()), success: true, ..Default::default() }
    }
    fn failed(title: String, detail: String) -> Self {
        ResultItem { title, detail, ..Default::default() }
    }
}

#[derive(Serialize, Clone)]
struct ProgressEvent {
    job: String,
    fraction: f64,
    status: String,
}

fn reporter(app: &AppHandle, job: &str) -> impl Fn(f64, &str) + Sync {
    let app = app.clone();
    let job = job.to_string();
    move |fraction, status| {
        let _ = app.emit("progress", ProgressEvent { job: job.clone(), fraction, status: status.to_string() });
    }
}

fn chosen_dir(out_dir: &Option<String>) -> Option<PathBuf> {
    out_dir.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())
}

fn is_pdf(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("pdf")).unwrap_or(false)
}

fn is_udf(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("udf")).unwrap_or(false)
}

/// Scratch state for multi-step jobs whose pages are rendered by the UI.
#[derive(Default)]
struct Sessions {
    scans: Mutex<HashMap<String, Vec<(Vec<u8>, f32, f32)>>>,
    exports: Mutex<HashMap<String, (PathBuf, String, usize)>>,
    allowed_reads: Mutex<HashSet<PathBuf>>,
}

pub(crate) fn token() -> String {
    let mut bytes = [0u8; 12];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn header<'a>(request: &'a Request<'_>, name: &str) -> CmdResult<&'a str> {
    request.headers().get(name).and_then(|v| v.to_str().ok()).ok_or_else(|| format!("{name} eksik"))
}

// MARK: - Files

#[tauri::command]
async fn file_info(paths: Vec<String>, sessions: State<'_, Sessions>) -> CmdResult<Vec<FileInfo>> {
    let expanded: Vec<PathBuf> = paths
        .into_iter()
        .map(PathBuf::from)
        .flat_map(|p| {
            if p.is_dir() {
                let mut items: Vec<PathBuf> = std::fs::read_dir(&p).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
                items.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
                items
            } else {
                vec![p]
            }
        })
        .filter(|p| is_pdf(p) || is_udf(p) || images::is_supported(p))
        .collect();
    {
        let mut allowed = sessions.allowed_reads.lock().unwrap();
        allowed.extend(expanded.iter().cloned());
    }
    blocking(move || {
        expanded
            .into_iter()
            .map(|p| {
                let pdf = is_pdf(&p);
                let (pages, locked, kind) = if pdf {
                    let (pages, locked) = ops::page_count(&p).unwrap_or((0, false));
                    (pages, locked, "pdf")
                } else if is_udf(&p) {
                    (0, false, "udf")
                } else {
                    (tiff_pages(&p), false, "image")
                };
                FileInfo { name: file_name(&p), size: file_size(&p), pages, locked, kind, path: p.display().to_string() }
            })
            .collect()
    })
    .await
}

fn tiff_pages(path: &Path) -> u32 {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "tif" && ext != "tiff" {
        return 1;
    }
    images::load_frames(path).map(|f| f.len() as u32).unwrap_or(1)
}

/// Only files the user picked or dropped (registered through `file_info`) can be read back.
fn check_readable(sessions: &Sessions, path: &Path) -> CmdResult<()> {
    if sessions.allowed_reads.lock().unwrap().contains(path) { Ok(()) } else { Err("Bu dosyaya erişim izni yok.".into()) }
}

#[tauri::command]
async fn read_pdf(path: String, sessions: State<'_, Sessions>) -> CmdResult<Response> {
    let path = PathBuf::from(path);
    check_readable(&sessions, &path)?;
    if !is_pdf(&path) {
        return Err("Yalnızca PDF okunabilir.".into());
    }
    let bytes = blocking(move || std::fs::read(&path)).await?.map_err(|e| e.to_string())?;
    Ok(Response::new(bytes))
}

#[tauri::command]
async fn thumbnail(path: String, sessions: State<'_, Sessions>) -> CmdResult<Response> {
    let path = PathBuf::from(path);
    check_readable(&sessions, &path)?;
    let bytes = blocking(move || images::thumbnail(&path, 160)).await?.map_err(err)?;
    Ok(Response::new(bytes))
}

#[tauri::command]
fn open_path(path: String) -> CmdResult<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_path(path: String) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn ghostscript() -> Option<String> {
    compress::ghostscript_path().map(|p| p.display().to_string())
}

#[tauri::command]
fn initial_files() -> Vec<String> {
    if cli_udf_request().is_some() {
        return vec![];
    }
    std::env::args().skip(1).filter(|a| !a.starts_with('-') && Path::new(a).exists()).collect()
}

// MARK: - Compress

#[tauri::command]
async fn compress(app: AppHandle, job: String, paths: Vec<String>, level: Level, use_ghostscript: bool, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let total = paths.len();
        let progress = reporter(&app, &job);
        paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let input = PathBuf::from(path);
                let prefix = if total > 1 { format!("({}/{total}) ", i + 1) } else { String::new() };
                let sub = |f: f64, s: &str| progress((i as f64 + f) / total as f64, &format!("{prefix}{s}"));
                let dir = directory_for(&input, chosen_dir(&out_dir).as_deref());
                match compress::compress(&input, level, use_ghostscript, &dir, &sub) {
                    Ok(outcome) => compression_result(&input, outcome),
                    Err(e) => ResultItem::failed(file_name(&input), e.to_string()),
                }
            })
            .collect()
    })
    .await
}

fn compression_result(input: &Path, outcome: compress::Outcome) -> ResultItem {
    match outcome.output {
        Some(out) => {
            let saved = 100 - ((outcome.new_size as f64 / outcome.original_size.max(1) as f64) * 100.0).round() as i64;
            ResultItem {
                original_size: Some(outcome.original_size),
                new_size: Some(outcome.new_size),
                ..ResultItem::ok(&out, format!("%{saved} daha küçük · {}", outcome.method))
            }
        }
        None => ResultItem {
            original_size: Some(outcome.original_size),
            ..ResultItem::failed(file_name(input), "Bu dosya zaten optimize; daha fazla küçültülemedi. Orijinal korunuyor.".into())
        },
    }
}

#[tauri::command]
fn scan_begin(sessions: State<'_, Sessions>) -> String {
    let t = token();
    sessions.scans.lock().unwrap().insert(t.clone(), Vec::new());
    t
}

#[tauri::command]
fn scan_page(request: Request<'_>, sessions: State<'_, Sessions>) -> CmdResult<()> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err("Sayfa verisi eksik".into()) };
    let t = header(&request, "x-token")?;
    let w: f32 = header(&request, "x-width")?.parse().map_err(|_| "x-width")?;
    let h: f32 = header(&request, "x-height")?.parse().map_err(|_| "x-height")?;
    let mut scans = sessions.scans.lock().unwrap();
    scans.get_mut(t).ok_or("Oturum bulunamadı")?.push((bytes.clone(), w, h));
    Ok(())
}

#[tauri::command]
async fn scan_finish(token: String, path: String, out_dir: Option<String>, sessions: State<'_, Sessions>) -> CmdResult<ResultItem> {
    let pages = sessions.scans.lock().unwrap().remove(&token).ok_or("Oturum bulunamadı")?;
    blocking(move || {
        let input = PathBuf::from(&path);
        let dir = directory_for(&input, chosen_dir(&out_dir).as_deref());
        match compress::scan_pdf(&input, pages, &dir) {
            Ok(outcome) => compression_result(&input, outcome),
            Err(e) => ResultItem::failed(file_name(&input), e.to_string()),
        }
    })
    .await
}

#[tauri::command]
fn scan_dpi(level: Level) -> u32 {
    level.scan_dpi()
}

// MARK: - Images → PDF

#[tauri::command]
async fn images_to_pdf(app: AppHandle, job: String, paths: Vec<String>, options: ImageOptions, name: String, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let inputs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let first = inputs.first().ok_or("Resim yok")?;
        let base = if name.trim().is_empty() { base_name(first) } else { name.trim().to_string() };
        let out = unique_file(&directory_for(first, chosen_dir(&out_dir).as_deref()), &base, "pdf");
        let pages = images::images_to_pdf(&inputs, &options, &out, &reporter(&app, &job)).map_err(err)?;
        Ok(vec![ResultItem::ok(&out, format!("{pages} sayfa · {}", human_size(file_size(&out))))])
    })
    .await?
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_000_000.0 { format!("{:.1} MB", b / 1_000_000.0).replace('.', ",") } else { format!("{:.0} KB", (b / 1000.0).max(1.0)) }
}

// MARK: - Merge / split / organize

#[tauri::command]
async fn merge(app: AppHandle, job: String, paths: Vec<String>, name: String, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let inputs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let first = inputs.first().ok_or("PDF yok")?;
        let base = if name.trim().is_empty() { "Birleştirilmiş belge".to_string() } else { name.trim().to_string() };
        let out = unique_file(&directory_for(first, chosen_dir(&out_dir).as_deref()), &base, "pdf");
        let pages = ops::merge(&inputs, &out, &reporter(&app, &job)).map_err(err)?;
        Ok(vec![ResultItem::ok(&out, format!("{} dosya · {pages} sayfa · {}", inputs.len(), human_size(file_size(&out))))])
    })
    .await?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SplitRequest {
    kind: String,
    ranges: String,
    separate: bool,
    every: u32,
}

#[tauri::command]
async fn split(app: AppHandle, job: String, path: String, mode: SplitRequest, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let input = PathBuf::from(&path);
        let split_mode = match mode.kind.as_str() {
            "ranges" => SplitMode::Ranges { text: mode.ranges, separate: mode.separate },
            "every" => SplitMode::Every(mode.every),
            _ => SplitMode::EachPage,
        };
        let dir = directory_for(&input, chosen_dir(&out_dir).as_deref());
        let (out, count, is_folder) = ops::split(&input, split_mode, &dir, &reporter(&app, &job)).map_err(err)?;
        let detail = if is_folder { format!("{count} PDF dosyası oluşturuldu") } else { human_size(file_size(&out)) };
        Ok(vec![ResultItem { is_folder, ..ResultItem::ok(&out, detail) }])
    })
    .await?
}

#[tauri::command]
async fn organize(path: String, pages: Vec<(u32, i64)>, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let input = PathBuf::from(&path);
        let dir = directory_for(&input, chosen_dir(&out_dir).as_deref());
        let out = unique_file(&dir, &format!("{} - düzenlenmiş", base_name(&input)), "pdf");
        let count = ops::organize(&input, &pages, &out).map_err(err)?;
        Ok(vec![ResultItem::ok(&out, format!("{count} sayfa · {}", human_size(file_size(&out))))])
    })
    .await?
}

// MARK: - PDF → images

#[tauri::command]
fn export_begin(path: String, pages: usize, out_dir: Option<String>, sessions: State<'_, Sessions>) -> CmdResult<(String, String)> {
    let input = PathBuf::from(&path);
    let base = base_name(&input);
    let folder = unique_folder(&directory_for(&input, chosen_dir(&out_dir).as_deref()), &format!("{base} - görseller")).map_err(err)?;
    let t = token();
    sessions.exports.lock().unwrap().insert(t.clone(), (folder.clone(), base, pages.to_string().len()));
    Ok((t, folder.display().to_string()))
}

#[tauri::command]
fn export_page(request: Request<'_>, sessions: State<'_, Sessions>) -> CmdResult<()> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err("Görsel verisi eksik".into()) };
    let t = header(&request, "x-token")?;
    let index: usize = header(&request, "x-index")?.parse().map_err(|_| "x-index")?;
    let ext = match header(&request, "x-ext")? {
        "png" => "png",
        _ => "jpg",
    };
    let exports = sessions.exports.lock().unwrap();
    let (folder, base, digits) = exports.get(t).ok_or("Oturum bulunamadı")?;
    let out = unique_file(folder, &format!("{base} - sayfa {index:0digits$}", digits = *digits), ext);
    engine::output::write_bytes(&out, bytes).map_err(err)
}

#[tauri::command]
fn export_finish(token: String, sessions: State<'_, Sessions>) {
    sessions.exports.lock().unwrap().remove(&token);
}

// MARK: - UDF → PDF

#[tauri::command]
async fn udf_to_pdf(app: AppHandle, job: String, paths: Vec<String>, merge: bool, name: String, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    let inputs: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    let first = inputs.first().ok_or("UDF yok")?.clone();
    let merge = merge && inputs.len() > 1;
    let chosen = chosen_dir(&out_dir);
    let base = if name.trim().is_empty() { base_name(&first) } else { name.trim().to_string() };
    let merged = unique_file(&directory_for(&first, chosen.as_deref()), &base, "pdf");
    let progress = reporter(&app, &job);
    let results = udf_print::convert_many(&app, &inputs, merge, &merged, |p| directory_for(p, chosen.as_deref()), &progress).await;
    Ok(results
        .into_iter()
        .map(|r| match r {
            Ok((path, pages)) => {
                let detail = if merge && path == merged { format!("{} UDF · {pages} sayfa · {}", inputs.len(), human_size(file_size(&path))) } else { format!("{pages} sayfa · {}", human_size(file_size(&path))) };
                ResultItem::ok(&path, detail)
            }
            Err((title, detail)) => ResultItem::failed(title, detail),
        })
        .collect())
}

/// `BelgeAraclari.exe --udf-to-pdf input.udf output.pdf` converts without showing a window
/// (used for batch work and by the Windows CI test).
fn cli_udf_request() -> Option<(PathBuf, PathBuf)> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--udf-to-pdf")?;
    Some((PathBuf::from(args.get(i + 1)?), PathBuf::from(args.get(i + 2)?)))
}

// MARK: - Passwords

#[tauri::command]
async fn protect(app: AppHandle, job: String, paths: Vec<String>, encrypt: bool, password: String, owner_password: String, out_dir: Option<String>) -> CmdResult<Vec<ResultItem>> {
    blocking(move || {
        let progress = reporter(&app, &job);
        paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let input = PathBuf::from(path);
                progress(i as f64 / paths.len() as f64, &file_name(&input));
                let suffix = if encrypt { "şifreli" } else { "şifresiz" };
                let out = unique_file(&directory_for(&input, chosen_dir(&out_dir).as_deref()), &format!("{} - {suffix}", base_name(&input)), "pdf");
                let result = if encrypt { ops::encrypt(&input, &password, &owner_password, &out) } else { ops::decrypt(&input, &password, &out) };
                match result {
                    Ok(()) => ResultItem::ok(&out, if encrypt { "Açılış şifresi eklendi (AES-256)".into() } else { "Şifre kaldırıldı".into() }),
                    Err(e) => ResultItem::failed(file_name(&input), e.to_string()),
                }
            })
            .collect()
    })
    .await
}

// MARK: - App

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Sessions::default())
        .manage(udf_print::PrintPages::default())
        .register_asynchronous_uri_scheme_protocol(udf_print::SCHEME, |ctx, request, responder| {
            let html = udf_print::serve(ctx.app_handle(), request.uri().path());
            let response = match html {
                Some(html) => tauri::http::Response::builder().header("Content-Type", "text/html; charset=utf-8").body(html.into_bytes()),
                None => tauri::http::Response::builder().status(404).body(Vec::new()),
            };
            responder.respond(response.unwrap_or_default());
        })
        .invoke_handler(tauri::generate_handler![
            file_info, read_pdf, thumbnail, open_path, reveal_path, ghostscript, initial_files,
            compress, scan_begin, scan_page, scan_finish, scan_dpi,
            images_to_pdf, udf_to_pdf, merge, split, organize,
            export_begin, export_page, export_finish, protect
        ])
        .setup(|app| {
            if let Some((input, output)) = cli_udf_request() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let code = match udf_print::convert(&handle, &input, &output).await {
                        Ok(pages) => {
                            println!("{pages} sayfa → {}", output.display());
                            0
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            1
                        }
                    };
                    handle.exit(code);
                });
            } else if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Belge Araçları");
                let _ = window.show();
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Belge Araçları başlatılamadı");
}
