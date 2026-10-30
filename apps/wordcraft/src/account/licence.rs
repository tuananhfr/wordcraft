//! The edit gate: editing needs `erpconsoffice.use` in an active lease of this device.

use erpcons_account::{Account, LicenceState, OFFICE_USE, Status};
use wordcraft_ui_egui::account::EditLock;

/// Why editing is locked now, `None` when it is allowed. Reads only the account's cached state.
pub(super) fn edit_lock(account: &Account) -> Option<EditLock> {
    edit_lock_in(&account.status(), |feature| account.allowed(feature))
}

/// [`edit_lock`] of a status snapshot; `allowed` is asked only while the lease lists the feature.
pub(super) fn edit_lock_in(status: &Status, allowed: impl FnOnce(&str) -> bool) -> Option<EditLock> {
    let signed_in = match status {
        Status::Guest | Status::SigningIn => return Some(EditLock::SignIn),
        Status::SignedIn(signed_in) => signed_in,
    };
    // PI licenses through organizations only; an Office account gets a one-person one (D578).
    if signed_in.active_org.is_none() {
        return Some(EditLock::ChooseOrg);
    }
    // `allowed()` alone also grants on fresh online entitlements; only this device's lease
    // reflects a revoked or capped device, so it must grant too.
    match &signed_in.licence {
        LicenceState::DeviceRevoked => Some(EditLock::Revoked),
        LicenceState::DeviceCapReached => Some(EditLock::DeviceCap),
        LicenceState::NotEntitled => Some(EditLock::NoPlan),
        LicenceState::NotActivated => Some(EditLock::NotActivated),
        LicenceState::SessionOnly => Some(EditLock::SessionOnly),
        LicenceState::Inactive { .. } if !signed_in.online => Some(EditLock::RenewOffline),
        // Online yet expired: renewal failed or has not run yet, which is not "no plan".
        LicenceState::Inactive { .. } => Some(EditLock::RenewFailed),
        LicenceState::Active { features, .. } if features.iter().any(|f| f == OFFICE_USE) && allowed(OFFICE_USE) => None,
        LicenceState::Active { .. } => Some(EditLock::NoPlan),
    }
}
