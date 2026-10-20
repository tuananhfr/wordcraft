use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use image::ImageEncoder;
use regex::Regex;
use serde_json::{Value, json};
use wordcraft_doc::numbering::{AbstractNum, Level, Num};
use wordcraft_doc::props::{CharProps, NumRef, TextColor, Underline};
use wordcraft_doc::section::NumFormat;
use wordcraft_doc::styles::{Style, StyleKind};
use wordcraft_doc::{Document, InlineObject, Path, Pos, Rgb, StoryRef};
use wordcraft_engine::{Selection, Session, io::save_bytes};

type Package = BTreeMap<String, Vec<u8>>;

fn run(session: &mut Session, command: &str, params: Value) -> Value {
    session.run(command, &params).unwrap_or_else(|e| panic!("{command}: {e}"))
}

fn save(session: &Session) -> Package {
    let bytes = save_bytes("paste.docx", &session.doc).expect("save DOCX in memory");
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("DOCX archive");
    let mut parts = BTreeMap::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).expect("DOCX entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("read DOCX entry");
        assert!(parts.insert(entry.name().to_string(), bytes).is_none(), "duplicate DOCX part");
    }
    parts
}

fn xml<'a>(parts: &'a Package, name: &str) -> &'a str {
    std::str::from_utf8(parts.get(name).unwrap_or_else(|| panic!("missing {name}"))).expect("UTF-8 XML")
}

fn tags<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let pattern = format!(r"<{}(?:\s[^>]*)?/?>", regex::escape(name));
    Regex::new(&pattern).expect("tag pattern").find_iter(xml).map(|m| m.as_str()).collect()
}

// Fixtures contain no nested elements of the same name in the scopes inspected here.
fn elements<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let name = regex::escape(name);
    let pattern = format!(r"(?s)<{name}(?:\s[^>]*)?>.*?</{name}>");
    Regex::new(&pattern).expect("element pattern").find_iter(xml).map(|m| m.as_str()).collect()
}

fn attr<'a>(element: &'a str, name: &str) -> &'a str {
    let opening = element.split('>').next().expect("opening tag");
    let prefix = format!("{name}=\"");
    opening.split_once(&prefix).unwrap_or_else(|| panic!("missing {name}: {opening}")).1.split('"').next().expect("attribute value")
}

fn fragments(label: &str, xml: &str, expected: &[&str]) {
    for fragment in expected {
        assert!(xml.contains(fragment), "{label}: missing {fragment}\n{xml}");
    }
}

fn containing_text<'a>(xml: &'a str, name: &str, text: &str) -> &'a str {
    let needle = format!(">{text}<");
    elements(xml, name).into_iter().find(|e| e.contains(&needle)).unwrap_or_else(|| panic!("missing {name} containing {text}: {xml}"))
}

fn text_fragments(xml: &str, name: &str, text: &str, expected: &[&str]) {
    fragments(text, containing_text(xml, name, text), expected);
}

fn assert_styles_resolve(parts: &Package) {
    let styles = xml(parts, "word/styles.xml");
    let definitions: Vec<_> = tags(styles, "w:style").into_iter().map(|st| attr(st, "w:styleId")).collect();
    assert!(definitions.contains(&"Normal"));
    let document = xml(parts, "word/document.xml");
    for name in ["w:pStyle", "w:rStyle", "w:tblStyle"] {
        for reference in tags(document, name) {
            let id = attr(reference, "w:val");
            assert!(definitions.contains(&id), "dangling {name}: {id}");
        }
    }
}

fn assert_numbering_resolves(parts: &Package) {
    let numbering = xml(parts, "word/numbering.xml");
    let instances = elements(numbering, "w:num");
    let abstracts = elements(numbering, "w:abstractNum");
    assert!(!instances.is_empty(), "missing numbering instances");
    for instance in &instances {
        let reference = tags(instance, "w:abstractNumId");
        assert_eq!(reference.len(), 1, "{instance}");
        let id = attr(reference[0], "w:val");
        assert!(abstracts.iter().any(|a| attr(a, "w:abstractNumId") == id), "dangling abstractNumId {id}");
    }
    for name in ["word/document.xml", "word/styles.xml"] {
        for reference in tags(xml(parts, name), "w:numId") {
            let id = attr(reference, "w:val");
            if id != "0" {
                assert!(instances.iter().any(|n| attr(n, "w:numId") == id), "{name}: dangling numId {id}");
            }
        }
    }
}

fn paste_save(html: &str) -> Package {
    let mut session = Session::new(Document::new());
    run(&mut session, "edit.paste", json!({"html": html}));
    let parts = save(&session);
    assert_styles_resolve(&parts);
    assert_numbering_resolves(&parts);
    parts
}

#[test]
fn word_html_paste_saves_golden_run_paragraph_and_list_properties() {
    let parts = paste_save(
        r#"
        <style>
          p.MsoBody { font-family:'Golden Paste Serif'; font-size:14pt; color:#123456;
            font-weight:bold; font-style:italic; text-decoration:underline line-through;
            margin-left:1in; margin-right:24px; text-indent:-.5in;
            margin-top:6pt; margin-bottom:12pt; line-height:150%; text-align:justify; }
          p.MsoListParagraph { mso-list:l0 level1 lfo1; }
        </style>
        <p class='MsoBody'>Word styled text</p>
        <p class='MsoListParagraph'><span style='mso-list:Ignore'>1.&nbsp;</span>Word first item</p>
        <p style='mso-list:l0 level2 lfo1'><!--[if !supportLists]><span>1.&nbsp;</span><![endif]-->Word nested item</p>
    "#,
    );
    let document = xml(&parts, "word/document.xml");
    text_fragments(
        document,
        "w:r",
        "Word styled text",
        &[
            "<w:b/>",
            "<w:i/>",
            "<w:u w:val=\"single\"/>",
            "<w:strike/>",
            "<w:color w:val=\"123456\"/>",
            "<w:sz w:val=\"28\"/>",
            "<w:rFonts w:ascii=\"Golden Paste Serif\" w:hAnsi=\"Golden Paste Serif\"/>",
        ],
    );
    text_fragments(
        document,
        "w:p",
        "Word styled text",
        &[
            "<w:ind w:left=\"1440\" w:right=\"360\" w:hanging=\"720\"/>",
            "<w:spacing w:before=\"120\" w:after=\"240\" w:line=\"360\" w:lineRule=\"auto\"/>",
            "<w:jc w:val=\"both\"/>",
        ],
    );
    for (text, level) in [("Word first item", "0"), ("Word nested item", "1")] {
        text_fragments(document, "w:p", text, &["<w:numPr>", &format!("<w:ilvl w:val=\"{level}\"/>")]);
    }
    assert!(!document.contains(">1."), "list markers must not become document text: {document}");
    fragments("Word numbering", xml(&parts, "word/numbering.xml"), &["<w:numFmt w:val=\"decimal\"/>", "<w:lvlText w:val=\"%1.\"/>"]);
    fragments("Word styles", xml(&parts, "word/styles.xml"), &["w:styleId=\"ListParagraph\""]);
}

#[test]
fn google_docs_html_paste_saves_golden_inline_formatting_and_nested_lists() {
    let parts = paste_save(
        r#"
        <b id='docs-internal-guid-golden' style='font-weight:normal'>
        <p style='margin-left:36pt;margin-right:12pt;text-indent:18pt;margin-top:0pt;
          margin-bottom:8pt;line-height:1.25;text-align:center'>
          <span style="font-family:'Golden Paste Sans',sans-serif;font-size:16px;color:rgb(1,2,3);
            font-weight:700;font-style:italic;text-decoration:underline line-through">Google styled text</span></p>
        <ul style='margin-left:0pt'><li style='margin-left:36pt'>Google outer item</li>
          <li style='margin-left:72pt'>Google inner item</li></ul></b>
    "#,
    );
    let document = xml(&parts, "word/document.xml");
    text_fragments(
        document,
        "w:r",
        "Google styled text",
        &[
            "<w:b/>",
            "<w:i/>",
            "<w:u w:val=\"single\"/>",
            "<w:strike/>",
            "<w:color w:val=\"010203\"/>",
            "<w:sz w:val=\"24\"/>",
            "<w:rFonts w:ascii=\"Golden Paste Sans\" w:hAnsi=\"Golden Paste Sans\"/>",
        ],
    );
    text_fragments(
        document,
        "w:p",
        "Google styled text",
        &[
            "<w:ind w:left=\"720\" w:right=\"240\" w:firstLine=\"360\"/>",
            "<w:spacing w:before=\"0\" w:after=\"160\" w:line=\"300\" w:lineRule=\"auto\"/>",
            "<w:jc w:val=\"center\"/>",
        ],
    );
    for (text, level) in [("Google outer item", "0"), ("Google inner item", "1")] {
        text_fragments(document, "w:p", text, &["<w:numPr>", &format!("<w:ilvl w:val=\"{level}\"/>")]);
    }
    fragments("Google numbering", xml(&parts, "word/numbering.xml"), &["<w:numFmt w:val=\"bullet\"/>"]);
    fragments("Google styles", xml(&parts, "word/styles.xml"), &["w:styleId=\"ListParagraph\""]);
}

#[test]
fn libreoffice_html_paste_saves_golden_css_and_heading_style() {
    let parts = paste_save(
        r#"
        <style>p.lo { font-family:'Golden Paste Mono';font-size:10pt;color:#a1b2c3;
          font-weight:600;font-style:oblique;text-decoration:underline line-through;
          margin-left:24pt;margin-right:6pt;text-indent:12pt;margin-top:4pt;
          margin-bottom:10pt;line-height:20px;text-align:right; }</style>
        <p class='lo' lang='en-US'><font face='Golden Paste Mono'>LibreOffice styled text</font></p>
        <h2>LibreOffice heading</h2>
        <ol><li>LibreOffice numbered item</li></ol>
    "#,
    );
    let document = xml(&parts, "word/document.xml");
    text_fragments(
        document,
        "w:r",
        "LibreOffice styled text",
        &[
            "<w:b/>",
            "<w:i/>",
            "<w:u w:val=\"single\"/>",
            "<w:strike/>",
            "<w:color w:val=\"A1B2C3\"/>",
            "<w:sz w:val=\"20\"/>",
            "<w:rFonts w:ascii=\"Golden Paste Mono\" w:hAnsi=\"Golden Paste Mono\"/>",
        ],
    );
    text_fragments(
        document,
        "w:p",
        "LibreOffice styled text",
        &[
            "<w:ind w:left=\"480\" w:right=\"120\" w:firstLine=\"240\"/>",
            "<w:spacing w:before=\"80\" w:after=\"200\" w:line=\"300\" w:lineRule=\"exact\"/>",
            "<w:jc w:val=\"right\"/>",
        ],
    );
    text_fragments(document, "w:p", "LibreOffice heading", &["<w:pStyle w:val=\"Heading2\"/>"]);
    text_fragments(document, "w:p", "LibreOffice numbered item", &["<w:numPr>", "<w:ilvl w:val=\"0\"/>"]);
    fragments("LibreOffice numbering", xml(&parts, "word/numbering.xml"), &["<w:numFmt w:val=\"decimal\"/>"]);
    fragments("LibreOffice styles", xml(&parts, "word/styles.xml"), &["w:styleId=\"Heading2\"", "w:styleId=\"ListParagraph\""]);
}

#[test]
fn merged_table_html_paste_saves_golden_cell_properties() {
    let parts = paste_save(
        r#"
        <style>td.office { border:2pt dashed #123456;border-top:none;
          background-color:rgb(240,241,242);vertical-align:middle; }</style>
        <table><col width='120'><col style='width:2in'><col width='80'>
          <tr><td class='office' rowspan='2'>Merged down</td>
            <td colspan='2' style='border:none;width:204pt'>Merged across</td></tr>
          <tr><td width='192' style='border-bottom:2px double rgb(1,2,3)'>
            <ol><li>Table numbered item</li></ol></td>
            <td width='80' bgcolor='#ffffff' valign='bottom'>Last cell</td></tr></table>
    "#,
    );
    let document = xml(&parts, "word/document.xml");
    fragments("table grid", document, &["<w:gridCol w:w=\"1800\"/>", "<w:gridCol w:w=\"2880\"/>", "<w:gridCol w:w=\"1200\"/>"]);
    text_fragments(
        document,
        "w:tc",
        "Merged down",
        &[
            "<w:tcW w:w=\"1800\" w:type=\"dxa\"/>",
            "<w:vMerge w:val=\"restart\"/>",
            "<w:tcBorders>",
            "<w:top w:val=\"nil\"",
            "<w:left w:val=\"dashed\" w:sz=\"16\" w:space=\"0\" w:color=\"123456\"/>",
            "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F0F1F2\"/>",
            "<w:vAlign w:val=\"center\"/>",
        ],
    );
    text_fragments(
        document,
        "w:tc",
        "Merged across",
        &["<w:tcW w:w=\"4080\" w:type=\"dxa\"/>", "<w:gridSpan w:val=\"2\"/>", "<w:tcBorders>", "w:val=\"nil\""],
    );
    let continuations: Vec<_> = elements(document, "w:tc").into_iter().filter(|c| c.contains("<w:vMerge/>")).collect();
    assert_eq!(continuations.len(), 1, "one continuation for rowspan=2");
    fragments("vertical continuation", continuations[0], &["<w:tcW w:w=\"1800\" w:type=\"dxa\"/>"]);
    text_fragments(
        document,
        "w:tc",
        "Table numbered item",
        &["<w:tcW w:w=\"2880\" w:type=\"dxa\"/>", "<w:tcBorders>", "<w:bottom w:val=\"double\" w:sz=\"12\" w:space=\"0\" w:color=\"010203\"/>"],
    );
    text_fragments(document, "w:p", "Table numbered item", &["<w:numPr>"]);
    text_fragments(document, "w:tc", "Last cell", &["<w:tcW w:w=\"1200\" w:type=\"dxa\"/>", "<w:vAlign w:val=\"bottom\"/>"]);
    fragments("table numbering", xml(&parts, "word/numbering.xml"), &["<w:numFmt w:val=\"decimal\"/>"]);
    fragments("table styles", xml(&parts, "word/styles.xml"), &["w:styleId=\"ListParagraph\""]);
}

fn solid_png(pixel: [u8; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes).write_image(&pixel, 1, 1, image::ExtendedColorType::Rgba8).expect("synthetic PNG");
    bytes
}

fn add_picture(doc: &mut Document, pos: &Pos, bytes: &[u8]) {
    let media = doc.add_media(bytes.to_vec(), "png");
    let picture = InlineObject::Image { media, w: 24.0, h: 18.0, alt: "Golden picture".into(), float: Default::default(), crop: [0.0; 4], ole: None };
    doc.insert_object(pos, picture, &CharProps::default()).expect("insert picture");
}

fn add_numbering(doc: &mut Document, format: NumFormat) {
    doc.numbering.abstracts.push(AbstractNum { id: 7, levels: vec![Level { format, ..Default::default() }], ..Default::default() });
    doc.numbering.nums.push(Num { id: 42, abstract_id: 7, ..Default::default() });
    doc.para_mut(StoryRef::Body, &Path::top(0)).expect("first paragraph").props.numbering = Some(NumRef { num: 42, level: 0 });
}

#[test]
fn cross_document_copy_paste_save_resolves_styles_numbering_and_picture_relationships() {
    let source_png = solid_png([32, 96, 160, 255]);
    let destination_png = solid_png([160, 96, 32, 255]);
    let mut source = Document::from_text("Source item");
    source.styles.upsert(Style {
        id: "SourceList".into(),
        name: "Source List".into(),
        based_on: Some("Normal".into()),
        chr: CharProps { italic: Some(true), ..Default::default() },
        ..Default::default()
    });
    source.styles.upsert(Style {
        id: "SourceAccent".into(),
        name: "Source Accent".into(),
        kind: StyleKind::Character,
        chr: CharProps { bold: Some(true), color: Some(TextColor::Rgb(Rgb(0x12, 0x34, 0x56))), ..Default::default() },
        ..Default::default()
    });
    let props = CharProps { style: Some("SourceAccent".into()), underline: Some(Underline::Single), ..Default::default() };
    source.format_range(&Pos::body(0, 0), &Pos::body(0, 11), &|p| *p = props.clone()).expect("format source");
    source.para_mut(StoryRef::Body, &Path::top(0)).expect("source paragraph").props.style = Some("SourceList".into());
    add_numbering(&mut source, NumFormat::Decimal);
    add_picture(&mut source, &Pos::body(0, 11), &source_png);
    let mut a = Session::new(source);
    run(&mut a, "select.all", json!({}));
    let clipboard = run(&mut a, "edit.copy", json!({}));
    assert!(clipboard["html"].as_str().expect("HTML clipboard").contains("name=\"erpcons-clip\""));

    let mut destination = Document::from_text("Destination\n");
    add_numbering(&mut destination, NumFormat::LowerRoman);
    add_picture(&mut destination, &Pos::body(0, 11), &destination_png);
    let mut b = Session::new(destination);
    b.sel = Selection::caret(Pos::body(1, 0));
    run(&mut b, "edit.paste", clipboard);
    let parts = save(&b);
    let document = xml(&parts, "word/document.xml");
    text_fragments(document, "w:p", "Source item", &["<w:pStyle w:val=\"SourceList\"/>", "<w:numPr>"]);
    text_fragments(document, "w:r", "Source item", &["<w:rStyle w:val=\"SourceAccent\"/>", "<w:u w:val=\"single\"/>"]);
    let styles = xml(&parts, "word/styles.xml");
    let definitions = elements(styles, "w:style");
    let list_style = definitions.iter().find(|s| attr(s, "w:styleId") == "SourceList").expect("copied paragraph style");
    fragments("copied paragraph style", list_style, &["<w:basedOn w:val=\"Normal\"/>", "<w:i/>"]);
    let accent_style = definitions.iter().find(|s| attr(s, "w:styleId") == "SourceAccent").expect("copied character style");
    fragments("copied character style", accent_style, &["<w:b/>", "<w:color w:val=\"123456\"/>"]);
    assert_styles_resolve(&parts);
    assert_numbering_resolves(&parts);
    let numbering = xml(&parts, "word/numbering.xml");
    for (text, format) in [("Source item", "decimal"), ("Destination", "lowerRoman")] {
        let paragraph = containing_text(document, "w:p", text);
        let refs = tags(paragraph, "w:numId");
        assert_eq!(refs.len(), 1, "{paragraph}");
        let id = attr(refs[0], "w:val");
        if text == "Source item" {
            assert_ne!(id, "42", "source numbering must be remapped around the destination instance");
        }
        let instances = elements(numbering, "w:num");
        let instance = instances.iter().find(|n| attr(n, "w:numId") == id).expect("numbering instance");
        let refs = tags(instance, "w:abstractNumId");
        let abstract_id = attr(refs[0], "w:val");
        let abstracts = elements(numbering, "w:abstractNum");
        let definition = abstracts.iter().find(|n| attr(n, "w:abstractNumId") == abstract_id).expect("abstract numbering");
        fragments(text, definition, &[&format!("<w:numFmt w:val=\"{format}\"/>")]);
    }

    let rels = xml(&parts, "word/_rels/document.xml.rels");
    let relationships = tags(rels, "Relationship");
    let mut targets = Vec::new();
    for (text, expected_png) in [("Source item", source_png), ("Destination", destination_png)] {
        let paragraph = containing_text(document, "w:p", text);
        let blips = tags(paragraph, "a:blip");
        assert_eq!(blips.len(), 1, "{paragraph}");
        let id = attr(blips[0], "r:embed");
        let relationship = relationships.iter().find(|r| attr(r, "Id") == id).expect("image relationship");
        assert!(attr(relationship, "Type").ends_with("/image"), "{relationship}");
        assert!(!relationship.contains("TargetMode=\"External\""), "{relationship}");
        let target = attr(relationship, "Target");
        assert!(target.starts_with("media/"), "{target}");
        let path = format!("word/{target}");
        assert_eq!(parts.get(&path).expect("referenced media part"), &expected_png, "{text}: picture bytes");
        targets.push(target);
    }
    assert_ne!(targets[0], targets[1], "source and destination pictures stay distinct");
    assert_eq!(document.matches("r:embed=\"").count(), 2, "every picture reference is checked");
}
