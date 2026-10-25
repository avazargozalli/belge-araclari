//! UYAP Doküman Formatı (.udf) for the Windows app. Parsing, HTML and Word output live in the
//! shared `belge-core` crate; this module adds the PDF step: the letterhead, header, footer and page
//! numbers are printed as a "decoration" page per body page and placed underneath with [`underlay`].

use std::collections::HashMap;
use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

pub use belge_core::udf::{body_html, decoration_html, docx, ready_margins, UdfDocument};

use super::{EngineError, Result};

pub fn parse_file(path: &Path) -> Result<UdfDocument> {
    belge_core::udf::parse_file(path).map_err(EngineError::Message)
}

// MARK: - Underlay

fn page_content(doc: &Document, page: ObjectId) -> Vec<u8> {
    doc.get_page_content(page).unwrap_or_default()
}

fn page_resources(doc: &Document, page: ObjectId) -> Object {
    let mut current = doc.get_dictionary(page).ok();
    for _ in 0..32 {
        let Some(dict) = current else { break };
        if let Ok(res) = dict.get(b"Resources") {
            return match res {
                Object::Reference(id) => doc.get_object(*id).cloned().unwrap_or(Object::Dictionary(dictionary! {})),
                other => other.clone(),
            };
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok().and_then(|p| doc.get_dictionary(p).ok());
    }
    Object::Dictionary(dictionary! {})
}

/// Draws page *n* of `decoration` underneath page *n* of `body` (letterhead behind the text).
pub fn underlay(body: &mut Document, mut decoration: Document) -> Result<()> {
    decoration.renumber_objects_with(body.max_id + 1);
    let deco_pages = decoration.get_pages();
    let mut forms: HashMap<u32, (ObjectId, Vec<u8>, Object, Vec<f32>)> = HashMap::new();
    for (&n, &id) in &deco_pages {
        let media = decoration
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"MediaBox").ok())
            .and_then(|m| m.as_array().ok())
            .map(|a| a.iter().filter_map(|v| v.as_float().ok()).collect())
            .unwrap_or_else(|| vec![0.0, 0.0, 595.28, 841.89]);
        forms.insert(n, (id, page_content(&decoration, id), page_resources(&decoration, id), media));
    }
    body.max_id = decoration.max_id;
    body.objects.extend(std::mem::take(&mut decoration.objects));

    for (n, page_id) in body.get_pages() {
        let Some((_, content, resources, media)) = forms.remove(&n) else { continue };
        let mut form = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => media.into_iter().map(Object::Real).collect::<Vec<_>>(),
                "Resources" => resources,
            },
            content,
        );
        let _ = form.compress();
        let form_id = body.add_object(form);
        let prefix_id = body.add_object(Stream::new(dictionary! {}, b"q /BelgeAntet Do Q\n".to_vec()));

        let mut resources = page_resources(body, page_id);
        if let Object::Dictionary(dict) = &mut resources {
            let mut xobjects = match dict.get(b"XObject") {
                Ok(Object::Reference(id)) => body.get_dictionary(*id).cloned().unwrap_or_default(),
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => dictionary! {},
            };
            xobjects.set("BelgeAntet", form_id);
            dict.set("XObject", xobjects);
        }
        let page = body.get_dictionary_mut(page_id).map_err(|_| EngineError::NoPages)?;
        let mut contents = vec![Object::Reference(prefix_id)];
        match page.get(b"Contents") {
            Ok(Object::Array(items)) => contents.extend(items.iter().cloned()),
            Ok(other) => contents.push(other.clone()),
            Err(_) => {}
        }
        page.set("Contents", contents);
        page.set("Resources", resources);
    }
    body.prune_objects();
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub use belge_core::udf::label;
}
