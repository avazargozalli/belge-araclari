//! PDF engine for the Windows edition. Everything runs locally; nothing here touches the network.
//! UI-independent so it can be unit-tested on any platform.

pub mod compress;
pub mod images;
pub mod ops;
pub mod output;

#[cfg(test)]
mod tests;

use std::path::Path;

pub const CREATOR: &str = "Belge Araçları · Azar Gozalli";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("\"{0}\" açılamadı. Dosya bozuk veya desteklenmeyen bir biçimde olabilir.")]
    CannotOpen(String),
    #[error("\"{0}\" şifreli. Önce \"PDF Şifrele\" aracından şifresini kaldırın.")]
    Locked(String),
    #[error("\"{0}\" kaydedilemedi. Klasöre yazma izni olduğundan emin olun.")]
    CannotWrite(String),
    #[error("İşlenecek sayfa bulunamadı.")]
    NoPages,
    #[error("Sayfa aralığı anlaşılamadı: {0}. Örnek: 1-3, 5, 8-10")]
    InvalidRange(String),
    #[error("\"{0}\" için girilen şifre yanlış.")]
    WrongPassword(String),
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Progress callback: fraction in 0..=1 and a short status line.
pub type Progress<'a> = &'a (dyn Fn(f64, &str) + Sync);
