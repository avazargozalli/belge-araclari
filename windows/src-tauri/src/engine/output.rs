//! Where results go. Originals are never overwritten: every name is made unique.

use std::path::{Path, PathBuf};

use super::{EngineError, Result};

/// Folder for results of `input`: the chosen folder if given and writable, else next to the input,
/// else the user's Downloads folder.
pub fn directory_for(input: &Path, chosen: Option<&Path>) -> PathBuf {
    if let Some(dir) = chosen {
        if is_writable_dir(dir) {
            return dir.to_path_buf();
        }
    }
    if let Some(parent) = input.parent() {
        if is_writable_dir(parent) {
            return parent.to_path_buf();
        }
    }
    downloads_dir()
}

fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    home.map(|h| h.join("Downloads")).unwrap_or_else(std::env::temp_dir)
}

fn is_writable_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".belge-araclari-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn base_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Belge".into())
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').to_string();
    if trimmed.is_empty() { "Belge".into() } else { trimmed }
}

pub fn unique_file(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let clean = sanitize(base);
    let mut candidate = dir.join(format!("{clean}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{clean} ({n}).{ext}"));
        n += 1;
    }
    candidate
}

pub fn unique_folder(dir: &Path, base: &str) -> Result<PathBuf> {
    let clean = sanitize(base);
    let mut candidate = dir.join(&clean);
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{clean} ({n})"));
        n += 1;
    }
    std::fs::create_dir_all(&candidate).map_err(|_| EngineError::CannotWrite(clean))?;
    Ok(candidate)
}

/// Writes atomically through a temporary sibling so a failed write never leaves a half file.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("belge-tmp");
    let name = super::file_name(path);
    std::fs::write(&tmp, bytes).map_err(|_| EngineError::CannotWrite(name.clone()))?;
    std::fs::rename(&tmp, path).map_err(|_| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::CannotWrite(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_never_collide() {
        let dir = std::env::temp_dir().join(format!("ba-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_file(&dir, "Dosya: ilk/deneme", "pdf");
        assert_eq!(a.file_name().unwrap(), "Dosya- ilk-deneme.pdf");
        std::fs::write(&a, b"x").unwrap();
        let b = unique_file(&dir, "Dosya: ilk/deneme", "pdf");
        assert_eq!(b.file_name().unwrap(), "Dosya- ilk-deneme (2).pdf");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
