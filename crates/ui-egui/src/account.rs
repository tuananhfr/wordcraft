//! Optional PI ID account (ERPCons): File › Account, the title bar's account button and the paid
//! licence expiry notice. The host installs an [`AccountHook`] (desktop: `erpcons-account` on a
//! worker thread; web: the office shell's `window.erpAccount`). Without one nothing here shows and
//! every feature is allowed.

mod page;

pub use page::{date_text, error_text};
pub(crate) use page::{page, title_button, warning_notice};

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
    fn retry_activation(&self);
    fn allowed(&self, feature: &str) -> bool;
    fn warning_shown(&self, days: u32);
    /// Called once with the egui context so the adapter can request a repaint when its snapshot changes.
    fn attach(&self, ctx: &egui::Context);
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

/// Whether holding `feature` means a paid plan. `.view` and `.export` are pinned free and the built-in
/// catalog lists `.use` as free (products.md §2, §3); anything else needs an entitlement.
pub fn is_pro_feature(feature: &str) -> bool {
    let builtin_free = feature.split_once('.').is_some_and(|(_, name)| name == "use");
    !(feature.ends_with(".view") || feature.ends_with(".export") || builtin_free)
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
}

impl WordApp {
    /// Whether `feature` may be used. True without an account hook; nothing is gated on it yet.
    pub fn allowed(&self, feature: &str) -> bool {
        self.services.account.as_ref().is_none_or(|hook| hook.allowed(feature))
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
}

#[cfg(test)]
mod tests {
    use super::is_pro_feature;

    #[test]
    fn the_avatar_shows_the_given_name_like_the_office_shell() {
        assert_eq!(super::avatar_letter("Nguyễn Văn An", None).as_deref(), Some("A"));
        assert_eq!(super::avatar_letter(" ", Some("binh@example.com")).as_deref(), Some("B"));
        assert_eq!(super::avatar_letter("", None), None);
    }

    #[test]
    fn only_features_outside_the_free_built_ins_are_pro() {
        for free in ["vanban.view", "vanban.export", "vanban.use", "pdf.view", "bangtinh.export"] {
            assert!(!is_pro_feature(free), "{free}");
        }
        for pro in ["vanban.advanced", "vanban.mail_merge", "pdf.ocr", "vanban.use.extra"] {
            assert!(is_pro_feature(pro), "{pro}");
        }
    }
}
