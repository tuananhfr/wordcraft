//! `Services::intake_result`: every file from the inbox is reported once, after the app decided.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use wordcraft_doc::Document;
use wordcraft_engine::Session;

use crate::{Services, WordApp};

type Reports = Rc<RefCell<Vec<(String, usize, bool)>>>;

fn app() -> (WordApp, Reports) {
    let reports = Reports::default();
    let sink = reports.clone();
    let services = Services {
        inbox: Some(Arc::new(Mutex::new(Vec::new()))),
        intake_result: Some(Box::new(move |name: &str, size, ok| sink.borrow_mut().push((name.to_string(), size, ok)))),
        ..Default::default()
    };
    (WordApp::new(Session::new(Document::from_text("Already open")), services), reports)
}

fn deliver(app: &mut WordApp, name: &str, bytes: Vec<u8>) {
    app.services.inbox.as_ref().unwrap().lock().unwrap().push((name.to_string(), bytes));
    app.drain_inbox();
}

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(4, 4, |_, _| image::Rgba([10, 120, 200, 255]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

#[test]
fn each_file_reports_whether_it_opened_or_was_inserted() {
    let (mut app, reports) = app();
    let docx = wordcraft_engine::io::save_bytes("x.docx", &Document::from_text("Hợp đồng")).unwrap();
    let cases: [(&str, Vec<u8>, bool); 5] = [
        ("Hợp đồng.docx", docx.clone(), true),
        ("hỏng.docx", b"PK\x03\x04 not really a package".to_vec(), false),
        ("ảnh.png", png(), true),
        ("danh sách.csv", b"Name,City\nAn,Hue\n".to_vec(), true),
        ("phông.ttf", b"not a font".to_vec(), false),
    ];
    for (name, bytes, _) in &cases {
        deliver(&mut app, name, bytes.clone());
    }
    let expected: Vec<_> = cases.iter().map(|(name, bytes, ok)| (name.to_string(), bytes.len(), *ok)).collect();
    assert_eq!(*reports.borrow(), expected);
    assert!(app.document_count() >= 2, "the good document opened beside the first");
}

#[test]
fn the_open_documents_limit_reports_a_refusal() {
    let (mut app, reports) = app();
    let docx = wordcraft_engine::io::save_bytes("x.docx", &Document::from_text("Một")).unwrap();
    for i in 0..crate::open_documents::MAX_DOCUMENTS {
        deliver(&mut app, &format!("{i}.docx"), docx.clone());
    }
    let reports = reports.borrow();
    assert_eq!(reports.len(), crate::open_documents::MAX_DOCUMENTS);
    assert!(reports.iter().take(crate::open_documents::MAX_DOCUMENTS - 1).all(|(_, _, ok)| *ok));
    assert_eq!(reports.last(), Some(&(format!("{}.docx", crate::open_documents::MAX_DOCUMENTS - 1), docx.len(), false)));
}
