//! Optional PI ID account (ERPCons): File › Account, the title bar's account button, the paid
//! licence expiry notice and, on desktop, the edit lock of an unlicensed app. The host installs an
//! [`AccountHook`] (desktop: `erpcons-account` on a worker thread; web: the office shell's
//! `window.erpAccount`). Without one nothing here shows and every feature is allowed.

mod lock;
mod page;

pub use lock::EditLock;
pub use page::{date_text, error_text};
pub(crate) use page::{lock_notice, page, title_button, warning_notice};

use crate::WordApp;

/// The Backstage page id of File › Account.
pub const ACCOUNT_PAGE: &str = "account";

/// Implemented by the native and web adapters. Every method returns at once; work runs on the adapter's own
/// worker (thread on desktop, promise on web) and the result shows up in the next `snapshot()`.
pub trait AccountHook {
    fn snapshot(&self) -> AccountSnapshot;
    fn sign_in(&self);
    fn cancel_sign_in(&self);
    fn sign_out(&self);
    fn select_org(&self, org_id: Option<String>);
    /// File › Account's Retry: sync now and activate again where PI refused.
    fn retry_activation(&self);
    fn allowed(&self, feature: &str) -> bool;
    fn warning_shown(&self, days: u32);
    /// Called once with the egui context so the adapter can request a repaint when its snapshot changes.
    fn attach(&self, ctx: &egui::Context);
    /// Why editing is locked now, `None` when it isn't. Asked every frame, so it answers from cached
    /// state. Only the desktop adapter locks; the web keeps its free tier.
    fn edit_lock(&self) -> Option<EditLock> {
        None
    }
    /// An edit was just refused by [`AccountHook::edit_lock`]; the adapter may sync with PI, at its
    /// own pace, in case that lifts it.
    fn edit_refused(&self) {}
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub view: AccountView,
    /// A command the user started is still running.
    pub busy: bool,
    pub error: Option<AccountError>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AccountView {
    #[default]
    Guest,
    SigningIn,
    SignedIn(SignedInView),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedInView {
    pub name: String,
    pub email: Option<String>,
    pub orgs: Vec<OrgView>,
    /// `None` is the personal context.
    pub active_org: Option<String>,
    pub licence: LicenceView,
    pub warning: Option<WarningView>,
    pub online: bool,
    pub session_only: bool,
    pub reauth_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrgView {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LicenceView {
    /// No non-free entitlement (normal today).
    Free,
    /// Desktop lease.
    Active {
        expires_at: i64,
        pro: bool,
    },
    /// Web: no lease, entitlements read online.
    Online {
        pro: bool,
    },
    Inactive {
        expires_at: i64,
    },
    DeviceRevoked,
    DeviceCapReached,
    SessionOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WarningView {
    pub days: u32,
    pub expires_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountError {
    Cancelled,
    PopupBlocked,
    Network,
    Timeout,
    Denied,
    RateLimited { retry_after_secs: Option<u64> },
    DeviceCap,
    DeviceRevoked,
    Suspended,
    OrgUnavailable,
    ReauthRequired,
    ClientTooOld,
    ServiceUnavailable,
    Other,
}

/// Whether `feature` needs a lease or an entitlement: everything but the pinned free `.view` and
/// `.export` (products.md §2). `erpconsoffice.use` (editing) is one since erpcons-account 0.5.0.
pub fn is_pro_feature(feature: &str) -> bool {
    !(feature.ends_with(".view") || feature.ends_with(".export"))
}

/// Whether holding `feature` means a paid plan: a pro feature other than `.use`, which the free
/// plan grants.
pub fn is_paid_feature(feature: &str) -> bool {
    is_pro_feature(feature) && feature.split_once('.').is_none_or(|(_, name)| name != "use")
}

/// The avatar letter: the given name's first letter (Vietnamese names put it last, as the office
/// shell does), else the email's.
pub fn avatar_letter(name: &str, email: Option<&str>) -> Option<String> {
    let given = name.split_whitespace().next_back().unwrap_or_default();
    given.chars().chain(email.unwrap_or_default().chars()).find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect())
}

/// What the app keeps between frames about the account.
#[derive(Default)]
pub(crate) struct AccountState {
    attached: bool,
    /// The expiry warning closed in this run; hidden even before the adapter's snapshot drops it.
    dismissed: Option<WarningView>,
    /// The edit lock applied to the documents (`Session::edit_lock`).
    lock: Option<EditLock>,
    /// The lock notice closed in this run; it comes back when the reason changes.
    lock_dismissed: Option<EditLock>,
}

impl WordApp {
    /// Whether `feature` may be used. True without an account hook.
    pub fn allowed(&self, feature: &str) -> bool {
        self.services.account.as_ref().is_none_or(|hook| hook.allowed(feature))
    }

    /// Why editing is locked, as applied to the documents; `None` without an account hook.
    pub fn edit_lock(&self) -> Option<EditLock> {
        self.account.lock
    }

    /// Opens File › Account; does nothing without an account hook.
    pub fn show_account_page(&mut self) {
        if self.services.account.is_some() {
            self.ui.backstage = true;
            self.ui.backstage_page = ACCOUNT_PAGE.into();
        }
    }

    pub(crate) fn account_snapshot(&self) -> Option<AccountSnapshot> {
        self.services.account.as_ref().map(|hook| hook.snapshot())
    }

    pub(crate) fn attach_account(&mut self, ctx: &egui::Context) {
        if self.account.attached {
            return;
        }
        if let Some(hook) = &self.services.account {
            hook.attach(ctx);
            self.account.attached = true;
        }
    }

    /// Every frame, from `logic`: the hook's edit lock goes on every open document, or comes off
    /// without reopening any. What the input method was composing goes in first, as when a host
    /// turns editing off; a dialog whose OK would edit then closes (`access_gate::close_refused`).
    pub(crate) fn sync_edit_lock(&mut self) {
        let lock = self.services.account.as_ref().and_then(|hook| hook.edit_lock());
        if lock.is_some() && !self.session.edit_lock {
            self.commit_composition();
        }
        self.account.lock = lock;
        self.session.edit_lock = lock.is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::{is_paid_feature, is_pro_feature};

    #[test]
    fn the_avatar_shows_the_given_name_like_the_office_shell() {
        assert_eq!(super::avatar_letter("Nguyễn Văn An", None).as_deref(), Some("A"));
        assert_eq!(super::avatar_letter(" ", Some("binh@example.com")).as_deref(), Some("B"));
        assert_eq!(super::avatar_letter("", None), None);
    }

    #[test]
    fn view_and_export_stay_free_and_editing_needs_a_licence() {
        for free in ["erpconsoffice.view", "erpconsoffice.export"] {
            assert!(!is_pro_feature(free) && !is_paid_feature(free), "{free}");
        }
        assert!(is_pro_feature("erpconsoffice.use"), "editing needs a lease");
        assert!(!is_paid_feature("erpconsoffice.use"), "the free plan grants it");
        for paid in ["erpconsoffice.advanced", "erpconsoffice.mail_merge", "erpconsoffice.use.extra"] {
            assert!(is_pro_feature(paid) && is_paid_feature(paid), "{paid}");
        }
    }
}
