use super::*;
use serde_json::json;
use wordcraft_engine::Session;
use wordcraft_ui_egui::WordApp;

#[test]
fn html_plain_strips_tags_and_decodes_entities_without_a_dom() {
    assert_eq!(
        html_to_plain("<p title='1 > 0'>Tiếng <b>Việt</b>&nbsp;&amp; &#x1f642;</p><p>&#65;&lt;&gt;&quot;&apos;</p>"),
        "Tiếng Việt & 🙂\nA<>\"'"
    );
    assert_eq!(html_to_plain("<head><style>hidden</style></head><!-- x > hidden --><p>Shown<br>next</p><script>alert(1)</script>"), "Shown\nnext");
    assert_eq!(html_to_plain("<table><tr><td>A</td><td>B</td></tr><tr><td>C</td></tr></table>"), "A\tB\t\nC");
    assert_eq!(html_to_plain("&unknown; &#x110000; &amp unfinished<"), "&unknown; &#x110000; &amp unfinished");
}

#[test]
fn caps_drop_oversized_rich_parts_and_bound_images() {
    let mut paste = PendingPaste {
        text: "fallback".into(),
        html: Some("x".repeat(MAX_TEXT_BYTES + 1)),
        rtf: Some("x".repeat(MAX_TEXT_BYTES + 1)),
        images: vec![("image/png".into(), vec![0; MAX_IMAGE_BYTES + 1]), ("text/plain".into(), vec![0])],
    };
    paste.images.extend((0..MAX_IMAGES + 1).map(|_| ("image/png".into(), vec![1])));
    assert!(paste.enforce_caps());
    assert_eq!(paste.text, "fallback");
    assert!(paste.html.is_none() && paste.rtf.is_none());
    assert_eq!(paste.images.len(), MAX_IMAGES);
    assert!(!paste.enforce_caps());
    let mut boundary = PendingPaste {
        html: Some("x".repeat(MAX_TEXT_BYTES)),
        images: vec![("image/png".into(), vec![0; MAX_IMAGE_BYTES])],
        ..Default::default()
    };
    assert!(!boundary.enforce_caps());
    assert_eq!(boundary.text.len(), MAX_TEXT_BYTES);
    let mut unicode = PendingPaste { text: "ế".repeat(MAX_TEXT_BYTES / 3 + 1), ..Default::default() };
    assert!(unicode.enforce_caps());
    assert!(unicode.text.len() <= MAX_TEXT_BYTES && unicode.text.ends_with('ế'));
}

#[test]
fn html_only_paste_produces_exactly_one_event_and_preserves_the_html() {
    let bridge = ClipboardBridge::default();
    let html = "<meta name=\"erpcons-clip\" content=\"opaque\"><p><b>strong</b></p>";
    bridge.deliver(PendingPaste { html: Some(html.into()), ..Default::default() }, vec!["text/html".into()], None);
    let mut raw = egui::RawInput::default();
    assert!(bridge.begin_frame(&mut raw, &Inbox::default()).is_none());
    assert_eq!(raw.events, vec![egui::Event::Paste("strong".into())]);
    assert_eq!(bridge.peek(|p| p.html.clone()).flatten().as_deref(), Some(html));
    assert!(bridge.take().is_none());
}

#[test]
fn image_only_paste_uses_the_picture_inbox_without_a_text_event() {
    let bridge = ClipboardBridge::default();
    let images = vec![("image/jpeg".into(), vec![1, 2]), ("image/png".into(), vec![3])];
    bridge.deliver(PendingPaste { images, ..Default::default() }, vec!["image/jpeg".into()], None);
    let pictures = Inbox::default();
    let mut raw = egui::RawInput::default();
    assert!(bridge.begin_frame(&mut raw, &pictures).is_none());
    assert!(raw.events.is_empty());
    assert_eq!(*pictures.lock().unwrap(), vec![("pasted.jpg".into(), vec![1, 2]), ("pasted.png".into(), vec![3])]);
}

#[test]
fn rich_text_with_an_image_does_not_also_insert_a_picture() {
    let bridge = ClipboardBridge::default();
    bridge.deliver(PendingPaste { text: "caption".into(), images: vec![("image/png".into(), vec![3])], ..Default::default() }, vec![], None);
    let mut services = Services::default();
    bridge.connect(&mut services);
    let pictures = Inbox::default();
    let mut raw = egui::RawInput::default();
    bridge.begin_frame(&mut raw, &pictures);
    assert_eq!(raw.events, vec![egui::Event::Paste("caption".into())]);
    assert!(pictures.lock().unwrap().is_empty());
    assert!(matches!(services.clipboard_picture.as_ref().unwrap()(), Some(ClipboardPicture::Encoded(bytes)) if bytes == [3]));
}

#[test]
fn menu_read_replays_the_requested_command_once_with_fresh_data() {
    let bridge = ClipboardBridge::default();
    bridge.deliver(PendingPaste { text: "fresh".into(), ..Default::default() }, vec!["text/plain".into()], Some("edit.pasteText".into()));
    let pictures = Inbox::default();
    let mut raw = egui::RawInput::default();
    assert_eq!(bridge.begin_frame(&mut raw, &pictures).as_deref(), Some("edit.pasteText"));
    assert!(raw.events.is_empty(), "replaying a menu command must not also send a document paste event");
    assert!(bridge.begin_frame(&mut raw, &pictures).is_none());
    assert!(bridge.peek(|p| p.text.clone()).is_none());
}

fn app(bridge: &ClipboardBridge) -> WordApp {
    let mut services = Services::default();
    bridge.connect(&mut services);
    WordApp::new(Session::new(wordcraft_doc::Document::new()), services)
}

fn paste_frame(bridge: &ClipboardBridge, app: &mut WordApp) {
    let mut raw = egui::RawInput::default();
    let command = bridge.begin_frame(&mut raw, &Inbox::default());
    if let Some(command) = command {
        app.run(&command, json!({})).unwrap();
    }
    let ctx = egui::Context::default();
    ctx.run_ui(raw, |ui| wordcraft_ui_egui::keys::canvas_events(app, ui.ctx())).drop_without_applying_deltas();
}

#[test]
fn browser_paste_event_keeps_html_bold_through_services() {
    let bridge = ClipboardBridge::default();
    let mut app = app(&bridge);
    bridge.deliver(
        PendingPaste { text: "plain strong".into(), html: Some("<p>plain <b>strong</b></p>".into()), ..Default::default() },
        vec!["text/html".into()],
        None,
    );
    paste_frame(&bridge, &mut app);
    let para = app.session.doc.para_at(&app.session.sel.focus).unwrap();
    assert_eq!(para.text, "plain strong");
    assert_eq!(para.props_of_char(7).bold, Some(true));
}

#[test]
fn next_frame_clears_all_hooks_and_next_paste_cannot_reuse_rich_data() {
    let bridge = ClipboardBridge::default();
    let mut app = app(&bridge);
    bridge.deliver(
        PendingPaste {
            text: "strong".into(),
            html: Some("<b>strong</b>".into()),
            rtf: Some("{\\rtf1 strong}".into()),
            images: vec![("image/png".into(), vec![1])],
        },
        vec!["text/html".into(), "text/rtf".into(), "image/png".into()],
        None,
    );
    paste_frame(&bridge, &mut app);
    assert!(app.services.clipboard_picture.as_ref().unwrap()().is_some());
    app.run("edit.undo", json!({})).unwrap();
    bridge.begin_frame(&mut egui::RawInput::default(), &Inbox::default());
    assert!(app.services.clipboard_text.as_ref().unwrap()().is_none());
    assert!(app.services.clipboard_html.as_ref().unwrap()().is_none());
    assert!(app.services.clipboard_rtf.as_ref().unwrap()().is_none());
    assert!(app.services.clipboard_picture.as_ref().unwrap()().is_none());
    assert!(app.services.clipboard_formats.as_ref().unwrap()().is_empty());
    bridge.deliver(PendingPaste { text: "plain".into(), ..Default::default() }, vec!["text/plain".into()], None);
    paste_frame(&bridge, &mut app);
    let para = app.session.doc.para_at(&app.session.sel.focus).unwrap();
    assert_eq!(para.text, "plain");
    assert!(!para.props_of_char(0).bold.unwrap_or(false));
}

#[test]
fn html_inputs_are_left_to_the_browser_except_for_the_egui_text_agent() {
    assert!(native_text_target("INPUT", false, false));
    assert!(native_text_target("TEXTAREA", false, false));
    assert!(native_text_target("DIV", false, true));
    assert!(!native_text_target("INPUT", true, false));
    assert!(!native_text_target("CANVAS", false, false));
}

#[test]
fn notices_are_visible_localized_and_do_not_accumulate_duplicates() {
    use wordcraft_ui_egui::i18n;
    let previous = i18n::current();
    i18n::set_current(i18n::Lang::from_pref("vi"));
    assert_eq!(Notice::ReadFailed.message(), "Trình duyệt không cho đọc bảng tạm — hãy dùng Ctrl+V");
    assert_eq!(Notice::WriteFailed.message(), "Không ghi được vào bảng tạm");
    assert_eq!(Notice::TooLarge.message(), "Một phần nội dung bảng tạm quá lớn nên đã được bỏ qua");
    i18n::set_current(i18n::Lang::EN);
    assert_eq!(Notice::WriteFailed.message(), "Could not write to the clipboard");
    i18n::set_current(previous);
    let bridge = ClipboardBridge::default();
    bridge.notice(Notice::ReadFailed);
    bridge.notice(Notice::ReadFailed);
    assert_eq!(bridge.take_notices(), vec![Notice::ReadFailed]);
    assert!(bridge.take_notices().is_empty());
}
