//! Native adapter for the optional PI ID account (`erpcons-account`). Installed only in builds made
//! with `ERPCONS_PI_ISSUER` (and optionally `ERPCONS_PI_API_BASE`); other builds get no hook, no thread
//! and never touch the credential store or the shared PI folder.
//!
//! One worker thread owns the blocking [`Account`] and runs the UI's commands in order; the UI reads
//! the last published snapshot, and the edit lock from the account's cached state. Only account
//! state crosses to the library: never a document, path or file name.

mod config;
mod licence;
mod resync;

use std::cell::OnceCell;
use std::sync::{
    Arc, Mutex, OnceLock, PoisonError,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
};
use std::time::{Duration, Instant};

use erpcons_account::{
    Account, ApiCode, Config, Error, LicenceState, MembershipStatus, OsSecretStore, RevocationReason, SecretStore, Status, WarningStage,
};
use wordcraft_ui_egui::account::{
    AccountError, AccountHook, AccountSnapshot, AccountView, EditLock, LicenceView, OrgView, SignedInView, WarningView, is_paid_feature,
    is_pro_feature,
};
use wordcraft_ui_egui::i18n;

use resync::Resync;

/// Network sync while the app runs (products.md §4: entitlements, lease refresh).
const REFRESH_EVERY: Duration = Duration::from_secs(3600);
/// Re-reads the shared session from disk, so signing in or out in another ERPCons app shows here (ADR 0010).
const RELOAD_EVERY: Duration = Duration::from_secs(60);

type Opener = Box<dyn Fn(&str) -> Result<(), String> + Send>;

/// The hook for `Services::account`; `None` unless this build has a PI issuer.
pub fn hook() -> Option<Box<dyn AccountHook>> {
    let config = config::from_build()?;
    Some(Box::new(DesktopAccount::new(config, Arc::new(OsSecretStore::new()), Box::new(open_browser))))
}

fn open_browser(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|error| error.to_string())
}

enum Command {
    SignIn,
    SignOut,
    SelectOrg(Option<String>),
    RetryActivation,
    WarningShown(WarningStage),
    Refresh,
    /// Wakes the worker for a refused edit; `Shared::resync_wanted` says it is still wanted.
    Resync,
}

/// What the worker and the UI thread share.
#[derive(Default)]
struct Shared {
    account: OnceLock<Arc<Account>>,
    /// The account could not be opened: editing stays locked.
    unavailable: AtomicBool,
    snapshot: Mutex<AccountSnapshot>,
    ctx: OnceLock<egui::Context>,
    /// Cancel pressed before the library started waiting; its own flag is reset when `sign_in` or
    /// `select_org` starts.
    cancelled: AtomicBool,
    /// An edit was refused for a reason a sync may lift; the worker syncs when its schedule allows.
    resync_wanted: AtomicBool,
}

impl Shared {
    fn publish(&self, update: impl FnOnce(&mut AccountSnapshot)) {
        update(&mut self.snapshot.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }
}

/// Not yet started: the worker starts on `attach`, once the interface language is known.
struct Pending {
    config: Config,
    store: Arc<dyn SecretStore>,
    opener: Opener,
    resync: resync::Timing,
}

pub struct DesktopAccount {
    shared: Arc<Shared>,
    pending: Mutex<Option<Pending>>,
    commands: OnceCell<Sender<Command>>,
}

impl DesktopAccount {
    /// `store` is the OS credential store in the app; tests pass a `MemorySecretStore`.
    pub fn new(config: Config, store: Arc<dyn SecretStore>, opener: Opener) -> Self {
        Self::with_resync(config, store, opener, resync::Timing::default())
    }

    fn with_resync(config: Config, store: Arc<dyn SecretStore>, opener: Opener, resync: resync::Timing) -> Self {
        let pending = Pending { config, store, opener, resync };
        Self { shared: Arc::default(), pending: Mutex::new(Some(pending)), commands: OnceCell::new() }
    }

    fn send(&self, command: Command) {
        match self.commands.get() {
            Some(tx) if tx.send(command).is_ok() => {}
            _ => self.shared.publish(|s| {
                if s.view == AccountView::SigningIn {
                    s.view = AccountView::Guest;
                }
                s.error = Some(AccountError::Other);
            }),
        }
    }

    fn start(&self) {
        let Some(mut pending) = self.pending.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
        let vietnamese = i18n::current().code() == "vi";
        pending.config.ui_locale = Some(if vietnamese { "vi" } else { "en" }.into());
        pending.config.callback_success_html = callback_page("Signed in to PI ID. You can close this tab and go back to the app.");
        pending.config.callback_failure_html = callback_page("Sign-in did not finish. Close this tab and try again in the app.");
        let (tx, rx) = mpsc::channel();
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new().name("pi-account".into()).spawn(move || Worker::run(shared, pending, rx));
        match spawned {
            Ok(_) => {
                let _ = self.commands.set(tx);
            }
            Err(error) => {
                log::warn!("PI account worker not started: {error}");
                self.shared.unavailable.store(true, Ordering::SeqCst);
                self.shared.publish(|s| s.error = Some(AccountError::Other));
            }
        }
    }
}

impl AccountHook for DesktopAccount {
    fn snapshot(&self) -> AccountSnapshot {
        self.shared.snapshot.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn sign_in(&self) {
        self.shared.cancelled.store(false, Ordering::SeqCst);
        self.shared.publish(|s| {
            s.view = AccountView::SigningIn;
            s.error = None;
        });
        self.send(Command::SignIn);
    }

    fn cancel_sign_in(&self) {
        // Not queued: the worker is blocked inside `sign_in` or `select_org` until this unblocks it.
        self.shared.cancelled.store(true, Ordering::SeqCst);
        if let Some(account) = self.shared.account.get() {
            account.cancel_sign_in();
        }
    }

    fn sign_out(&self) {
        self.send(Command::SignOut);
    }

    fn select_org(&self, org_id: Option<String>) {
        // PI switches organization through the browser (ADR 0011): the same wait and Cancel as sign-in.
        self.shared.cancelled.store(false, Ordering::SeqCst);
        self.shared.publish(|s| {
            s.view = AccountView::SigningIn;
            s.error = None;
        });
        self.send(Command::SelectOrg(org_id));
    }

    fn retry_activation(&self) {
        self.send(Command::RetryActivation);
    }

    fn allowed(&self, feature: &str) -> bool {
        let Some(account) = self.shared.account.get() else { return !is_pro_feature(feature) };
        let allowed = account.allowed(feature);
        if account.take_refresh_request()
            && let Some(tx) = self.commands.get()
        {
            let _ = tx.send(Command::Refresh);
        }
        allowed
    }

    fn warning_shown(&self, days: u32) {
        let stage = match days {
            7 => WarningStage::Days7,
            3 => WarningStage::Days3,
            1 => WarningStage::Days1,
            _ => return,
        };
        self.send(Command::WarningShown(stage));
    }

    fn attach(&self, ctx: &egui::Context) {
        let _ = self.shared.ctx.set(ctx.clone());
        self.start();
    }

    fn edit_lock(&self) -> Option<EditLock> {
        // Failing open would let anyone unlock editing by breaking the shared PI folder.
        if self.shared.unavailable.load(Ordering::SeqCst) {
            return Some(EditLock::Unavailable);
        }
        let Some(account) = self.shared.account.get() else { return Some(EditLock::Checking) };
        licence::edit_lock(account)
    }

    fn edit_refused(&self) {
        // The worker rate-limits syncs; only the first refusal sends, so refusals never pile up
        // in the queue.
        if self.edit_lock().is_some_and(EditLock::sync_may_lift)
            && !self.shared.resync_wanted.swap(true, Ordering::SeqCst)
            && let Some(tx) = self.commands.get()
        {
            let _ = tx.send(Command::Resync);
        }
    }
}

fn callback_page(key: &str) -> String {
    let text = i18n::t(key).replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("<!doctype html><meta charset=\"utf-8\"><title>PI ID</title><p style=\"font:16px sans-serif;margin:3em\">{text}</p>")
}

struct Worker {
    shared: Arc<Shared>,
    account: Arc<Account>,
    opener: Opener,
    next_refresh: Instant,
    next_reload: Instant,
    resync: Resync,
}

impl Worker {
    fn run(shared: Arc<Shared>, pending: Pending, rx: Receiver<Command>) {
        let Pending { config, store, opener, resync } = pending;
        let account = match Account::new(config, store) {
            Ok(account) => Arc::new(account),
            Err(error) => {
                log::warn!("PI account unavailable: {error}");
                shared.unavailable.store(true, Ordering::SeqCst);
                shared.publish(|s| *s = AccountSnapshot { error: Some(AccountError::Other), ..AccountSnapshot::default() });
                // Answer every later command with the same error instead of leaving the UI waiting.
                while rx.recv().is_ok() {
                    shared.publish(|s| *s = AccountSnapshot { error: Some(AccountError::Other), ..AccountSnapshot::default() });
                }
                return;
            }
        };
        let _ = shared.account.set(account.clone());
        let now = Instant::now();
        let resync = Resync::new(resync, now);
        let mut worker = Worker { shared, account, opener, next_refresh: now + REFRESH_EVERY, next_reload: now + RELOAD_EVERY, resync };
        if let Err(error) = worker.account.reload() {
            log::warn!("PI account state not read: {error}");
        }
        worker.publish(None, false);
        if signed_in(&worker.account.status()) {
            worker.refresh();
        }
        loop {
            let mut due = worker.next_refresh.min(worker.next_reload);
            if worker.shared.resync_wanted.load(Ordering::SeqCst) {
                due = due.min(worker.resync.not_before);
            }
            match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(command) => worker.handle(command),
                Err(RecvTimeoutError::Timeout) => worker.tick(),
                Err(RecvTimeoutError::Disconnected) => return,
            }
            worker.sync_if_asked();
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if now >= self.next_refresh && signed_in(&self.account.status()) {
            self.refresh();
        } else if now >= self.next_refresh {
            self.next_refresh = now + REFRESH_EVERY;
        }
        if now >= self.next_reload {
            self.next_reload = now + RELOAD_EVERY;
            if let Err(error) = self.account.reload() {
                log::warn!("PI account state not read: {error}");
            }
            // Also repaints, so a lease that ran out while the app sat idle locks editing.
            self.publish_status();
        }
    }

    /// A refresh `allowed()` asked for (stale entitlements), or one for a refused edit once the
    /// schedule allows it.
    fn sync_if_asked(&mut self) {
        let asked = self.account.take_refresh_request();
        let resync_due = self.shared.resync_wanted.load(Ordering::SeqCst) && Instant::now() >= self.resync.not_before;
        if !(asked || resync_due) {
            return;
        }
        if signed_in(&self.account.status()) {
            self.refresh();
        } else {
            self.shared.resync_wanted.store(false, Ordering::SeqCst);
        }
    }

    /// Background sync: only failures the user can act on are shown; going offline shows as Offline.
    fn refresh(&mut self) {
        // Cleared first: an edit refused while this runs asks again for the next slot.
        self.shared.resync_wanted.store(false, Ordering::SeqCst);
        let result = self.account.refresh();
        self.after_sync(&result);
        let wait = result.as_ref().err().and_then(Error::retry_after).unwrap_or_default();
        self.next_refresh = Instant::now() + REFRESH_EVERY.max(wait);
        match result {
            Ok(()) => self.publish(None, false),
            Err(error) if error.is_transient() => self.publish_status(),
            Err(error) => {
                log::warn!("PI account refresh failed: {error}");
                self.publish(error_view(&error), false);
            }
        }
    }

    /// Schedules the next sync a refused edit may ask for.
    fn after_sync(&mut self, result: &Result<(), Error>) {
        let still_locked = licence::edit_lock(&self.account).is_some_and(EditLock::sync_may_lift);
        self.resync.after(Instant::now(), result, still_locked);
    }

    fn handle(&mut self, command: Command) {
        let result = match command {
            Command::Refresh => return self.refresh(),
            // `sync_if_asked` runs it when the schedule allows.
            Command::Resync => return,
            Command::RetryActivation => return self.retry(),
            Command::WarningShown(stage) => {
                if let Err(error) = self.account.mark_warning_shown(stage) {
                    log::warn!("PI expiry warning not recorded: {error}");
                }
                return self.publish_status();
            }
            Command::SignIn => self.browser_flow(|account, open| account.sign_in(open)),
            Command::SelectOrg(org) => self.browser_flow(|account, open| account.select_org(org.as_deref(), open)),
            Command::SignOut => {
                self.shared.publish(|s| s.busy = true);
                self.account.sign_out()
            }
        };
        if let Err(error) = &result {
            log::warn!("PI account: {error}");
        }
        self.publish(result.err().as_ref().and_then(error_view), false);
    }

    /// File › Account's Retry: sync now, then activate again where PI refused it (device cap, no
    /// plan), which a plain sync never retries (lease.md §6).
    fn retry(&mut self) {
        self.shared.publish(|s| s.busy = true);
        self.shared.resync_wanted.store(false, Ordering::SeqCst);
        self.resync.reset();
        let mut result = self.account.refresh();
        let refused = matches!(self.account.status(), Status::SignedIn(s)
            if matches!(s.licence, LicenceState::DeviceCapReached | LicenceState::NotEntitled));
        if refused {
            result = self.account.retry_activation();
        }
        self.after_sync(&result);
        if let Err(error) = &result {
            log::warn!("PI account retry: {error}");
        }
        self.publish(result.err().as_ref().and_then(error_view), false);
    }

    /// Sign-in or organization switch: both block on the browser until its callback or Cancel.
    fn browser_flow(&mut self, run: impl FnOnce(&Account, &dyn Fn(&str) -> Result<(), String>) -> Result<(), Error>) -> Result<(), Error> {
        // A background reload may have replaced the wait the UI showed before this command arrived.
        self.shared.publish(|s| s.view = AccountView::SigningIn);
        let (shared, opener) = (&self.shared, &self.opener);
        let account: &Account = &self.account;
        let open = |url: &str| {
            if shared.cancelled.load(Ordering::SeqCst) {
                account.cancel_sign_in();
                return Ok(());
            }
            opener(url)
        };
        let result = run(account, &open);
        if result.is_ok() {
            self.next_refresh = Instant::now() + REFRESH_EVERY;
        }
        result
    }

    fn publish(&self, error: Option<AccountError>, busy: bool) {
        let view = view_of(&self.account.status());
        self.shared.publish(|s| *s = AccountSnapshot { view, busy, error });
    }

    /// The account's current state, keeping the error already shown.
    fn publish_status(&self) {
        let view = view_of(&self.account.status());
        self.shared.publish(|s| {
            s.view = view;
            s.busy = false;
        });
    }
}

fn signed_in(status: &Status) -> bool {
    matches!(status, Status::SignedIn(_))
}

fn view_of(status: &Status) -> AccountView {
    let s = match status {
        Status::Guest => return AccountView::Guest,
        Status::SigningIn => return AccountView::SigningIn,
        Status::SignedIn(s) => s,
    };
    let (name, email) = s.user.as_ref().map(|u| (u.display_name.clone(), u.email.clone())).unwrap_or_default();
    AccountView::SignedIn(SignedInView {
        name,
        email,
        orgs: s
            .orgs
            .iter()
            .filter(|m| m.status == MembershipStatus::Active)
            .map(|m| OrgView { id: m.org_id.clone(), name: m.name.clone(), role: Some(m.role.clone()).filter(|r| !r.is_empty()) })
            .collect(),
        active_org: s.active_org.clone(),
        licence: licence_of(&s.licence),
        warning: s.warning.map(|w| WarningView { days: w.stage.days(), expires_at: w.expires_at }),
        online: s.online,
        session_only: s.session_only,
        reauth_required: s.reauth_required,
    })
}

fn licence_of(licence: &LicenceState) -> LicenceView {
    match licence {
        // No lease: the plan reads Free, and the edit lock says why editing is off.
        LicenceState::NotActivated | LicenceState::NotEntitled => LicenceView::Free,
        // The free plan's lease grants `erpconsoffice.use`; only features beyond it are paid.
        LicenceState::Active { features, expires_at, .. } => {
            LicenceView::Active { expires_at: *expires_at, pro: features.iter().any(|f| is_paid_feature(f)) }
        }
        LicenceState::Inactive { expires_at, .. } => LicenceView::Inactive { expires_at: *expires_at },
        LicenceState::DeviceRevoked => LicenceView::DeviceRevoked,
        LicenceState::DeviceCapReached => LicenceView::DeviceCapReached,
        LicenceState::SessionOnly => LicenceView::SessionOnly,
    }
}

/// The error line for a failed call; `None` when the licence summary already says it (activation
/// blocked, no credential store).
fn error_view(error: &Error) -> Option<AccountError> {
    Some(match error {
        Error::Cancelled => AccountError::Cancelled,
        Error::Network(_) => AccountError::Network,
        Error::Timeout | Error::SignInTimedOut => AccountError::Timeout,
        Error::AuthorizationDenied(_) => AccountError::Denied,
        Error::InvalidGrant | Error::NotSignedIn => AccountError::ReauthRequired,
        Error::UnknownOrg | Error::OrgUnavailable => AccountError::OrgUnavailable,
        Error::Revoked(RevocationReason::DeviceRevoked) => AccountError::DeviceRevoked,
        Error::Revoked(RevocationReason::MembershipRevoked) => AccountError::OrgUnavailable,
        Error::Revoked(RevocationReason::UserSuspended) => AccountError::Suspended,
        Error::ActivationBlocked | Error::CredentialStoreUnavailable => return None,
        Error::Api(api) => match api.code {
            ApiCode::RateLimited => AccountError::RateLimited { retry_after_secs: api.retry_after.map(|d| d.as_secs()) },
            ApiCode::DependencyUnavailable => AccountError::ServiceUnavailable,
            ApiCode::DeviceCapReached => AccountError::DeviceCap,
            ApiCode::DeviceRevoked => AccountError::DeviceRevoked,
            ApiCode::UserSuspended => AccountError::Suspended,
            ApiCode::MembershipInactive => AccountError::OrgUnavailable,
            ApiCode::Unauthenticated | ApiCode::InsufficientScope => AccountError::ReauthRequired,
            ApiCode::ClientVersionUnsupported => AccountError::ClientTooOld,
            ApiCode::Forbidden | ApiCode::QuotaExceeded => AccountError::Denied,
            _ => AccountError::Other,
        },
        _ => AccountError::Other,
    })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_licence;
