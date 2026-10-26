//! The optional PI ID account UI against a fake hook: nothing shows without one; File › Account,
//! the title bar's button and the expiry notice call the hook and show its snapshot.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::WordApp;
use crate::account::{
    ACCOUNT_PAGE, AccountError, AccountHook, AccountSnapshot, AccountView, LicenceView, OrgView, SignedInView, WarningView, date_text, error_text,
};
use crate::i18n;

/// 2026-10-24 (UTC).
const EXPIRES: i64 = 1_792_828_800;

#[derive(Default)]
struct Fake {
    snapshot: Mutex<AccountSnapshot>,
    calls: Mutex<Vec<String>>,
    attached: AtomicUsize,
}

impl Fake {
    fn new(view: AccountView) -> Arc<Fake> {
        let fake = Arc::new(Fake::default());
        fake.set(view);
        fake
    }

    fn set(&self, view: AccountView) {
        self.snapshot.lock().unwrap().view = view;
    }

    fn set_error(&self, error: Option<AccountError>) {
        self.snapshot.lock().unwrap().error = error;
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn call(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

struct Hook(Arc<Fake>);

impl AccountHook for Hook {
    fn snapshot(&self) -> AccountSnapshot {
        self.0.snapshot.lock().unwrap().clone()
    }
    fn sign_in(&self) {
        self.0.call("sign_in".into());
    }
    fn cancel_sign_in(&self) {
        self.0.call("cancel_sign_in".into());
    }
    fn sign_out(&self) {
        self.0.call("sign_out".into());
    }
    fn select_org(&self, org_id: Option<String>) {
        self.0.call(format!("select_org:{}", org_id.as_deref().unwrap_or("personal")));
    }
    fn retry_activation(&self) {
        self.0.call("retry_activation".into());
    }
    fn allowed(&self, feature: &str) -> bool {
        self.0.call(format!("allowed:{feature}"));
        false
    }
    fn warning_shown(&self, days: u32) {
        self.0.call(format!("warning_shown:{days}"));
    }
    fn attach(&self, _ctx: &egui::Context) {
        self.0.attached.fetch_add(1, Ordering::SeqCst);
    }
}

fn app(fake: Option<&Arc<Fake>>, language: &str) -> WordApp {
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Draft")), Default::default());
    app.ui.language = language.into();
    app.services.account = fake.map(|f| Box::new(Hook(f.clone())) as Box<dyn AccountHook>);
    app
}

fn harness_in(fake: Option<&Arc<Fake>>, width: f32, language: &str) -> egui_kittest::Harness<'static, WordApp> {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(width, 844.0)).build_ui_state(
        |ui, app: &mut WordApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app(fake, language),
    );
    h.run_steps(6);
    h
}

fn harness(fake: Option<&Arc<Fake>>) -> egui_kittest::Harness<'static, WordApp> {
    harness_in(fake, 1366.0, "en")
}

fn account_page(h: &mut egui_kittest::Harness<'static, WordApp>) {
    h.state_mut().show_account_page();
    h.run_steps(4);
}

fn org(id: &str, name: &str, role: Option<&str>) -> OrgView {
    OrgView { id: id.into(), name: name.into(), role: role.map(Into::into) }
}

fn signed_in(licence: LicenceView) -> SignedInView {
    SignedInView {
        name: "Nguyễn An".into(),
        email: Some("an.nguyen@example.com".into()),
        orgs: vec![org("org_a", "Công ty A", Some("admin")), org("org_b", "Công ty B", None)],
        active_org: None,
        licence,
        warning: None,
        online: true,
        session_only: false,
        reauth_required: false,
    }
}

#[test]
fn without_a_hook_nothing_new_shows_and_everything_is_allowed() {
    let mut h = harness(None);
    assert!(h.state().allowed("vanban.advanced"));
    assert!(h.query_by_label("Sign in").is_none());
    h.state_mut().show_account_page();
    assert!(!h.state().ui.backstage, "no account page to open");
    h.state_mut().run("ui.backstage", json!({"value": true, "page": ACCOUNT_PAGE})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Options").is_some(), "the Backstage is open");
    assert!(h.query_by_label("Account").is_none());
    assert!(h.query_by_label("Sign in with PI ID").is_none());
    assert!(h.query_by_label_contains("Good").is_some(), "a stale account page falls back to Home");
}

#[test]
fn a_guest_opens_the_page_from_the_title_bar_and_signs_in() {
    let fake = Fake::new(AccountView::Guest);
    let mut h = harness(Some(&fake));
    assert_eq!(fake.attached.load(Ordering::SeqCst), 1, "attached once");
    h.get_by_label("Sign in").click();
    h.run_steps(4);
    assert!(h.state().ui.backstage);
    assert_eq!(h.state().ui.backstage_page, ACCOUNT_PAGE);
    assert_eq!(h.query_all_by_label("Account").count(), 2, "listed in the Backstage and the page's heading");
    assert!(h.query_by_label_contains("every feature works without an account").is_some());
    h.get_by_label("Sign in with PI ID").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["sign_in"]);
    assert_eq!(fake.attached.load(Ordering::SeqCst), 1);
    assert!(!h.state().allowed("vanban.advanced"), "the hook answers");
}

#[test]
fn signing_in_waits_for_the_browser_and_cancel_calls_the_hook() {
    let fake = Fake::new(AccountView::SigningIn);
    let mut h = harness(Some(&fake));
    account_page(&mut h);
    assert!(h.query_by_label("Continue in your browser…").is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["cancel_sign_in"]);
}

#[test]
fn signed_in_lists_the_organizations_switches_and_signs_out() {
    let fake = Fake::new(AccountView::SignedIn(signed_in(LicenceView::Free)));
    let mut h = harness(Some(&fake));
    assert!(h.query_by_label("Nguyễn An").is_some(), "the title bar button names the user");
    account_page(&mut h);
    assert!(h.query_by_label("an.nguyen@example.com").is_some());
    assert!(h.query_by_label("Plan: Free").is_some());
    assert!(h.query_by_label("Connected to PI ID").is_some());
    assert!(h.query_by_label("Công ty A").is_some());
    h.get_by_label("Personal").click();
    h.run_steps(2);
    assert!(fake.calls().is_empty(), "the active context is not switched again");
    h.get_by_label("Công ty B").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["select_org:org_b"]);
    fake.set(AccountView::SignedIn(SignedInView { active_org: Some("org_b".into()), online: false, ..signed_in(LicenceView::Free) }));
    h.run_steps(2);
    assert!(h.query_by_label_contains("Offline").is_some());
    h.get_by_label("Personal").click();
    h.run_steps(2);
    h.get_by_label("Sign out").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["select_org:org_b", "select_org:personal", "sign_out"]);
}

#[test]
fn reauth_offers_sign_in_again() {
    let fake = Fake::new(AccountView::SignedIn(SignedInView { reauth_required: true, ..signed_in(LicenceView::Free) }));
    let mut h = harness(Some(&fake));
    account_page(&mut h);
    h.get_by_label("Sign in again").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["sign_in"]);
}

#[test]
fn licence_lines_and_device_limit_retry() {
    let fake = Fake::new(AccountView::SignedIn(signed_in(LicenceView::Active { expires_at: EXPIRES, pro: false })));
    let mut h = harness(Some(&fake));
    account_page(&mut h);
    assert!(h.query_by_label("Plan: Free").is_some());
    assert!(h.query_by_label("This device is activated until 2026-10-24.").is_some());
    assert!(h.query_by_label("Retry").is_none());
    fake.set(AccountView::SignedIn(signed_in(LicenceView::DeviceCapReached)));
    h.run_steps(2);
    assert!(h.query_by_label_contains("device limit").is_some());
    h.get_by_label("Retry").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["retry_activation"]);
    for (licence, text) in [
        (LicenceView::Active { expires_at: EXPIRES, pro: true }, "Paid features are valid until 2026-10-24."),
        (LicenceView::Inactive { expires_at: EXPIRES }, "Your license expired on 2026-10-24. Free features keep working."),
        (LicenceView::DeviceRevoked, "This device was removed from your account. Free features keep working."),
        (LicenceView::SessionOnly, "Signed in for this session only: this computer has no credential store."),
        (LicenceView::Online { pro: true }, "Paid features are active."),
    ] {
        fake.set(AccountView::SignedIn(signed_in(licence)));
        h.run_steps(2);
        assert!(h.query_by_label(text).is_some(), "{licence:?}");
    }
}

#[test]
fn the_expiry_notice_shows_only_for_a_paid_lease_and_closing_records_it() {
    let warning = Some(WarningView { days: 7, expires_at: EXPIRES });
    for licence in [LicenceView::Active { expires_at: EXPIRES, pro: false }, LicenceView::Free, LicenceView::Online { pro: true }] {
        let fake = Fake::new(AccountView::SignedIn(SignedInView { warning, ..signed_in(licence) }));
        let h = harness(Some(&fake));
        assert!(h.query_by_label_contains("Paid features expire").is_none(), "{licence:?}");
    }
    let fake = Fake::new(AccountView::SignedIn(signed_in(LicenceView::Active { expires_at: EXPIRES, pro: true })));
    let mut h = harness(Some(&fake));
    assert!(h.query_by_label_contains("Paid features expire").is_none(), "no warning due");
    fake.set(AccountView::SignedIn(SignedInView { warning, ..signed_in(LicenceView::Active { expires_at: EXPIRES, pro: true }) }));
    h.run_steps(2);
    assert!(h.query_by_label("Paid features expire on 2026-10-24 (7 days left). Connect to the internet to renew them.").is_some());
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert_eq!(fake.calls(), ["warning_shown:7"]);
    assert!(h.query_by_label_contains("Paid features expire").is_none(), "hidden before the adapter drops it");
}

#[test]
fn errors_show_in_plain_language_and_a_cancelled_sign_in_shows_nothing() {
    let fake = Fake::new(AccountView::Guest);
    fake.set_error(Some(AccountError::RateLimited { retry_after_secs: Some(90) }));
    let mut h = harness(Some(&fake));
    account_page(&mut h);
    assert!(h.query_by_label("PI ID is busy. Try again in 2 min.").is_some());
    fake.set_error(Some(AccountError::Cancelled));
    h.run_steps(2);
    assert!(h.query_by_label_contains("PI ID is").is_none());
    assert_eq!(error_text(AccountError::Cancelled), None);
    fake.set_error(Some(AccountError::PopupBlocked));
    h.run_steps(2);
    assert!(h.query_by_label("The browser blocked the sign-in window. Allow pop-ups for this site and try again.").is_some());
    assert!(h.query_by_label("Sign in with PI ID").is_some(), "the guest can try again");
    let previous = i18n::current();
    i18n::set_current(i18n::Lang::from_pref("vi"));
    assert_eq!(error_text(AccountError::Network).as_deref(), Some("Không kết nối được PI ID. Hãy kiểm tra mạng rồi thử lại."));
    assert_eq!(error_text(AccountError::RateLimited { retry_after_secs: None }).as_deref(), Some("PI ID đang bận. Hãy thử lại sau ít phút."));
    assert_eq!(
        error_text(AccountError::PopupBlocked).as_deref(),
        Some("Trình duyệt đã chặn cửa sổ đăng nhập. Hãy cho phép cửa sổ bật lên cho trang này rồi thử lại.")
    );
    assert_eq!(date_text(EXPIRES), "24/10/2026");
    i18n::set_current(i18n::Lang::EN);
    assert_eq!(date_text(EXPIRES), "2026-10-24");
    i18n::set_current(previous);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn desktop_dates_show_the_local_calendar_day() {
    const CHILD: &str = "WORDCRAFT_TEST_LOCAL_DATE_CHILD";
    const NAME: &str = "tests_account::desktop_dates_show_the_local_calendar_day";
    // 2026-10-24 20:00 UTC is already 25 October at UTC+7.
    const EVENING_UTC: i64 = 1_792_872_000;
    if std::env::var_os(CHILD).is_some() {
        i18n::set_current(i18n::Lang::EN);
        let local = chrono::DateTime::from_timestamp(EVENING_UTC, 0).unwrap().with_timezone(&chrono::Local);
        assert_eq!(date_text(EVENING_UTC), local.format("%Y-%m-%d").to_string());
        // chrono reads `TZ` only on Unix; on Windows the comparison above uses the host's own zone.
        #[cfg(unix)]
        assert_eq!(date_text(EVENING_UTC), "2026-10-25", "TZ=<+07>-7 must move the day forward");
        return;
    }
    // The gate container runs in UTC, where local and UTC days agree. `TZ` is set only on a child run of this
    // one test, so no other test in this process sees the zone change.
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([NAME, "--exact", "--test-threads=1", "--nocapture"])
        .env(CHILD, "1")
        .env("TZ", "<+07>-7")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && stdout.contains("1 passed"), "child run failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn the_account_button_keeps_narrow_title_bars_from_overlapping() {
    for (w, lang) in [(360.0, "en"), (390.0, "vi"), (768.0, "vi"), (1366.0, "vi")] {
        let fake = Fake::new(AccountView::SignedIn(signed_in(LicenceView::Free)));
        let mut a = app(Some(&fake), lang);
        a.session.path = Some("Báo cáo tiến độ thi công công trình nhà ở quý ba.docx".into());
        a.run("text.insert", json!({"text": " edited"})).unwrap();
        let texts = crate::chrome::tests::chrome_texts(&mut a, w);
        assert!(texts.iter().any(|(s, _)| s == "A"), "{w}: the given name's initial shows");
        for (i, (s, r)) in texts.iter().enumerate() {
            assert!(r.min.x >= -0.5 && r.max.x <= w + 0.5, "{w}: {s:?} at {r:?} is off the window");
            for (s2, r2) in &texts[i + 1..] {
                assert!(!r.shrink(0.5).intersects(r2.shrink(0.5)), "{w}: {s:?} {r:?} overlaps {s2:?} {r2:?}");
            }
        }
    }
}

#[test]
fn a_phone_wide_account_page_keeps_names_and_addresses_whole_inside_the_page() {
    let name = "Nguyễn Hoàng Bảo Ngọc Phương";
    let email = "nguyen.hoang.bao.ngoc@congty-xaydung-vidu.com.vn";
    let long_org = "Công ty Cổ phần Xây dựng và Kiến trúc Ví dụ";
    let view = SignedInView {
        name: name.into(),
        email: Some(email.into()),
        orgs: vec![org("org_a", long_org, Some("license_admin")), org("org_b", "Văn phòng Kiến trúc Mẫu", None)],
        ..signed_in(LicenceView::Active { expires_at: EXPIRES, pro: false })
    };
    let width = 390.0;
    let fake = Fake::new(AccountView::SignedIn(view));
    let mut h = harness_in(Some(&fake), width, "en");
    account_page(&mut h);
    // The Backstage page starts at its heading and keeps a 40 px margin from the window's right edge.
    let heading = h.query_all_by_label("Account").map(|n| n.rect()).find(|r| r.min.x >= 200.0).expect("the page heading");
    let (left, right) = (heading.min.x, width - 40.0);
    // Below the heading: the title bar's button carries the user's name too.
    let rect = |label: &str| {
        let rects: Vec<_> = h.query_all_by_label(label).map(|n| n.rect()).filter(|r| r.min.y > heading.min.y).collect();
        assert_eq!(rects.len(), 1, "{label:?} on the page: {rects:?}");
        rects[0]
    };
    let one_row = rect("Plan: Free").height();
    let radio_row = rect("Personal").height();
    for (label, row) in [(name, 18.0 * 1.4), (email, one_row), (long_org, radio_row), ("Văn phòng Kiến trúc Mẫu", radio_row)] {
        let r = rect(label);
        assert!(r.height() <= row + 0.5, "{label:?} wraps: {r:?}");
    }
    for label in [
        name,
        email,
        long_org,
        "Personal",
        "Organization",
        "License",
        "Plan: Free",
        "This device is activated until 2026-10-24.",
        "Connected to PI ID",
        "Sign out",
    ] {
        let r = rect(label);
        assert!(r.min.x >= left - 0.5 && r.max.x <= right + 0.5, "{label:?} at {r:?} leaves the page {left}..{right}");
    }
}

#[test]
fn every_account_string_is_translated_in_vietnamese_and_polish() {
    let keys = [
        "Account",
        "Sign in with PI ID",
        "Signing in is optional: every feature works without an account.",
        "Continue in your browser…",
        "PI ID account",
        "Your session has expired. Sign in again.",
        "Sign in again",
        "Organization",
        "Personal",
        "License",
        "You will need to sign in again the next time you open the app.",
        "Connected to PI ID",
        "Offline. Your account syncs when you reconnect.",
        "Sign out",
        "Plan: Free",
        "This device is activated until {date}.",
        "Paid features are valid until {date}.",
        "Paid features are active.",
        "Your license expired on {date}. Free features keep working.",
        "This device was removed from your account. Free features keep working.",
        "Your account has reached its device limit, so this device is not activated.",
        "Signed in for this session only: this computer has no credential store.",
        "Signing in…",
        "Sign in",
        "Paid features expire on {date} ({days} days left). Connect to the internet to renew them.",
        "Could not reach PI ID. Check your connection and try again.",
        "PI ID took too long to answer. Try again.",
        "PI ID refused the request. Free features keep working.",
        "PI ID is busy. Try again in {minutes} min.",
        "PI ID is busy. Try again in a few minutes.",
        "Your account has reached its device limit. Remove a device in PI ID, then retry.",
        "Your PI ID account is suspended. Free features keep working.",
        "This organization is not available to your account.",
        "This version of the app is too old for PI ID. Update the app, then try again.",
        "PI ID is temporarily unavailable. Try again later.",
        "Something went wrong with PI ID. Try again.",
        "The browser blocked the sign-in window. Allow pop-ups for this site and try again.",
        "Signed in to PI ID. You can close this tab and go back to the app.",
        "Sign-in did not finish. Close this tab and try again in the app.",
    ];
    for code in ["vi", "pl"] {
        let lang = i18n::Lang::from_pref(code);
        for key in keys {
            assert!(i18n::has(lang, key), "{code}: {key}");
        }
    }
    let vi = i18n::Lang::from_pref("vi");
    assert_eq!(i18n::tr(vi, "Account"), "Tài khoản");
    assert_eq!(i18n::tr(vi, "Sign in with PI ID"), "Đăng nhập bằng PI ID");
}

/// Renders the page states and the title bar to PNGs when `WORDCRAFT_ACCOUNT_SCREENSHOTS` names a folder.
#[test]
fn account_screenshots() {
    let Some(dir) = std::env::var_os("WORDCRAFT_ACCOUNT_SCREENSHOTS") else { return };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let error = SignedInView { orgs: vec![org("org_a", "Công ty A", Some("admin"))], ..signed_in(LicenceView::DeviceCapReached) };
    let shots = [
        ("account-guest", AccountView::Guest, None, true),
        ("account-signed-in", AccountView::SignedIn(signed_in(LicenceView::Active { expires_at: EXPIRES, pro: false })), None, true),
        ("account-error", AccountView::SignedIn(error), Some(AccountError::DeviceCap), true),
        ("title-bar-guest", AccountView::Guest, None, false),
        ("title-bar-signed-in", AccountView::SignedIn(signed_in(LicenceView::Free)), None, false),
    ];
    for (name, view, error, page) in shots {
        for width in [1280.0, 390.0] {
            let fake = Fake::new(view.clone());
            fake.set_error(error);
            let mut h = harness_in(Some(&fake), width, "vi");
            if page {
                account_page(&mut h);
            }
            h.run_steps(4);
            h.render().unwrap().save(dir.join(format!("{name}-{width}.png"))).unwrap();
        }
    }
}
