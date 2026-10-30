//! The desktop edit lock against pi-mock: guests and accounts without a usable Office lease view,
//! save and export but cannot edit; a lease unlocks editing in the open document.

use std::sync::atomic::{AtomicI64, AtomicU64};

use erpcons_account::{Clock, MemorySecretStore, OFFICE_USE, PinnedKey};
use pi_mock::{ORG_A, ORG_OFFICE, Scenario, Server, Tenant};
use serde_json::json;
use wordcraft_ui_egui::{Services, WordApp};

use super::tests::follow_redirect;
use super::*;

const T0: i64 = 1_792_828_800;
const DAY: i64 = 86_400;

struct TestClock {
    unix: AtomicI64,
    mono_ms: AtomicU64,
}

impl Clock for TestClock {
    fn unix_now(&self) -> i64 {
        self.unix.load(Ordering::SeqCst)
    }
    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.mono_ms.load(Ordering::SeqCst))
    }
}

struct Env {
    server: Option<Server>,
    issuer: String,
    lease_key: PinnedKey,
    dir: tempfile::TempDir,
    clock: Arc<TestClock>,
}

impl Env {
    fn new(mut scenario: Scenario) -> Self {
        scenario.now.get_or_insert(T0);
        let server = Server::start(scenario);
        Self {
            issuer: server.issuer().to_owned(),
            lease_key: PinnedKey { kid: server.lease_key_id(), key: server.lease_public_key() },
            server: Some(server),
            dir: tempfile::tempdir().unwrap(),
            clock: Arc::new(TestClock { unix: AtomicI64::new(T0), mono_ms: AtomicU64::new(1_000_000) }),
        }
    }

    fn server(&self) -> &Server {
        self.server.as_ref().expect("the server is up")
    }

    fn config(&self) -> Config {
        let mut config = config::for_issuer(&self.issuer, "0.6.5");
        // The mock's development key is pinned here only, never in a build.
        config.platform_keys = vec![self.lease_key.clone()];
        config.state_dir = Some(self.dir.path().to_path_buf());
        config.clock = self.clock.clone();
        config.timeout = Duration::from_secs(2);
        config.max_retry_wait = Duration::from_secs(3);
        config.sign_in_timeout = Duration::from_secs(20);
        config
    }

    /// The app's adapter with the test browser, an in-memory credential store and a resync
    /// schedule of milliseconds.
    fn hook(&self) -> DesktopAccount {
        let timing = resync::Timing { min: Duration::from_millis(20), max: Duration::from_millis(50) };
        let hook = DesktopAccount::with_resync(self.config(), Arc::new(MemorySecretStore::new()), Box::new(follow_redirect), timing);
        hook.attach(&egui::Context::default());
        hook
    }

    /// Moves the client's clocks and PI's time together.
    fn advance(&self, secs: i64) {
        self.clock.unix.fetch_add(secs, Ordering::SeqCst);
        self.clock.mono_ms.fetch_add(secs.unsigned_abs() * 1000, Ordering::SeqCst);
        let now = self.clock.unix_now();
        if let Some(server) = &self.server {
            server.update_scenario(|s| s.now = Some(now));
        }
    }
}

fn wait_for_lock(hook: &DesktopAccount, what: &str, expected: Option<EditLock>) {
    let until = Instant::now() + Duration::from_secs(30);
    while hook.edit_lock() != expected || hook.snapshot().busy {
        assert!(Instant::now() < until, "timed out waiting for {what}: {:?}", hook.edit_lock());
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn signed_in_view(hook: &DesktopAccount) -> SignedInView {
    match hook.snapshot().view {
        AccountView::SignedIn(view) => view,
        other => panic!("not signed in: {other:?}"),
    }
}

/// A user whose organizations are A (Office free plan) and B (none); PI's page picks personal.
fn two_orgs() -> Scenario {
    Scenario { tenants: vec![Tenant::office_free("ten_office", ORG_A)], ..Scenario::default() }
}

#[test]
fn a_guest_is_locked_until_sign_in_brings_a_lease() {
    let env = Env::new(Scenario::office_free());
    let hook = env.hook();
    wait_for_lock(&hook, "the guest lock", Some(EditLock::SignIn));
    assert!(!hook.allowed(OFFICE_USE));
    hook.sign_in();
    wait_for_lock(&hook, "the lease", None);
    assert!(hook.allowed(OFFICE_USE));
    let view = signed_in_view(&hook);
    assert_eq!(view.active_org.as_deref(), Some(ORG_OFFICE), "the free organization");
    assert!(matches!(view.licence, LicenceView::Active { pro: false, .. }), "the free plan is no paid plan: {:?}", view.licence);
    assert_eq!(env.server().device_rows().len(), 1, "this computer activated once");
}

/// The whole app: a guest views, saves and exports the open document but cannot edit it; signing
/// in unlocks it in place, without reopening.
#[test]
fn the_app_saves_and_exports_while_locked_and_edits_once_licensed() {
    let env = Env::new(Scenario::office_free());
    let services = Services { account: Some(Box::new(env.hook())), ..Default::default() };
    let mut app = WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::from_text("Báo cáo")), services);
    let ctx = egui::Context::default();
    let frame = |app: &mut WordApp| {
        let mut out = ctx.run_ui(Default::default(), |ui| app.logic(ui.ctx()));
        out.textures_delta.clear();
    };
    let until = Instant::now() + Duration::from_secs(30);
    while app.edit_lock() != Some(EditLock::SignIn) {
        assert!(Instant::now() < until, "the guest lock: {:?}", app.edit_lock());
        frame(&mut app);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(app.read_only());
    let refused = app.run("text.insert", json!({"text": "x"})).unwrap_err();
    assert_eq!(refused, i18n::t(EditLock::SignIn.reason()), "the status bar says why");
    assert!(app.run("edit.undo", json!({})).is_err());
    let docx = env.dir.path().join("locked.docx");
    app.run("file.save", json!({"path": docx.to_string_lossy()})).unwrap();
    assert!(docx.is_file(), "Save still writes the document");
    let pdf = env.dir.path().join("locked.pdf");
    app.run("file.exportPdf", json!({"path": pdf.to_string_lossy()})).unwrap();
    assert!(pdf.is_file(), "Export still works");
    app.run("select.all", json!({})).unwrap();
    app.run("edit.copy", json!({})).unwrap();

    app.services.account.as_ref().unwrap().sign_in();
    let until = Instant::now() + Duration::from_secs(30);
    while app.edit_lock().is_some() {
        assert!(Instant::now() < until, "the lease: {:?}", app.edit_lock());
        frame(&mut app);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!app.read_only());
    app.run("caret.docEnd", json!({})).unwrap();
    app.run("text.insert", json!({"text": " đã sửa"})).unwrap();
    assert!(app.session.doc.plain_text(wordcraft_doc::StoryRef::Body).contains("Báo cáo đã sửa"), "the same document, now editable");
}

#[test]
fn signing_out_and_a_revoked_device_lock_editing_again() {
    let env = Env::new(Scenario::office_free());
    let hook = env.hook();
    hook.sign_in();
    wait_for_lock(&hook, "the lease", None);
    let device = env.server().device_rows().first().map(|d| d.device_id.clone()).unwrap();
    assert!(env.server().revoke_device(&device));
    // Past the renewal point of a 7-day lease, so the next sync asks PI.
    env.advance(13 * 3600);
    hook.send(Command::Refresh);
    wait_for_lock(&hook, "the revoked device", Some(EditLock::Revoked));
    assert!(!hook.allowed(OFFICE_USE));
    hook.sign_out();
    wait_for_lock(&hook, "the sign-out", Some(EditLock::SignIn));
}

#[test]
fn an_expired_lease_locks_with_renewal_failed_online_and_connect_offline() {
    let mut env = Env::new(Scenario::office_free());
    let hook = env.hook();
    hook.sign_in();
    wait_for_lock(&hook, "the lease", None);
    env.advance(8 * DAY);
    // Nothing synced since: still online, so the renewal has not run yet.
    wait_for_lock(&hook, "the expired lease", Some(EditLock::RenewFailed));
    env.server = None;
    hook.send(Command::Refresh);
    wait_for_lock(&hook, "the offline lock", Some(EditLock::RenewOffline));
}

#[test]
fn a_refused_edit_resyncs_and_a_renewed_lease_unlocks() {
    let env = Env::new(Scenario::office_free());
    let hook = env.hook();
    hook.sign_in();
    wait_for_lock(&hook, "the lease", None);
    env.advance(8 * DAY);
    wait_for_lock(&hook, "the expired lease", Some(EditLock::RenewFailed));
    hook.edit_refused();
    wait_for_lock(&hook, "the renewed lease", None);
}

#[test]
fn the_personal_context_asks_for_an_organization() {
    let env = Env::new(two_orgs());
    let hook = env.hook();
    hook.sign_in();
    wait_for_lock(&hook, "the personal context", Some(EditLock::ChooseOrg));
    assert_eq!(signed_in_view(&hook).active_org, None);
    hook.select_org(Some(ORG_A.into()));
    wait_for_lock(&hook, "the organization's lease", None);
}

#[test]
fn retry_activates_once_the_organization_has_a_plan() {
    let env = Env::new(Scenario { tenants: Vec::new(), ..Scenario::office_free() });
    let hook = env.hook();
    hook.sign_in();
    wait_for_lock(&hook, "no plan", Some(EditLock::NoPlan));
    assert!(EditLock::NoPlan.retry_offered());
    // PI gives the organization Office. Its free plan reports no seat before an activation, so
    // only the user's Retry activates (ADR 0013).
    env.server().update_scenario(|s| s.tenants = Scenario::office_free().tenants);
    hook.retry_activation();
    wait_for_lock(&hook, "the lease after Retry", None);
    assert_eq!(hook.snapshot().error, None);
}

#[test]
fn an_account_that_cannot_open_keeps_editing_locked() {
    let env = Env::new(Scenario::office_free());
    let mut config = env.config();
    // Any failure of `Account::new` will do; the library refuses a client id not the profile's.
    config.client_id = "not-office".into();
    let hook = DesktopAccount::new(config, Arc::new(MemorySecretStore::new()), Box::new(follow_redirect));
    assert_eq!(hook.edit_lock(), Some(EditLock::Checking), "before the worker read the account");
    hook.attach(&egui::Context::default());
    let until = Instant::now() + Duration::from_secs(10);
    while hook.edit_lock() == Some(EditLock::Checking) {
        assert!(Instant::now() < until, "the account was never opened");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(hook.edit_lock(), Some(EditLock::Unavailable));
}

#[cfg(debug_assertions)]
#[test]
fn a_development_lease_key_parses_only_as_kid_and_64_hex_digits() {
    let hex = "00".repeat(31) + "ff";
    let key = config::parse_dev_lease_key(&format!("pi-mock-dev-1:{hex}")).unwrap();
    assert_eq!((key.kid.as_str(), key.key[31]), ("pi-mock-dev-1", 0xff));
    for bad in ["", "kid", ":00", &format!("kid:{}", "0".repeat(63)), &format!("kid:+{}", "0".repeat(63)), &format!("kid:{}", "g".repeat(64))] {
        assert!(config::parse_dev_lease_key(bad).is_none(), "{bad}");
    }
}

#[test]
fn the_build_turns_licensing_on() {
    assert!(config::for_issuer("https://pi.example", "1.0.0").offline_licensing);
}
