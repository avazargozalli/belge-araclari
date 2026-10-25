use std::io::Read;
use std::path::{Path, PathBuf};

use crate::udf;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../windows/tests/fixtures/ornek-dilekce.udf")
}

fn part(docx: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
    let mut out = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut out).unwrap();
    out
}

fn well_formed(xml: &str) {
    roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("bozuk XML: {e}"));
}

#[test]
fn docx_from_fixture_has_all_parts() {
    let doc = udf::parse_file(&fixture()).unwrap();
    let bytes = udf::docx::to_docx(&[doc]).unwrap();
    let zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let names: Vec<&str> = zip.file_names().collect();
    for name in ["[Content_Types].xml", "_rels/.rels", "word/document.xml", "word/styles.xml", "word/settings.xml", "word/header1.xml", "word/footer1.xml", "word/_rels/document.xml.rels"] {
        assert!(names.contains(&name), "{name} eksik: {names:?}");
    }
    for name in names.iter().filter(|n| n.ends_with(".xml") || n.ends_with(".rels")) {
        well_formed(&part(&bytes, name));
    }
    let document = part(&bytes, "word/document.xml");
    assert!(document.contains("<w:pgSz w:w=\"11906\" w:h=\"16838\""), "A4 sayfa");
    assert!(document.contains("<w:tbl>"), "tablo");
    assert!(document.contains("w:gridSpan"), "birleşik hücre");
    assert!(document.contains("MAHKEMESİNE"), "metin");
}

#[test]
fn docx_letterhead_sits_behind_text_in_header() {
    let doc = udf::parse_file(&fixture()).unwrap();
    let bytes = udf::docx::to_docx(&[doc]).unwrap();
    let header = part(&bytes, "word/header1.xml");
    assert!(header.contains("behindDoc=\"1\""));
    assert!(header.contains("relativeFrom=\"page\""));
    let rels = part(&bytes, "word/_rels/header1.xml.rels");
    assert!(rels.contains("media/image"));
}

#[test]
fn docx_page_numbers_are_fields() {
    let doc = udf::parse_file(&fixture()).unwrap();
    let bytes = udf::docx::to_docx(&[doc]).unwrap();
    let footer = part(&bytes, "word/footer1.xml");
    assert!(footer.contains(" PAGE "), "{footer}");
}

#[test]
fn docx_merges_documents_as_sections() {
    let a = udf::parse_file(&fixture()).unwrap();
    let b = udf::parse_file(&fixture()).unwrap();
    let bytes = udf::docx::to_docx(&[a, b]).unwrap();
    let document = part(&bytes, "word/document.xml");
    assert_eq!(document.matches("<w:sectPr>").count(), 2);
    assert!(document.contains("rIdH2") && document.contains("rIdF2"));
    well_formed(&part(&bytes, "word/header2.xml"));
}

#[test]
fn docx_writes_list_labels_tabs_and_styles() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?><template format_id="1.8">
<content><![CDATA[Bir	iki
Madde
Madde
Madde
Madde
]]></content>
<properties><pageFormat mediaSizeName="1" leftMargin="70" rightMargin="70" topMargin="70" bottomMargin="70" paperOrientation="1" headerFOffset="20" footerFOffset="20"/></properties>
<elements resolver="hvl-default">
<paragraph TabSet="200:1:1"><content startOffset="0" length="8" bold="true" underline="true"/></paragraph>
<paragraph Numbered="true" NumberType="NUMBER_TYPE_CHAR_SMALL_PARANTEZ" ListId="1"><content startOffset="8" length="6"/></paragraph>
<paragraph Numbered="true" NumberType="NUMBER_TYPE_CHAR_SMALL_PARANTEZ" ListId="1"><content startOffset="14" length="6"/></paragraph>
<paragraph Numbered="true" NumberType="NUMBER_TYPE_CHAR_SMALL_PARANTEZ" ListId="1"><content startOffset="20" length="6"/></paragraph>
<paragraph Numbered="true" NumberType="NUMBER_TYPE_CHAR_SMALL_PARANTEZ" ListId="1"><content startOffset="26" length="6"/></paragraph>
</elements><styles><style name="default" family="Dialog" size="12"/><style name="hvl-default" family="Times New Roman" size="12"/></styles></template>"#;
    let doc = udf::parse_xml(xml).unwrap();
    let bytes = udf::docx::to_docx(&[doc]).unwrap();
    let document = part(&bytes, "word/document.xml");
    well_formed(&document);
    assert!(document.contains("<w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"4000\"/>"), "{document}");
    assert!(document.contains("<w:b/>") && document.contains("<w:u w:val=\"single\"/>"));
    // Turkish alphabet: the fourth item is ç), not d).
    assert!(document.contains(">ç)<"), "{document}");
    assert!(document.contains("<w:pgMar w:top=\"1400\""));
}

#[test]
fn list_labels_follow_editor_rules() {
    assert_eq!(udf::label("NUMBER_TYPE_NUMBER_DOT", 3), "3.");
    assert_eq!(udf::label("NUMBER_TYPE_CHAR_SMALL_D_PARANTHESE", 4), "(ç)");
    assert_eq!(udf::label("NUMBER_TYPE_ROMAN_BIG_DOT", 4), "IV.");
}
