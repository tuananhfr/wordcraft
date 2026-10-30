//! Why the app does not let the user edit: the desktop app's PI licence (erpcons-account ADR 0013).
//! The account hook reports it; the app shows every document read-only, Save still allowed.

/// The reason editing is locked; [`EditLock::reason`] is its English catalog key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditLock {
    /// The account is still being read at start-up.
    Checking,
    /// The account could not be opened on this computer.
    Unavailable,
    /// A guest, or a sign-in still in the browser.
    SignIn,
    /// The personal context: PI licenses organizations only.
    ChooseOrg,
    Revoked,
    DeviceCap,
    NotActivated,
    /// No credential store: the device key cannot be kept, so there is no lease either.
    SessionOnly,
    /// The lease expired while offline.
    RenewOffline,
    /// The lease expired while online: renewal failed or has not run yet.
    RenewFailed,
    NoPlan,
}

impl EditLock {
    pub fn reason(self) -> &'static str {
        match self {
            EditLock::Checking => "Checking your PI ID license…",
            EditLock::Unavailable => "PI ID licensing is unavailable on this computer, so editing is off",
            EditLock::SignIn => "Sign in with PI ID to edit",
            EditLock::ChooseOrg => "Choose an organization to edit",
            EditLock::Revoked => "This device was removed from your PI ID account. Sign in again to edit",
            EditLock::DeviceCap => "Your plan's device limit is reached. Remove a device in PI ID, then press Retry in File › Account",
            EditLock::NotActivated | EditLock::SessionOnly => "This computer is not activated for editing yet",
            EditLock::RenewOffline => "Connect to the internet to renew your license and edit",
            EditLock::RenewFailed => "Could not renew your license. The app keeps trying",
            EditLock::NoPlan => "This organization has no Office plan for editing",
        }
    }

    /// A plain sync with PI may lift it: activation retried, lease renewed, a plan bought or an
    /// organization joined since the last sync.
    pub fn sync_may_lift(self) -> bool {
        matches!(self, EditLock::ChooseOrg | EditLock::NotActivated | EditLock::RenewOffline | EditLock::RenewFailed | EditLock::NoPlan)
    }

    /// File › Account offers Retry; a device-cap block lifts only on the user's retry (lease.md §6).
    pub fn retry_offered(self) -> bool {
        self.sync_may_lift() || self == EditLock::DeviceCap
    }
}
