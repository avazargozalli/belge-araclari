//! Page-level operations: open, merge, split, reorder/rotate/delete, encrypt, decrypt.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{dictionary, Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, SaveOptions, StringFormat};

use super::output::{base_name, unique_file, unique_folder, write_bytes};
use super::{file_name, EngineError, Progress, Result, CREATOR};

// MARK: - Open / save

/// Loads a PDF. Files that only carry an owner password open transparently;
/// files that need a password to open are rejected with a helpful message.
pub fn open(path: &Path) -> Result<Document> {
    let name = file_name(path);
    let mut doc = Document::load(path).map_err(|_| EngineError::CannotOpen(name.clone()))?;
    if doc.is_encrypted() {
        if doc.encryption_state.is_none() {
            return Err(EngineError::Locked(name));
        }
        strip_encryption(&mut doc);
    }
    if doc.get_pages().is_empty() {
        return Err(EngineError::NoPages);
    }
    Ok(doc)
}

fn strip_encryption(doc: &mut Document) {
    if let Ok(id) = doc.trailer.get(b"Encrypt").and_then(Object::as_reference) {
        doc.objects.remove(&id);
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
}

pub fn page_count(path: &Path) -> Result<(u32, bool)> {
    let name = file_name(path);
    let doc = Document::load(path).map_err(|_| EngineError::CannotOpen(name))?;
    let locked = doc.is_encrypted() && doc.encryption_state.is_none();
    Ok((doc.get_pages().len() as u32, locked))
}

/// PDF text strings outside ASCII must be UTF-16BE with a byte-order mark.
pub fn text_string(text: &str) -> Object {
    if text.is_ascii() {
        return Object::String(text.as_bytes().to_vec(), StringFormat::Literal);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, StringFormat::Hexadecimal)
}

pub fn stamp_creator(doc: &mut Document) {
    let info_id = doc.trailer.get(b"Info").and_then(Object::as_reference).ok();
    match info_id.and_then(|id| doc.get_dictionary_mut(id).ok()) {
        Some(info) => {
            info.set("Creator", text_string(CREATOR));
            info.set("Producer", text_string(CREATOR));
        }
        None => {
            let id = doc.add_object(dictionary! {
                "Creator" => text_string(CREATOR),
                "Producer" => text_string(CREATOR),
            });
            doc.trailer.set("Info", id);
        }
    }
}

/// Serialises with compressed streams; object streams are used unless the document is encrypted.
pub fn to_bytes(doc: &mut Document, object_streams: bool) -> Result<Vec<u8>> {
    doc.compress();
    let mut buffer = Vec::new();
    if object_streams {
        let options = SaveOptions::builder().use_object_streams(true).use_xref_streams(true).build();
        if doc.save_with_options(&mut buffer, options).is_ok() {
            return Ok(buffer);
        }
        buffer.clear();
    }
    doc.save_to(&mut buffer).map_err(|e| EngineError::Message(format!("PDF yazılamadı: {e}")))?;
    Ok(buffer)
}

pub fn save(doc: &mut Document, path: &Path) -> Result<()> {
    stamp_creator(doc);
    let bytes = to_bytes(doc, true)?;
    write_bytes(path, &bytes)
}

// MARK: - Assemble

/// One page of an assembled document: source index, 1-based page number, extra clockwise rotation.
#[derive(Clone, Copy, Debug)]
pub struct Pick {
    pub doc: usize,
    pub page: u32,
    pub rotate: i64,
}

fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut current = doc.get_dictionary(page).ok()?;
    for _ in 0..64 {
        if let Ok(value) = current.get(key) {
            return Some(value);
        }
        let parent = current.get(b"Parent").and_then(Object::as_reference).ok()?;
        current = doc.get_dictionary(parent).ok()?;
    }
    None
}

/// Builds a new document from pages of one or more sources. Used by merge, split and the organizer.
/// Inherited page attributes are copied onto each page so they survive re-parenting.
pub fn assemble(mut sources: Vec<Document>, picks: &[Pick]) -> Result<Document> {
    if picks.is_empty() {
        return Err(EngineError::NoPages);
    }
    let mut next_id = 1;
    let mut page_maps = Vec::with_capacity(sources.len());
    for source in sources.iter_mut() {
        source.renumber_objects_with(next_id);
        next_id = source.max_id + 1;
        page_maps.push(source.get_pages());
    }
    let pages_id: ObjectId = (next_id, 0);
    next_id += 1;

    let mut kids = Vec::with_capacity(picks.len());
    let mut new_pages: Vec<(ObjectId, Dictionary)> = Vec::with_capacity(picks.len());
    let mut used = HashSet::new();
    for pick in picks {
        let source = sources.get(pick.doc).ok_or(EngineError::NoPages)?;
        let &page_id = page_maps[pick.doc].get(&pick.page).ok_or(EngineError::NoPages)?;
        let mut dict = source.get_dictionary(page_id).map_err(|_| EngineError::NoPages)?.clone();
        for key in [&b"Resources"[..], b"MediaBox", b"CropBox", b"Rotate"] {
            if !dict.has(key) {
                if let Some(value) = inherited(source, page_id, key) {
                    dict.set(key.to_vec(), value.clone());
                }
            }
        }
        let rotation = dict.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0) + pick.rotate;
        dict.set("Rotate", rotation.rem_euclid(360));
        dict.set("Parent", pages_id);
        // A page picked twice shares its resources but needs its own page object.
        let id = if used.insert((pick.doc, page_id)) {
            page_id
        } else {
            next_id += 1;
            (next_id - 1, 0)
        };
        kids.push(Object::Reference(id));
        new_pages.push((id, dict));
    }

    let acroform = if sources.len() == 1 {
        sources[0].catalog().ok().and_then(|c| c.get(b"AcroForm").ok().cloned())
    } else {
        None
    };

    let mut result = Document::with_version("1.7");
    for source in sources {
        result.objects.extend(source.objects);
    }
    for (id, dict) in new_pages {
        result.objects.insert(id, Object::Dictionary(dict));
    }
    let count = kids.len() as i64;
    result.objects.insert(pages_id, Object::Dictionary(dictionary! {
        "Type" => "Pages",
        "Kids" => kids,
        "Count" => count,
    }));
    let catalog_id: ObjectId = (next_id, 0);
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if let Some(form) = acroform {
        catalog.set("AcroForm", form);
    }
    result.objects.insert(catalog_id, Object::Dictionary(catalog));
    result.trailer.set("Root", catalog_id);
    result.max_id = next_id;
    result.prune_objects();
    Ok(result)
}

// MARK: - Merge

pub fn merge(inputs: &[PathBuf], output: &Path, progress: Progress) -> Result<u32> {
    let mut sources = Vec::with_capacity(inputs.len());
    let mut picks = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        progress(i as f64 / inputs.len() as f64 * 0.7, &format!("Ekleniyor: {}", file_name(path)));
        let doc = open(path)?;
        for page in doc.get_pages().keys() {
            picks.push(Pick { doc: i, page: *page, rotate: 0 });
        }
        sources.push(doc);
    }
    progress(0.8, "Kaydediliyor…");
    let mut merged = assemble(sources, &picks)?;
    save(&mut merged, output)?;
    Ok(picks.len() as u32)
}

// MARK: - Split

pub enum SplitMode {
    EachPage,
    Ranges { text: String, separate: bool },
    Every(u32),
}

/// Parses "1-3, 5, 8-" into 1-based page groups.
pub fn parse_ranges(text: &str, page_count: u32) -> Result<Vec<Vec<u32>>> {
    let normalized = text.replace('–', "-").replace(';', ",");
    let mut groups = Vec::new();
    for raw in normalized.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let invalid = || EngineError::InvalidRange(token.to_string());
        if let Some((a, b)) = token.split_once('-') {
            let (a, b) = (a.trim(), b.trim());
            let start = if a.is_empty() { 1 } else { a.parse::<u32>().map_err(|_| invalid())? };
            let end = if b.is_empty() { page_count } else { b.parse::<u32>().map_err(|_| invalid())? };
            if start < 1 || end > page_count || start > end {
                return Err(invalid());
            }
            groups.push((start..=end).collect());
        } else {
            let n = token.parse::<u32>().map_err(|_| invalid())?;
            if n < 1 || n > page_count {
                return Err(invalid());
            }
            groups.push(vec![n]);
        }
    }
    if groups.is_empty() {
        return Err(EngineError::InvalidRange(text.to_string()));
    }
    Ok(groups)
}

fn label(pages: &[u32]) -> String {
    match (pages.first(), pages.last()) {
        (Some(f), _) if pages.len() == 1 => format!("sayfa {f}"),
        (Some(f), Some(l)) if pages.windows(2).all(|w| w[1] == w[0] + 1) => format!("sayfa {f}-{l}"),
        _ => format!("{} sayfa", pages.len()),
    }
}

/// Returns the written file (one result) or folder (several), the number of files, and whether it is a folder.
pub fn split(input: &Path, mode: SplitMode, dir: &Path, progress: Progress) -> Result<(PathBuf, usize, bool)> {
    let source = open(input)?;
    let count = source.get_pages().len() as u32;
    let groups: Vec<Vec<u32>> = match mode {
        SplitMode::EachPage => (1..=count).map(|p| vec![p]).collect(),
        SplitMode::Every(n) => {
            let n = n.max(1);
            (1..=count).collect::<Vec<_>>().chunks(n as usize).map(|c| c.to_vec()).collect()
        }
        SplitMode::Ranges { text, separate } => {
            let parsed = parse_ranges(&text, count)?;
            if separate { parsed } else { vec![parsed.concat()] }
        }
    };
    let base = base_name(input);
    let build = |pages: &[u32]| -> Result<Document> {
        let picks: Vec<Pick> = pages.iter().map(|&page| Pick { doc: 0, page, rotate: 0 }).collect();
        assemble(vec![source.clone()], &picks)
    };

    if groups.len() == 1 {
        let out = unique_file(dir, &format!("{base} ({})", label(&groups[0])), "pdf");
        progress(0.5, "Kaydediliyor…");
        save(&mut build(&groups[0])?, &out)?;
        return Ok((out, 1, false));
    }

    let folder = unique_folder(dir, &format!("{base} - bölünmüş"))?;
    let digits = count.to_string().len();
    for (i, group) in groups.iter().enumerate() {
        progress(i as f64 / groups.len() as f64, &format!("Dosya {} / {}", i + 1, groups.len()));
        let name = if group.len() == 1 {
            format!("{base} - sayfa {:0digits$}", group[0])
        } else {
            format!("{base} - {}", label(group))
        };
        save(&mut build(group)?, &unique_file(&folder, &name, "pdf"))?;
    }
    Ok((folder, groups.len(), true))
}

// MARK: - Organize

/// `pages` are (1-based source page, extra clockwise rotation) in the new order.
pub fn organize(input: &Path, pages: &[(u32, i64)], output: &Path) -> Result<u32> {
    let source = open(input)?;
    let picks: Vec<Pick> = pages.iter().map(|&(page, rotate)| Pick { doc: 0, page, rotate }).collect();
    let mut doc = assemble(vec![source], &picks)?;
    save(&mut doc, output)?;
    Ok(picks.len() as u32)
}

// MARK: - Passwords

pub fn encrypt(input: &Path, user_password: &str, owner_password: &str, output: &Path) -> Result<()> {
    if user_password.is_empty() {
        return Err(EngineError::Message("Şifre boş olamaz.".into()));
    }
    let mut doc = open(input)?;
    stamp_creator(&mut doc);
    let owner = if owner_password.is_empty() { user_password } else { owner_password };
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|_| EngineError::Message("Şifreleme anahtarı üretilemedi.".into()))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let state = EncryptionState::try_from(EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner,
        user_password,
        permissions: Permissions::all(),
    })
    .map_err(|e| EngineError::Message(format!("Şifreleme hazırlanamadı: {e}")))?;
    doc.compress();
    doc.encrypt(&state).map_err(|e| EngineError::Message(format!("Şifrelenemedi: {e}")))?;
    let mut buffer = Vec::new();
    doc.save_to(&mut buffer).map_err(|_| EngineError::CannotWrite(file_name(output)))?;
    write_bytes(output, &buffer)
}

pub fn decrypt(input: &Path, password: &str, output: &Path) -> Result<()> {
    let name = file_name(input);
    let probe = Document::load(input).map_err(|_| EngineError::CannotOpen(name.clone()))?;
    if !probe.is_encrypted() && probe.encryption_state.is_none() {
        return Err(EngineError::Message(format!("\"{name}\" zaten şifresiz.")));
    }
    drop(probe);
    let mut doc = Document::load_with_password(input, password).map_err(|_| EngineError::WrongPassword(name.clone()))?;
    if doc.is_encrypted() && doc.encryption_state.is_none() {
        return Err(EngineError::WrongPassword(name));
    }
    strip_encryption(&mut doc);
    save(&mut doc, output)
}
