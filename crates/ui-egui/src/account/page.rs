//! File › Account, the title bar's account button, the paid licence expiry notice and the edit
//! lock notice.

use egui::{Align2, Rect, Response, RichText, Sense, Stroke, TextWrapMode, Ui, vec2};

use super::{AccountError, AccountView, EditLock, LicenceView, SignedInView};
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
    // At start-up the lock only says the licence is being read; nothing to show for it here.
    let lock = app.edit_lock().filter(|lock| *lock != EditLock::Checking);
    let mut action = None;
    match &snapshot.view {
        AccountView::Guest => {
            ui.add_enabled_ui(!snapshot.busy, |ui| {
                if ui.add(crate::widgets::primary_button(ui, tl!("Sign in with PI ID"))).clicked() {
                    action = Some(Action::SignIn);
                }
            });
            ui.add_space(6.0);
            let line = lock.map_or(tl!("Signing in is optional: every feature works without an account."), |lock| i18n::t(lock.reason()));
            ui.add(egui::Label::new(RichText::new(line).color(t.text_dim)).wrap());
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
            ui.add_enabled_ui(!snapshot.busy, |ui| signed_in(ui, view, lock, &t, &mut action));
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

fn signed_in(ui: &mut Ui, view: &SignedInView, lock: Option<EditLock>, t: &Tokens, action: &mut Option<Action>) {
    let name = if view.name.trim().is_empty() { tl!("PI ID account") } else { view.name.as_str() };
    // egui wraps a word wider than the page at any letter or `@`/`.`/`_`, which splits names and
    // addresses on phone-wide pages; one elided line keeps them whole, with the full text on hover.
    ui.add(egui::Label::new(RichText::new(name).font(semibold(18.0))).truncate());
    if let Some(email) = &view.email {
        ui.add(egui::Label::new(RichText::new(email).color(t.text_dim)).truncate());
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
    let orgs = view.orgs.iter().map(|org| (Some(org.id.clone()), org.name.clone()));
    for (id, label) in personal.chain(orgs) {
        if one_line_radio(ui, view.active_org == id, &label).clicked() && view.active_org != id {
            *action = Some(Action::SelectOrg(id));
        }
    }
    ui.add_space(14.0);
    ui.label(RichText::new(tl!("License")).font(semibold(15.0)));
    for line in licence_lines(view.licence) {
        ui.add(egui::Label::new(line).wrap());
    }
    if let Some(lock) = lock {
        ui.add(egui::Label::new(RichText::new(i18n::t(lock.reason())).font(medium(14.0))).wrap());
    }
    let retry = lock.map_or(view.licence == LicenceView::DeviceCapReached, EditLock::retry_offered);
    if retry && ui.button(tl!("Retry")).clicked() {
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

/// A radio button whose label stays on one line, elided with the full text on hover when it does not fit.
fn one_line_radio(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let text_width = egui::WidgetText::from(label).into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button).size().x;
    let fits = ui.spacing().icon_width + ui.spacing().icon_spacing + text_width <= ui.available_width();
    let response = ui
        .scope(|ui| {
            ui.style_mut().wrap_mode = Some(TextWrapMode::Truncate);
            ui.radio(selected, label)
        })
        .inner;
    if fits { response } else { response.on_hover_text(label) }
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
    crate::account::avatar_letter(&view.name, view.email.as_deref())
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
    let (close, open) = notice_bar(ui, &text, tl!("Account"));
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

/// Why editing is locked, above the page, with the way out: Sign in for a guest, File › Account
/// otherwise. Closing it hides it until the reason changes.
pub(crate) fn lock_notice(app: &mut WordApp, ui: &mut Ui) {
    // At start-up the lock only says the licence is being read: no notice for a moment's check.
    let Some(lock) = app.edit_lock().filter(|lock| *lock != EditLock::Checking) else { return };
    if app.account.lock_dismissed == Some(lock) {
        return;
    }
    let guest = app.account_snapshot().is_some_and(|s| s.view == AccountView::Guest);
    let (close, act) = notice_bar(ui, i18n::t(lock.reason()), if guest { tl!("Sign in") } else { tl!("Account") });
    if close {
        app.account.lock_dismissed = Some(lock);
    }
    if act {
        if guest && let Some(hook) = &app.services.account {
            hook.sign_in();
        }
        app.show_account_page();
    }
}

/// A notice bar above the page: `text`, a button labelled `action` and Close. Returns which of
/// Close and `action` was clicked.
fn notice_bar(ui: &mut Ui, text: &str, action: &str) -> (bool, bool) {
    let t = Tokens::get(ui.ctx());
    let (mut close, mut act) = (false, false);
    egui::Frame::new().fill(t.panel).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                close = ui.small_button(tl!("Close")).clicked();
                act = ui.small_button(action).clicked();
                ui.add(egui::Label::new(RichText::new(text).font(regular(13.0))).wrap());
            });
        });
    });
    (close, act)
}

/// A local calendar date: `dd/mm/yyyy` in Vietnamese, the ISO date WordCraft shows elsewhere otherwise.
pub fn date_text(unix_secs: i64) -> String {
    let (y, m, d) = local_ymd(unix_secs);
    if i18n::current().code() == "vi" { format!("{d:02}/{m:02}/{y:04}") } else { format!("{y:04}-{m:02}-{d:02}") }
}

#[cfg(not(target_arch = "wasm32"))]
fn local_ymd(unix_secs: i64) -> (i64, u32, u32) {
    use chrono::Datelike;
    match chrono::DateTime::from_timestamp(unix_secs, 0) {
        Some(utc) => {
            let local = utc.with_timezone(&chrono::Local);
            (i64::from(local.year()), local.month(), local.day())
        }
        // Beyond chrono's ±262,000-year range only a corrupt timestamp lands here; the UTC day still renders.
        None => wordcraft_engine::cmd::civil_from_days(unix_secs.div_euclid(86_400)),
    }
}

#[cfg(target_arch = "wasm32")]
fn local_ymd(unix_secs: i64) -> (i64, u32, u32) {
    wordcraft_engine::cmd::civil_from_days(unix_secs.saturating_add(local_offset_secs()).div_euclid(86_400))
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
        AccountError::PopupBlocked => tl!("The browser blocked the sign-in window. Allow pop-ups for this site and try again."),
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
