//! Desktop update notices; all network and installation work belongs to the host.

pub use native::*;
pub(crate) use native::{Updates, dialog, preferences};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    };

    use crate::{WordApp, i18n, theme};

    pub const RELEASES_PAGE: &str = "https://github.com/tuananhfr/wordcraft/releases";

    /// Display metadata only; the native adapter retains the verified offer and download.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Offer {
        pub version: String,
        pub notes_url: String,
        pub download_page_only: bool,
        pub windows_msi: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum CheckOutcome {
        Quiet,
        UpToDate,
        Available(Offer),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum PreviousResult {
        Installed { version: String },
        Failed { version: String, reason: String },
    }

    /// Blocking native operations; errors are translated by the application adapter.
    pub trait UpdateHook: Send + Sync {
        fn check(&self, forced: bool) -> Result<CheckOutcome, String>;
        fn download(&self, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String>;
        fn install_and_restart(&self) -> Result<(), String>;
        fn skip(&self, version: &str) -> Result<(), String>;
        fn auto_check(&self) -> bool;
        fn set_auto_check(&self, on: bool) -> Result<(), String>;
        fn take_previous_result(&self) -> Option<PreviousResult>;
    }

    enum Event {
        Checked(Result<CheckOutcome, String>, bool),
        Downloaded(Result<(), String>),
    }

    #[derive(Default)]
    pub(crate) struct Updates {
        pub(crate) open: bool,
        started: bool,
        queued_check: bool,
        approved: Vec<(u64, u64)>,
        offer: Option<Offer>,
        worker: Option<Receiver<Event>>,
        downloading: bool,
        ready: bool,
        ready_at: Option<std::time::Instant>,
        previous_until: Option<std::time::Instant>,
        message: Option<String>,
        retry_check: bool,
        retry_download: bool,
        previous: Option<(String, bool)>,
        cancel: Arc<AtomicBool>,
        received: Arc<AtomicU64>,
        total: Arc<AtomicU64>,
    }

    impl WordApp {
        fn update_open_url(&mut self, url: &str) {
            if let Some(ctx) = &self.ctx {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }

        /// Forced checks bypass the native service's throttle, preference and skipped version.
        pub fn check_for_updates(&mut self) {
            self.start_update_check(true);
        }

        fn start_update_check(&mut self, forced: bool) {
            let Some(hook) = self.services.updates.clone() else {
                self.update_open_url(RELEASES_PAGE);
                return;
            };
            if self.updates.worker.is_some() || self.updates.ready {
                self.updates.open |= forced;
                self.updates.queued_check |= forced && !self.updates.downloading && !self.updates.ready;
                return;
            }
            self.updates.open = forced;
            self.updates.offer = None;
            self.updates.message = None;
            self.updates.retry_check = false;
            self.updates.retry_download = false;
            let (tx, rx) = mpsc::channel();
            let ctx = self.ctx.clone();
            let language = i18n::current();
            let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
                i18n::set_current(language);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook.check(forced)))
                    .unwrap_or_else(|_| Err(tl!("The update check stopped unexpectedly").into()));
                let _ = tx.send(Event::Checked(result, forced));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            match spawned {
                Ok(_) => self.updates.worker = Some(rx),
                Err(error) => {
                    self.updates.message = Some(i18n::fmt(tl!("Could not start updater: {reason}"), &[("reason", &error.to_string())]));
                    self.updates.retry_check = true;
                    self.updates.open = forced;
                }
            }
        }

        pub(crate) fn poll_updates(&mut self) {
            if !self.updates.started {
                self.updates.started = true;
                if let Some(hook) = self.services.updates.clone() {
                    if let Some(previous) = hook.take_previous_result() {
                        self.updates.previous = Some(match previous {
                            PreviousResult::Installed { version } => {
                                self.updates.previous_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(6));
                                (i18n::fmt(tl!("Updated to version {version}"), &[("version", &version)]), false)
                            }
                            PreviousResult::Failed { version, reason } => {
                                (i18n::fmt(tl!("Update to {version} failed: {reason}"), &[("version", &version), ("reason", &reason)]), true)
                            }
                        });
                    }
                    if hook.auto_check() {
                        self.start_update_check(false);
                    }
                }
            }
            let Some(rx) = &self.updates.worker else { return };
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.updates.worker = None;
                    self.updates.message = Some(tl!("The update check stopped unexpectedly").into());
                    self.updates.retry_download = self.updates.downloading;
                    self.updates.retry_check = !self.updates.downloading;
                    self.updates.downloading = false;
                    self.updates.open = true;
                    return;
                }
            };
            self.updates.worker = None;
            if self.updates.queued_check && matches!(event, Event::Checked(..)) {
                self.updates.queued_check = false;
                self.start_update_check(true);
                return;
            }
            match event {
                Event::Checked(Ok(CheckOutcome::Available(offer)), _) => {
                    self.updates.offer = Some(offer);
                    self.updates.open = true;
                }
                Event::Checked(Ok(CheckOutcome::UpToDate | CheckOutcome::Quiet), forced) => {
                    self.updates.open = forced;
                    if forced {
                        self.updates.message = Some(tl!("Văn bản – ERPCons is up to date.").into());
                    }
                }
                Event::Checked(Err(error), forced) => {
                    self.updates.open = forced;
                    self.updates.message = Some(error);
                    self.updates.retry_check = true;
                }
                Event::Downloaded(result) => {
                    self.updates.downloading = false;
                    if self.updates.cancel.load(Ordering::Relaxed) {
                        self.updates.message = None;
                        self.updates.ready = false;
                    } else {
                        match result {
                            Ok(()) => {
                                self.updates.ready = true;
                                self.updates.ready_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(800));
                                self.updates.message = Some(if self.updates.offer.as_ref().is_some_and(|offer| offer.windows_msi) {
                                    tl!("The app will close, Windows will ask for installation permission — choose Yes — then the app will reopen automatically").into()
                                } else {
                                    tl!("The app will close and reopen after updating").into()
                                });
                            }
                            Err(error) => {
                                self.updates.message = Some(error);
                                self.updates.retry_download = true;
                            }
                        }
                    }
                }
            }
        }

        pub(crate) fn request_update(&mut self, ctx: &egui::Context) {
            if self.dialog.is_some() || self.file_dialog_open() {
                return;
            }
            self.updates.ready_at = None;
            if !self.updates.ready {
                self.updates.approved.clear();
            }
            let _ = self.guard_update(serde_json::json!({"approved": self.updates.approved}), ctx);
        }

        pub(crate) fn guard_update(&mut self, params: serde_json::Value, ctx: &egui::Context) -> Result<serde_json::Value, String> {
            let mut approved: Vec<(u64, u64)> =
                params.get("approved").cloned().and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default();
            if let Some((document, revision)) = self
                .document_tabs()
                .iter()
                .filter_map(|tab| self.find_document(tab.document).map(|(session, _)| (tab.document, session.rev(), session.dirty)))
                .find_map(|(id, revision, dirty)| {
                    (self.picker_saving(id) || (dirty && !approved.contains(&(id, revision)))).then_some((id, revision))
                })
            {
                approved.push((document, revision));
                let params = serde_json::json!({"approved": approved});
                if self.picker_saving(document) {
                    self.continue_after_picker_save(document, "ui.continueUpdate".into(), params);
                } else if let Some((session, untitled)) = self.find_document(document) {
                    let name = crate::open_documents::prompt_name_of(session, untitled);
                    self.dialog = Some(crate::dialogs::Dialog::SaveChanges { name, then: "ui.continueUpdate".into(), params, document });
                }
                return Ok(serde_json::json!({"pending": "saveChanges"}));
            }
            self.updates.approved = approved;
            self.continue_update(ctx);
            Ok(serde_json::json!({"done": true}))
        }

        pub(crate) fn continue_update(&mut self, ctx: &egui::Context) {
            let Some(hook) = self.services.updates.clone() else { return };
            if self.updates.ready {
                self.updates.ready = false;
                match hook.install_and_restart() {
                    Ok(()) => {
                        self.updates.open = false;
                        self.quit_requested = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(error) => {
                        self.updates.ready = false;
                        self.updates.message = Some(error);
                        self.updates.retry_download = true;
                    }
                }
                return;
            }
            if self.updates.worker.is_some() || self.updates.offer.is_none() {
                return;
            }
            self.updates.message = None;
            self.updates.retry_download = false;
            self.updates.downloading = true;
            self.updates.cancel = Arc::new(AtomicBool::new(false));
            self.updates.received.store(0, Ordering::Relaxed);
            self.updates.total.store(0, Ordering::Relaxed);
            let cancel = self.updates.cancel.clone();
            let received = self.updates.received.clone();
            let total = self.updates.total.clone();
            let ctx = ctx.clone();
            let language = i18n::current();
            let (tx, rx) = mpsc::channel();
            let spawned = std::thread::Builder::new().name("update-download".into()).spawn(move || {
                i18n::set_current(language);
                let progress = |bytes, size| {
                    received.store(bytes, Ordering::Relaxed);
                    total.store(size, Ordering::Relaxed);
                    ctx.request_repaint();
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook.download(&progress, &cancel)))
                    .unwrap_or_else(|_| Err(tl!("The update check stopped unexpectedly").into()));
                let _ = tx.send(Event::Downloaded(result));
                ctx.request_repaint();
            });
            match spawned {
                Ok(_) => self.updates.worker = Some(rx),
                Err(error) => {
                    self.updates.downloading = false;
                    self.updates.message = Some(i18n::fmt(tl!("Could not start updater: {reason}"), &[("reason", &error.to_string())]));
                    self.updates.retry_download = true;
                }
            }
        }

        fn skip_update(&mut self) {
            if let Some(hook) = &self.services.updates
                && let Some(offer) = &self.updates.offer
            {
                match hook.skip(&offer.version) {
                    Ok(()) => self.updates.open = false,
                    Err(error) => self.updates.message = Some(error),
                }
            }
        }
    }

    pub(crate) fn preferences(ui: &mut egui::Ui, app: &mut WordApp) {
        if let Some(hook) = app.services.updates.clone() {
            let mut on = hook.auto_check();
            if ui.add_enabled(app.updates.worker.is_none(), egui::Checkbox::new(&mut on, tl!("Automatically check for updates"))).changed()
                && let Err(error) = hook.set_auto_check(on)
            {
                app.status(error);
            }
        }
    }

    pub(crate) fn dialog(app: &mut WordApp, ctx: &egui::Context) {
        if let Some(until) = app.updates.previous_until {
            let remaining = until.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                app.updates.previous = None;
                app.updates.previous_until = None;
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
        if !app.updates.open && app.updates.previous.is_none() {
            return;
        }
        let mut later = false;
        let mut skip = false;
        let mut update = false;
        let mut retry = false;
        let mut notes = None;
        let mut dismiss_previous = false;
        let t = theme::Tokens::get(ctx);
        egui::Area::new(egui::Id::new("native-update-notice"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 48.0).clamp(120.0, 420.0));
                    if let Some((message, failed)) = &app.updates.previous {
                        ui.label(message);
                        ui.horizontal_wrapped(|ui| {
                            if *failed && ui.button(tl!("Retry")).clicked() {
                                retry = true;
                                dismiss_previous = true;
                            }
                            dismiss_previous |= ui.button(tl!("Close")).clicked();
                        });
                    }
                    if !app.updates.open {
                        return;
                    }
                    if let Some(offer) = &app.updates.offer {
                        ui.label(
                            egui::RichText::new(i18n::fmt(tl!("Văn bản – ERPCons {version} is available"), &[("version", &offer.version)])).strong(),
                        );
                    }
                    if let Some(message) = &app.updates.message {
                        ui.label(egui::RichText::new(message).color(t.text));
                    }
                    if app.updates.downloading {
                        let received = app.updates.received.load(Ordering::Relaxed);
                        let total = app.updates.total.load(Ordering::Relaxed);
                        let fraction = if total == 0 { 0.0 } else { (received as f64 / total as f64).clamp(0.0, 1.0) as f32 };
                        ui.add(egui::ProgressBar::new(fraction).show_percentage());
                        if ui.button(tl!("Cancel")).clicked() {
                            app.updates.cancel.store(true, Ordering::Relaxed);
                        }
                    } else if app.updates.worker.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(tl!("Checking for a newer version…"));
                        });
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            if app.updates.retry_check || app.updates.retry_download {
                                retry = ui.button(tl!("Retry")).clicked();
                                later = ui.button(tl!("Close")).clicked();
                            } else if app.updates.ready {
                                if app.updates.ready_at.is_none() {
                                    update = ui.button(tl!("Retry")).clicked();
                                }
                            } else if let Some(offer) = &app.updates.offer {
                                if offer.download_page_only {
                                    if ui.add(crate::widgets::primary_button(ui, tl!("Download new version"))).clicked() {
                                        notes = Some(offer.notes_url.clone());
                                    }
                                } else {
                                    update = ui.add(crate::widgets::primary_button(ui, tl!("Update now"))).clicked();
                                }
                                later = ui.button(tl!("Remind me later")).clicked();
                                skip = ui.button(tl!("Skip this version")).clicked();
                            } else if !app.updates.ready {
                                later = ui.button(tl!("Close")).clicked();
                            }
                        });
                        if let Some(offer) = &app.updates.offer
                            && !app.updates.ready
                            && ui.link(tl!("What's new")).clicked()
                        {
                            notes = Some(offer.notes_url.clone());
                        }
                    }
                });
            });
        if dismiss_previous {
            app.updates.previous = None;
        }
        if let Some(url) = notes {
            app.update_open_url(&url);
        }
        if later {
            app.updates.open = false;
        }
        if skip {
            app.skip_update();
        }
        if retry {
            if app.updates.retry_download && !dismiss_previous {
                app.request_update(ctx);
            } else {
                app.start_update_check(true);
            }
        }
        if update {
            app.request_update(ctx);
        }
        // New edits made during download must pass the save guard too.
        if app.updates.ready
            && app.dialog.is_none()
            && !app.file_dialog_open()
            && let Some(at) = app.updates.ready_at
        {
            let remaining = at.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                app.updates.ready_at = None;
                app.request_update(ctx);
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
}
