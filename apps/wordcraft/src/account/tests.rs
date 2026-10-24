use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;

use erpcons_account::{ApiError, ExpiryWarning, Membership, MemorySecretStore, PinnedKey, SignedIn, User};
use pi_mock::{ORG_A, Scenario, Server};

use super::*;

fn membership(id: &str, status: MembershipStatus) -> Membership {
    Membership { org_id: id.into(), name: format!("Org {id}"), role: "member".into(), status }
}

fn signed_in_status(licence: LicenceState) -> Status {
    Status::SignedIn(Box::new(SignedIn {
        user: Some(User { user_id: "usr_1".into(), display_name: "Nguyễn An".into(), email: Some("an@example.com".into()) }),
        orgs: vec![membership("org_a", MembershipStatus::Active), membership("org_s", MembershipStatus::Suspended)],
        active_org: Some("org_a".into()),
        licence,
        licences: BTreeMap::new(),
        warning: Some(ExpiryWarning { stage: WarningStage::Days3, expires_at: 1_800_000_000 }),
        online: false,
        session_only: false,
        reauth_required: true,
    }))
}

#[test]
fn status_maps_to_the_view() {
    assert_eq!(view_of(&Status::Guest), AccountView::Guest);
    assert_eq!(view_of(&Status::SigningIn), AccountView::SigningIn);
    let licence = LicenceState::Active { lease_id: "l".into(), features: vec!["vanban.view".into(), "vanban.use".into()], expires_at: 1_800_000_000 };
    let AccountView::SignedIn(view) = view_of(&signed_in_status(licence)) else { panic!("signed in") };
    assert_eq!(view.name, "Nguyễn An");
    assert_eq!(view.email.as_deref(), Some("an@example.com"));
    assert_eq!(view.orgs, vec![OrgView { id: "org_a".into(), name: "Org org_a".into(), role: Some("member".into()) }], "only active memberships");
    assert_eq!(view.active_org.as_deref(), Some("org_a"));
    assert_eq!(view.licence, LicenceView::Active { expires_at: 1_800_000_000, pro: false });
    assert_eq!(view.warning, Some(WarningView { days: 3, expires_at: 1_800_000_000 }));
    assert!(!view.online && view.reauth_required && !view.session_only);
    let mut no_user = signed_in_status(LicenceState::NotActivated);
    if let Status::SignedIn(s) = &mut no_user {
        s.user = None;
    }
    let AccountView::SignedIn(view) = view_of(&no_user) else { panic!("signed in") };
    assert_eq!((view.name.as_str(), view.email), ("", None), "before /v1/me answered");
}

#[test]
fn licence_states_map_to_the_view() {
    let active =
        |features: &[&str]| LicenceState::Active { lease_id: "l".into(), features: features.iter().map(|f| f.to_string()).collect(), expires_at: 5 };
    assert_eq!(licence_of(&LicenceState::NotActivated), LicenceView::Free);
    assert_eq!(licence_of(&active(&["vanban.view", "vanban.export"])), LicenceView::Active { expires_at: 5, pro: false });
    assert_eq!(licence_of(&active(&["vanban.view", "vanban.advanced"])), LicenceView::Active { expires_at: 5, pro: true });
    assert_eq!(licence_of(&LicenceState::Inactive { lease_id: "l".into(), expires_at: 9 }), LicenceView::Inactive { expires_at: 9 });
    assert_eq!(licence_of(&LicenceState::DeviceRevoked), LicenceView::DeviceRevoked);
    assert_eq!(licence_of(&LicenceState::DeviceCapReached), LicenceView::DeviceCapReached);
    assert_eq!(licence_of(&LicenceState::SessionOnly), LicenceView::SessionOnly);
}

#[test]
fn library_errors_map_to_the_error_line() {
    let api = |code: ApiCode, retry: Option<u64>| {
        Error::Api(ApiError {
            status: 400,
            code,
            correlation_id: None,
            retry_after: retry.map(Duration::from_secs),
            max_devices: None,
            active_devices: None,
        })
    };
    let cases = [
        (Error::Cancelled, Some(AccountError::Cancelled)),
        (Error::Network("down".into()), Some(AccountError::Network)),
        (Error::SignInTimedOut, Some(AccountError::Timeout)),
        (Error::Timeout, Some(AccountError::Timeout)),
        (Error::AuthorizationDenied("access_denied".into()), Some(AccountError::Denied)),
        (Error::InvalidGrant, Some(AccountError::ReauthRequired)),
        (Error::OrgUnavailable, Some(AccountError::OrgUnavailable)),
        (Error::UnknownOrg, Some(AccountError::OrgUnavailable)),
        (Error::Revoked(RevocationReason::DeviceRevoked), Some(AccountError::DeviceRevoked)),
        (Error::Revoked(RevocationReason::UserSuspended), Some(AccountError::Suspended)),
        (Error::ActivationBlocked, None),
        (Error::CredentialStoreUnavailable, None),
        (api(ApiCode::RateLimited, Some(120)), Some(AccountError::RateLimited { retry_after_secs: Some(120) })),
        (api(ApiCode::RateLimited, None), Some(AccountError::RateLimited { retry_after_secs: None })),
        (api(ApiCode::DependencyUnavailable, None), Some(AccountError::ServiceUnavailable)),
        (api(ApiCode::DeviceCapReached, None), Some(AccountError::DeviceCap)),
        (api(ApiCode::DeviceRevoked, None), Some(AccountError::DeviceRevoked)),
        (api(ApiCode::UserSuspended, None), Some(AccountError::Suspended)),
        (api(ApiCode::MembershipInactive, None), Some(AccountError::OrgUnavailable)),
        (api(ApiCode::Unauthenticated, None), Some(AccountError::ReauthRequired)),
        (api(ApiCode::ClientVersionUnsupported, None), Some(AccountError::ClientTooOld)),
        (api(ApiCode::Forbidden, None), Some(AccountError::Denied)),
        (api(ApiCode::QuotaExceeded, None), Some(AccountError::Denied)),
        (api(ApiCode::ValidationFailed, None), Some(AccountError::Other)),
        (Error::Lease(erpcons_account::VerifyError::Signature), Some(AccountError::Other)),
    ];
    for (error, expected) in cases {
        assert_eq!(error_view(&error), expected, "{error}");
    }
}

#[test]
fn the_browser_pages_are_translated_and_escaped() {
    let previous = i18n::current();
    i18n::set_current(i18n::Lang::from_pref("vi"));
    assert!(callback_page("Signed in to PI ID. You can close this tab and go back to the app.").contains("Đã đăng nhập PI ID."));
    i18n::set_current(i18n::Lang::EN);
    assert!(callback_page("a <b> & c").contains("a &lt;b&gt; &amp; c"));
    i18n::set_current(previous);
}

/// The system browser in tests: follows the authorization redirect to the app's loopback listener.
fn follow_redirect(url: &str) -> Result<(), String> {
    let url = url.to_owned();
    std::thread::spawn(move || {
        if let Some(location) = http_get(&url).and_then(|response| location_of(&response)) {
            let _ = http_get(&location);
        }
    });
    Ok(())
}

fn http_get(url: &str) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').map_or((rest, String::from("/")), |(host, path)| (host, format!("/{path}")));
    let mut stream = TcpStream::connect(host).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn location_of(response: &str) -> Option<String> {
    response.lines().take_while(|line| !line.is_empty()).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("location").then(|| value.trim().to_owned())
    })
}

fn mock_config(server: &Server, dir: &tempfile::TempDir) -> Config {
    let mut config = Config::new(server.issuer(), Product::VanBan, "0.6.5");
    // The mock's development key is pinned here only, never in a build.
    config.platform_keys = vec![PinnedKey { kid: server.lease_key_id(), key: server.lease_public_key() }];
    config.state_dir = Some(dir.path().to_path_buf());
    config.timeout = Duration::from_secs(5);
    config.max_retry_wait = Duration::from_secs(3);
    config.sign_in_timeout = Duration::from_secs(20);
    config
}

fn wait_for(hook: &DesktopAccount, what: &str, done: impl Fn(&AccountSnapshot) -> bool) -> AccountSnapshot {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = hook.snapshot();
        if done(&snapshot) && !snapshot.busy {
            return snapshot;
        }
        assert!(Instant::now() < until, "timed out waiting for {what}: {snapshot:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn started(server: &Server, dir: &tempfile::TempDir, opener: Opener) -> DesktopAccount {
    let hook = DesktopAccount::new(mock_config(server, dir), Arc::new(MemorySecretStore::new()), opener);
    hook.attach(&egui::Context::default());
    let until = Instant::now() + Duration::from_secs(10);
    while hook.shared.account.get().is_none() {
        assert!(Instant::now() < until, "worker did not open the account");
        std::thread::sleep(Duration::from_millis(10));
    }
    hook
}

#[test]
fn signs_in_switches_org_and_signs_out_against_pi_mock() {
    let server = Server::start(Scenario::default());
    let dir = tempfile::tempdir().unwrap();
    let hook = started(&server, &dir, Box::new(follow_redirect));
    assert_eq!(wait_for(&hook, "guest", |s| s.view == AccountView::Guest).error, None);

    hook.sign_in();
    let snapshot = wait_for(&hook, "sign-in", |s| matches!(&s.view, AccountView::SignedIn(v) if v.orgs.len() == 2));
    let AccountView::SignedIn(view) = snapshot.view else { panic!("signed in") };
    assert_eq!(snapshot.error, None);
    assert_eq!(view.email.as_deref(), Some("an.nguyen@example.com"));
    assert!(!view.name.is_empty());
    assert_eq!(view.active_org, None);
    assert!(matches!(view.licence, LicenceView::Active { pro: false, .. }), "{:?}", view.licence);
    assert!(view.online);
    assert!(hook.allowed("vanban.use"));

    hook.select_org(Some(ORG_A.into()));
    wait_for(&hook, "org switch", |s| matches!(&s.view, AccountView::SignedIn(v) if v.active_org.as_deref() == Some(ORG_A)));
    hook.select_org(None);
    wait_for(&hook, "personal context", |s| matches!(&s.view, AccountView::SignedIn(v) if v.active_org.is_none()));

    hook.sign_out();
    let snapshot = wait_for(&hook, "sign-out", |s| s.view == AccountView::Guest);
    assert_eq!(snapshot.error, None);

    let dir = dir.path().to_string_lossy().into_owned();
    for request in server.requests() {
        assert!(!request.body.contains(&dir) && !request.query.contains(&dir), "{} {}", request.method, request.path);
    }
}

#[test]
fn cancel_unblocks_a_sign_in_waiting_for_the_browser() {
    let server = Server::start(Scenario::default());
    let dir = tempfile::tempdir().unwrap();
    let hook = started(&server, &dir, Box::new(|_| Ok(())));
    hook.sign_in();
    assert_eq!(hook.snapshot().view, AccountView::SigningIn, "shown at once");
    std::thread::sleep(Duration::from_millis(200));
    hook.cancel_sign_in();
    let snapshot = wait_for(&hook, "cancel", |s| s.view == AccountView::Guest);
    assert_eq!(snapshot.error, Some(AccountError::Cancelled));
    assert_eq!(wordcraft_ui_egui::account::error_text(AccountError::Cancelled), None, "shown as nothing");
}
