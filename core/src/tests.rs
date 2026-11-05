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

// MARK: - Word reader

fn docx(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, body) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn sample_docx() -> Vec<u8> {
    let document = format!(
        r#"<w:document {W}><w:body>
<w:p><w:pPr><w:pStyle w:val="Baslik"/></w:pPr><w:r><w:t>DİLEKÇE</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Birinci</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Alt</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>İkinci</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r></w:p>
<w:tbl><w:tblPr><w:tblStyle w:val="TabloKilavuzu"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="3000"/></w:tblGrid>
<w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Birleşik</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
<w:p><w:pPr><w:tabs><w:tab w:val="right" w:leader="dot" w:pos="8000"/></w:tabs></w:pPr><w:r><w:rPr><w:b/><w:color w:val="FF0000"/></w:rPr><w:t>İçindekiler</w:t></w:r><w:r><w:tab/><w:t>7</w:t></w:r></w:p>
<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1417" w:right="1417" w:bottom="1417" w:left="1417" w:header="708" w:footer="708"/></w:sectPr>
</w:body></w:document>"#
    );
    let styles = format!(
        r#"<w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman"/><w:sz w:val="24"/><w:lang w:val="tr-TR"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Baslik"><w:basedOn w:val="Normal"/><w:pPr><w:jc w:val="center"/></w:pPr><w:rPr><w:b/><w:sz w:val="28"/></w:rPr></w:style>
<w:style w:type="table" w:styleId="TabloKilavuzu"><w:tblPr><w:tblBorders><w:top w:val="single"/><w:left w:val="single"/><w:bottom w:val="single"/><w:right w:val="single"/><w:insideH w:val="single"/><w:insideV w:val="single"/></w:tblBorders></w:tblPr></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering {W}><w:abstractNum w:abstractNumId="0">
<w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl>
<w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2)"/><w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr></w:lvl>
</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let header = format!(
        r#"<w:hdr {W}><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t xml:space="preserve">Sayfa </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> / </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>2</w:t></w:r></w:fldSimple></w:p></w:hdr>"#
    );
    let footnotes = format!(r#"<w:footnotes {W}><w:footnote w:id="2"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Dipnot metni.</w:t></w:r></w:p></w:footnote></w:footnotes>"#);
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdH" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/></Relationships>"#;
    docx(&[
        ("word/document.xml", &document),
        ("word/styles.xml", &styles),
        ("word/numbering.xml", &numbering),
        ("word/header1.xml", &header),
        ("word/footnotes.xml", &footnotes),
        ("word/_rels/document.xml.rels", rels),
    ])
}

#[test]
fn word_reader_keeps_text_lists_tables_and_page_numbers() {
    let xml = crate::word::docx_to_content_xml(&sample_docx()).unwrap();
    let doc = udf::parse_xml(&xml).unwrap();
    let html = udf::body_html(&doc);
    for text in ["DİLEKÇE", "Birinci", "Alt", "İkinci", "Birleşik", "İçindekiler", "Dipnot metni."] {
        assert!(xml.contains(text), "{text} eksik");
    }
    // Native UYAP lists: decimal "%1." and letters "%2)" (agree with Word up to c).
    assert!(xml.contains("Numbered=\"true\" NumberType=\"NUMBER_TYPE_NUMBER_DOT\" ListId=\"1\" ListLevel=\"1\""), "{xml}");
    assert!(xml.contains("NumberType=\"NUMBER_TYPE_CHAR_SMALL_PARANTHESE\" ListId=\"1\" ListLevel=\"2\" SecListTypeLevel2=\"NUMBER_TYPE_CHAR_SMALL_PARANTHESE\""));
    assert!(xml.contains("<page-break>"), "sayfa sonu öğesi");
    assert!(crate::udfout::validate(&xml).is_empty(), "{:?}", crate::udfout::validate(&xml));
    assert!(html.contains("<table"), "tablo");
    assert!(html.contains("colspan=\"2\""), "birleşik hücre");
    assert!(html.contains("break-before:page"), "sayfa sonu çizilir");
    assert!(xml.contains("pageNumber-spec=\"BSP32_2120\""), "sağa hizalı sayfa/toplam: {}", &xml[xml.find("<header").unwrap()..][..300]);
    assert!(xml.contains("pageNumber-foreStr=\"Sayfa \"") || xml.contains("pageNumber-foreStr=\"Sayfa\""));
    assert!(xml.contains("border=\"borderCell\""));
    assert!(xml.contains("Alignment=\"1\""), "ortalanmış başlık");
    assert!(xml.contains(":1:1"), "noktalı sağ sekme");
    assert!(xml.contains("foreground=\"-65536\""), "kırmızı yazı");
    assert!(xml.contains("superscript=\"true\""), "dipnot işareti");
}

#[test]
fn word_reader_round_trips_our_own_docx() {
    let original = udf::parse_file(&fixture()).unwrap();
    let bytes = udf::docx::to_docx(&[original]).unwrap();
    let xml = crate::word::docx_to_content_xml(&bytes).unwrap();
    let back = udf::parse_xml(&xml).unwrap();
    let html = udf::body_html(&back);
    assert!(html.contains("MAHKEMES"));
    assert!(html.contains("Tablo hücresi metni"));
    assert!(xml.contains("bgImageData"), "antet arka plan olarak korunmalı");
    assert!(xml.contains("<footer") && xml.contains("pageNumber-spec"), "sayfa numarası");
}

#[test]
fn word_reader_rejects_non_docx() {
    assert!(crate::word::docx_to_content_xml(b"not a zip").is_err());
    let empty = docx(&[("hello.txt", "x")]);
    assert!(crate::word::docx_to_content_xml(&empty).is_err());
}

#[test]
fn office_file_kinds() {
    use crate::office::{is_builtin, is_office, kind, Kind};
    assert!(is_office(Path::new("a.DOCX")) && is_office(Path::new("b.xls")) && is_office(Path::new("c.pptx")));
    assert!(!is_office(Path::new("d.pdf")) && !is_office(Path::new("e.udf")));
    assert!(is_builtin(Path::new("a.docx")) && !is_builtin(Path::new("a.doc")));
    assert_eq!(kind(Path::new("x.ods")), Some(Kind::Excel));
    assert_eq!(kind(Path::new("x.odp")), Some(Kind::PowerPoint));
}

// MARK: - PDF reader

/// A one-page PDF: a bold red title, a body line, and a ruled 2×2 table.
fn sample_pdf() -> Vec<u8> {
    use lopdf::{dictionary, Object, Stream};
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let regular = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman", "Encoding" => "WinAnsiEncoding" });
    let bold = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Bold", "Encoding" => "WinAnsiEncoding" });
    let content = b"BT /F2 14 Tf 1 0 0 rg 200 780 Td (DILEKCE) Tj ET \
BT /F1 12 Tf 0 0 0 rg 72 750 Td (Birinci satir metni.) Tj ET \
0 0 0 RG 0.5 w 72 700 m 372 700 l S 72 680 m 372 680 l S 72 660 m 372 660 l S \
72 700 m 72 660 l S 222 700 m 222 660 l S 372 700 m 372 660 l S \
BT /F1 12 Tf 76 686 Td (Donem) Tj ET BT /F1 12 Tf 226 686 Td (Tutar) Tj ET \
BT /F1 12 Tf 76 666 Td (Agustos) Tj ET BT /F1 12 Tf 226 666 Td (15.000) Tj ET"
        .to_vec();
    let content_id = doc.add_object(Stream::new(dictionary! {}, content));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Contents" => content_id, "Resources" => dictionary! { "Font" => dictionary! { "F1" => regular, "F2" => bold } },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = vec![];
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn pdf_reader_keeps_text_styles_and_tables() {
    let xml = crate::pdf::pdf_to_content_xml(&sample_pdf()).unwrap();
    assert!(crate::udfout::validate(&xml).is_empty(), "{:?}", crate::udfout::validate(&xml));
    for text in ["DILEKCE", "Birinci satir metni.", "Donem", "Tutar", "Agustos", "15.000"] {
        assert!(xml.contains(text), "{text} eksik: {xml}");
    }
    // Bold red title, Times New Roman, 14 pt.
    assert!(xml.contains("family=\"Times New Roman\" size=\"14\" bold=\"true\""), "{xml}");
    assert!(xml.contains("foreground=\"-65536\""), "kırmızı");
    // The ruled grid becomes a 2-column table with every line drawn.
    let doc = udf::parse_xml(&xml).unwrap();
    let html = udf::body_html(&doc);
    assert_eq!(html.matches("<td").count(), 4, "{html}");
    assert!(xml.contains("border=\"borderCell\""));
}

#[test]
fn pdf_font_names_become_families() {
    use crate::pdf::read::font_style;
    let s = font_style("ABCDEF+TimesNewRomanPS-BoldItalicMT", 0, 0.0);
    assert_eq!((s.family.as_str(), s.bold, s.italic), ("Times New Roman", true, true));
    assert_eq!(font_style("ArialMT", 0, 0.0).family, "Arial");
    assert_eq!(font_style("Calibri-Bold", 0, 0.0).family, "Calibri");
    assert_eq!(font_style("Times-Roman", 0, 0.0).family, "Times New Roman");
    assert_eq!(font_style("BookmanOldStyle", 0, 0.0).family, "Bookman Old Style");
}

#[test]
fn pages_as_pictures_make_one_page_each() {
    let png = crate::pdf::read::png(2, 2, 3, &[255; 12]);
    let xml = crate::pdf::pages_to_content_xml(&[(png.clone(), 595.0, 842.0), (png, 595.0, 842.0)]);
    assert!(crate::udfout::validate(&xml).is_empty());
    assert_eq!(xml.matches("<image ").count(), 2);
    assert_eq!(xml.matches("<page-break>").count(), 1);
}
