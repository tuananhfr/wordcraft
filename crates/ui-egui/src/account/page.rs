//! File › Account, the title bar's account button and the paid licence expiry notice.

use egui::{Align2, Rect, RichText, Sense, Stroke, Ui, vec2};

use super::{AccountError, AccountView, LicenceView, SignedInView};
use crate::theme::{Tokens, medium, regular, semibold};
use crate::{WordApp, i18n, icons};

enum Action {
    SignIn,
    CancelSignIn,
    SignOut,
    SelectOrg(Option<String>),
    RetryActivation,
}

pub(crate) fn page(app: &mut WordApp, ui: &mut Ui) {
    let Some(snapshot) = app.account_snapshot() else { return };
    let t = Tokens::get(ui.ctx());
    crate::backstage::heading(ui, "Account");
    let mut action = None;
    match &snapshot.view {
        AccountView::Guest => {
            ui.add_enabled_ui(!snapshot.busy, |ui| {
                if ui.add(crate::widgets::primary_button(ui, tl!("Sign in with PI ID"))).clicked() {
                    action = Some(Action::SignIn);
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new(tl!("Signing in is optional: every feature works without an account.")).color(t.text_dim));
        }
        AccountView::SigningIn => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Continue in your browser…"));
            });
            ui.add_space(6.0);
            if ui.button(tl!("Cancel")).clicked() {
                action = Some(Action::CancelSignIn);
            }
        }
        AccountView::SignedIn(view) => {
            ui.add_enabled_ui(!snapshot.busy, |ui| signed_in(ui, view, &t, &mut action));
        }
    }
    if snapshot.busy && snapshot.view != AccountView::SigningIn {
        ui.add_space(8.0);
        ui.spinner();
    }
    if let Some(text) = snapshot.error.and_then(error_text) {
        ui.add_space(10.0);
        ui.add(egui::Label::new(RichText::new(text).color(t.red)).wrap());
    }
    let Some(hook) = &app.services.account else { return };
    // The web adapter opens its popup here: still inside the click's frame, as browsers require.
    match action {
        Some(Action::SignIn) => hook.sign_in(),
        Some(Action::CancelSignIn) => hook.cancel_sign_in(),
        Some(Action::SignOut) => hook.sign_out(),
        Some(Action::SelectOrg(org)) => hook.select_org(org),
        Some(Action::RetryActivation) => hook.retry_activation(),
        None => {}
    }
}

fn signed_in(ui: &mut Ui, view: &SignedInView, t: &Tokens, action: &mut Option<Action>) {
    let name = if view.name.trim().is_empty() { tl!("PI ID account") } else { view.name.as_str() };
    ui.label(RichText::new(name).font(semibold(18.0)));
    if let Some(email) = &view.email {
        ui.label(RichText::new(email).color(t.text_dim));
    }
    if view.reauth_required {
        ui.add_space(8.0);
        ui.label(tl!("Your session has expired. Sign in again."));
        if ui.add(crate::widgets::primary_button(ui, tl!("Sign in again"))).clicked() {
            *action = Some(Action::SignIn);
        }
    }
    ui.add_space(14.0);
    ui.label(RichText::new(tl!("Organization")).font(semibold(15.0)));
    let personal = std::iter::once((None, tl!("Personal").to_string()));
    let orgs = view.orgs.iter().map(|org| {
        let label = match &org.role {
            Some(role) if !role.is_empty() => format!("{} ({role})", org.name),
            _ => org.name.clone(),
        };
        (Some(org.id.clone()), label)
    });
    for (id, label) in personal.chain(orgs) {
        if ui.radio(view.active_org == id, label).clicked() && view.active_org != id {
            *action = Some(Action::SelectOrg(id));
        }
    }
    ui.add_space(14.0);
    ui.label(RichText::new(tl!("License")).font(semibold(15.0)));
    for line in licence_lines(view.licence) {
        ui.add(egui::Label::new(line).wrap());
    }
    if view.licence == LicenceView::DeviceCapReached && ui.button(tl!("Retry")).clicked() {
        *action = Some(Action::RetryActivation);
    }
    if view.session_only {
        ui.add(egui::Label::new(RichText::new(tl!("You will need to sign in again the next time you open the app.")).color(t.text_dim)).wrap());
    }
    ui.add_space(8.0);
    let online = if view.online { tl!("Connected to PI ID") } else { tl!("Offline. Your account syncs when you reconnect.") };
    ui.label(RichText::new(online).color(t.text_dim));
    ui.add_space(14.0);
    if ui.button(RichText::new(tl!("Sign out")).font(medium(14.0))).clicked() {
        *action = Some(Action::SignOut);
    }
}

fn licence_lines(licence: LicenceView) -> Vec<String> {
    let with_date = |key: &str, at: i64| i18n::fmt(i18n::t(key), &[("date", &date_text(at))]);
    match licence {
        LicenceView::Free | LicenceView::Online { pro: false } => vec![tl!("Plan: Free").into()],
        LicenceView::Active { expires_at, pro: false } => {
            vec![tl!("Plan: Free").into(), with_date("This device is activated until {date}.", expires_at)]
        }
        LicenceView::Active { expires_at, pro: true } => vec![with_date("Paid features are valid until {date}.", expires_at)],
        LicenceView::Online { pro: true } => vec![tl!("Paid features are active.").into()],
        LicenceView::Inactive { expires_at } => vec![with_date("Your license expired on {date}. Free features keep working.", expires_at)],
        LicenceView::DeviceRevoked => vec![tl!("This device was removed from your account. Free features keep working.").into()],
        LicenceView::DeviceCapReached => vec![tl!("Your account has reached its device limit, so this device is not activated.").into()],
        LicenceView::SessionOnly => vec![tl!("Signed in for this session only: this computer has no credential store.").into()],
    }
}

/// The account button in the title bar, where the author's initials show without an account hook:
/// the user's initial when signed in, a person otherwise. It opens File › Account.
pub(crate) fn title_button(app: &mut WordApp, ui: &mut Ui, size: f32) {
    let Some(snapshot) = app.account_snapshot() else { return };
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let (label, initial) = match &snapshot.view {
        AccountView::SignedIn(view) => (display_name(view), initial(view)),
        AccountView::SigningIn => (tl!("Signing in…").to_string(), None),
        AccountView::Guest => (tl!("Sign in").to_string(), None),
    };
    match initial {
        Some(letter) => {
            ui.painter().circle_filled(r.center(), size / 2.0 - 1.0, t.accent);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, letter, semibold(11.5), t.on_accent);
        }
        None => {
            let fill = if resp.hovered() { t.hover } else { egui::Color32::TRANSPARENT };
            ui.painter().circle(r.center(), size / 2.0 - 1.0, fill, Stroke::new(1.0, t.border_strong));
            icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), "user", t.icon, t.icon);
        }
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    if resp.on_hover_text(&label).clicked() {
        app.show_account_page();
    }
}

fn display_name(view: &SignedInView) -> String {
    let name = view.name.trim();
    match (&view.email, name.is_empty()) {
        (_, false) => name.to_string(),
        (Some(email), true) => email.clone(),
        (None, true) => tl!("PI ID account").to_string(),
    }
}

fn initial(view: &SignedInView) -> Option<String> {
    view.name.chars().chain(view.email.iter().flat_map(|e| e.chars())).find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect())
}

/// The lease expiry notice above the page, shown only while paid features hang on that lease
/// (lease.md §8). Closing it tells the adapter, which records the stage as shown.
pub(crate) fn warning_notice(app: &mut WordApp, ui: &mut Ui) {
    let Some(snapshot) = app.account_snapshot() else { return };
    let AccountView::SignedIn(view) = &snapshot.view else { return };
    let (Some(warning), LicenceView::Active { pro: true, .. }) = (view.warning, view.licence) else { return };
    if app.account.dismissed == Some(warning) {
        return;
    }
    let days = warning.days.to_string();
    let text = i18n::fmt(
        tl!("Paid features expire on {date} ({days} days left). Connect to the internet to renew them."),
        &[("date", &date_text(warning.expires_at)), ("days", &days)],
    );
    let t = Tokens::get(ui.ctx());
    let (mut close, mut open) = (false, false);
    egui::Frame::new().fill(t.panel).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = ui.small_button(tl!("Close")).clicked();
                open = ui.small_button(tl!("Account")).clicked();
                ui.add(egui::Label::new(RichText::new(text).font(regular(13.0))).wrap());
            });
        });
    });
    if close {
        app.account.dismissed = Some(warning);
        if let Some(hook) = &app.services.account {
            hook.warning_shown(warning.days);
        }
    }
    if open {
        app.show_account_page();
    }
}

/// A local calendar date: `dd/mm/yyyy` in Vietnamese, the ISO date WordCraft shows elsewhere otherwise.
pub fn date_text(unix_secs: i64) -> String {
    let local = unix_secs.saturating_add(local_offset_secs());
    let (y, m, d) = wordcraft_engine::cmd::civil_from_days(local.div_euclid(86_400));
    if i18n::current().code() == "vi" { format!("{d:02}/{m:02}/{y:04}") } else { format!("{y:04}-{m:02}-{d:02}") }
}

// The desktop build has no time-zone source without a new dependency, so it shows the UTC date.
#[cfg(not(target_arch = "wasm32"))]
fn local_offset_secs() -> i64 {
    0
}

#[cfg(target_arch = "wasm32")]
fn local_offset_secs() -> i64 {
    let minutes = js_sys::Date::new_0().get_timezone_offset();
    if minutes.is_finite() { -(minutes as i64) * 60 } else { 0 }
}

/// The error line in the current language; `None` for a cancelled sign-in, which needs no message.
pub fn error_text(error: AccountError) -> Option<String> {
    let text = match error {
        AccountError::Cancelled => return None,
        AccountError::Network => tl!("Could not reach PI ID. Check your connection and try again."),
        AccountError::Timeout => tl!("PI ID took too long to answer. Try again."),
        AccountError::Denied => tl!("PI ID refused the request. Free features keep working."),
        AccountError::RateLimited { retry_after_secs: Some(secs) } if secs > 0 => {
            let minutes = secs.div_ceil(60).to_string();
            return Some(i18n::fmt(tl!("PI ID is busy. Try again in {minutes} min."), &[("minutes", &minutes)]));
        }
        AccountError::RateLimited { .. } => tl!("PI ID is busy. Try again in a few minutes."),
        AccountError::DeviceCap => tl!("Your account has reached its device limit. Remove a device in PI ID, then retry."),
        AccountError::DeviceRevoked => tl!("This device was removed from your account. Free features keep working."),
        AccountError::Suspended => tl!("Your PI ID account is suspended. Free features keep working."),
        AccountError::OrgUnavailable => tl!("This organization is not available to your account."),
        AccountError::ReauthRequired => tl!("Your session has expired. Sign in again."),
        AccountError::ClientTooOld => tl!("This version of the app is too old for PI ID. Update the app, then try again."),
        AccountError::ServiceUnavailable => tl!("PI ID is temporarily unavailable. Try again later."),
        AccountError::Other => tl!("Something went wrong with PI ID. Try again."),
    };
    Some(text.to_string())
}
