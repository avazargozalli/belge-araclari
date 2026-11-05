//! Any supported document → UDF.

use std::path::Path;

use crate::office::{self, ExternalError, Target};

/// Files that can become UDF.
pub fn can_make_udf(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    ext == "pdf" || office::kind(path) == Some(office::Kind::Word)
}

/// Converts a PDF or Word document to UDF. Returns how it was read, for the result line.
/// .doc, .rtf and .odt need an installed Microsoft Word or LibreOffice to become .docx first.
pub fn to_udf(input: &Path, output: &Path, use_office: bool) -> Result<String, String> {
    let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = input.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "pdf" {
        crate::pdf::pdf_to_udf(input, output)?;
        return Ok("PDF metni ve biçimi".into());
    }
    if office::is_builtin(input) {
        crate::word::docx_to_udf(input, output)?;
        return Ok("Word belgesi".into());
    }
    if office::kind(input) != Some(office::Kind::Word) {
        return Err(format!("\"{name}\" UDF'ye çevrilemez. PDF veya Word belgesi seçin."));
    }
    if !use_office {
        return Err(format!("\"{name}\" için kurulu Office kullanımı Ayarlar'da kapalı. Yalnızca .docx dosyaları onsuz çevrilebilir."));
    }
    let temp = std::env::temp_dir().join(format!("belge-udf-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
    let docx = temp.join("belge.docx");
    let result = match office::convert_external_to(input, &docx, Target::Docx) {
        Ok(engine) => crate::word::docx_to_udf(&docx, output).map(|_| format!("{engine} ile .docx'e, sonra UDF'ye")),
        Err(ExternalError::Unavailable) => Err(format!("\"{name}\" için Microsoft Word veya LibreOffice gerekir. Yalnızca .docx dosyaları onsuz çevrilebilir.")),
        Err(ExternalError::Failed(e)) => Err(e),
    };
    let _ = std::fs::remove_dir_all(&temp);
    result
}
