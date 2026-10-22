//! Native adapter for the signed ERPCons updater.

use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use erpcons_update::{CheckOutcome, Config, Error, InstallMode, Offer, PreviousResult, Ready, Updater};
use wordcraft_ui_egui::{
    i18n,
    updates::{CheckOutcome as UiOutcome, Offer as UiOffer, PreviousResult as UiPrevious, UpdateHook},
};

struct State {
    updater: Updater,
    offer: Option<Offer>,
    ready: Option<Ready>,
}

pub struct DesktopUpdates {
    state: Mutex<State>,
    auto_check: AtomicBool,
}

impl DesktopUpdates {
    pub fn new(state_path: PathBuf) -> Self {
        let updater = Updater::new(Config {
            app_id: "vanban",
            repo: "wordcraft",
            current_version: env!("CARGO_PKG_VERSION"),
            public_key: erpcons_update::ERPCONS_PUBLIC_KEY,
            state_path,
            user_agent: format!("ERPCons-VanBan/{}", env!("CARGO_PKG_VERSION")),
        });
        let auto_check = AtomicBool::new(updater.auto_check_enabled());
        Self { state: Mutex::new(State { updater, offer: None, ready: None }), auto_check }
    }

    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>, String> {
        self.state.try_lock().map_err(|error| match error {
            std::sync::TryLockError::WouldBlock => i18n::t("The updater is busy; try again shortly").into(),
            std::sync::TryLockError::Poisoned(_) => i18n::t("The updater failed; restart the app and try again").into(),
        })
    }
}

impl UpdateHook for DesktopUpdates {
    fn check(&self, forced: bool) -> Result<UiOutcome, String> {
        let mut state = self.state()?;
        match state.updater.check(forced).map_err(message)? {
            CheckOutcome::Available(offer) => {
                let ui = UiOffer {
                    version: offer.version.clone(),
                    notes_url: offer.notes_url.clone(),
                    download_page_only: offer.install == InstallMode::DownloadPageOnly,
                    windows_msi: windows_msi(),
                };
                state.offer = Some(offer);
                state.ready = None;
                Ok(UiOutcome::Available(ui))
            }
            CheckOutcome::UpToDate => Ok(UiOutcome::UpToDate),
            CheckOutcome::NotDue | CheckOutcome::Disabled | CheckOutcome::Skipped(_) => Ok(UiOutcome::Quiet),
        }
    }

    fn download(&self, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
        let mut state = self.state()?;
        let offer = state.offer.as_ref().ok_or_else(|| message(Error::Unsupported))?;
        let ready = state.updater.download(offer, progress, cancel).map_err(message)?;
        state.ready = Some(ready);
        Ok(())
    }

    fn install_and_restart(&self) -> Result<(), String> {
        let mut state = self.state()?;
        let ready = state.ready.take().ok_or_else(|| message(Error::Unsupported))?;
        state.updater.install_and_restart(ready).map_err(message)
    }

    fn skip(&self, version: &str) -> Result<(), String> {
        self.state()?.updater.skip(version).map_err(message)
    }

    fn auto_check(&self) -> bool {
        self.auto_check.load(Ordering::Relaxed)
    }

    fn set_auto_check(&self, on: bool) -> Result<(), String> {
        self.state()?.updater.set_auto_check(on).map_err(message)?;
        self.auto_check.store(on, Ordering::Relaxed);
        Ok(())
    }

    fn take_previous_result(&self) -> Option<UiPrevious> {
        self.state().ok()?.updater.take_previous_result().map(|result| match result {
            PreviousResult::Installed { version } => UiPrevious::Installed { version },
            PreviousResult::Failed { version, reason } => UiPrevious::Failed { version, reason },
        })
    }
}

fn windows_msi() -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    // Match v0.1.0's MSI detection; its public InstallMode only distinguishes automatic and page-only.
    ["ProgramFiles", "ProgramW6432"].iter().filter_map(std::env::var_os).any(|path| {
        let prefix = path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
        !prefix.is_empty() && exe.starts_with(&format!("{prefix}/"))
    })
}

fn message(error: Error) -> String {
    let (key, detail) = match error {
        Error::Offline(e) => ("Update network unavailable: {detail}", e),
        Error::Http(status) => ("Update server returned HTTP {detail}", status.to_string()),
        Error::BadSignature => ("The update signature is invalid", String::new()),
        Error::BadManifest(e) => ("The update manifest is invalid: {detail}", e),
        Error::HashMismatch => ("The downloaded update failed verification", String::new()),
        Error::Io(e) => ("Could not write the update: {detail}", e),
        Error::Unsupported => ("This installation cannot update automatically", String::new()),
        Error::PermissionDenied => ("Permission denied while updating; check folder permissions", String::new()),
        Error::Cancelled => ("Update cancelled", String::new()),
    };
    i18n::fmt(i18n::t(key), &[("detail", &detail)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_update_error_is_translated() {
        let previous = i18n::current();
        for (language, network) in [("en", "Update network unavailable: test"), ("vi", "Không có kết nối cập nhật: test")] {
            i18n::set_current(i18n::Lang::from_pref(language));
            assert_eq!(message(Error::Offline("test".into())), network);
            let cases = [
                (Error::Http(403), "Update server returned HTTP 403", "Máy chủ cập nhật trả về HTTP 403"),
                (Error::BadSignature, "The update signature is invalid", "Chữ ký bản cập nhật không hợp lệ"),
                (Error::BadManifest("test".into()), "The update manifest is invalid: test", "Thông tin bản cập nhật không hợp lệ: test"),
                (Error::HashMismatch, "The downloaded update failed verification", "Bản cập nhật đã tải không vượt qua xác minh"),
                (Error::Io("test".into()), "Could not write the update: test", "Không ghi được bản cập nhật: test"),
                (Error::Unsupported, "This installation cannot update automatically", "Bản cài đặt này không hỗ trợ cập nhật tự động"),
                (
                    Error::PermissionDenied,
                    "Permission denied while updating; check folder permissions",
                    "Không có quyền cập nhật; hãy kiểm tra quyền truy cập thư mục",
                ),
                (Error::Cancelled, "Update cancelled", "Đã huỷ cập nhật"),
            ];
            for (error, en, vi) in cases {
                assert_eq!(message(error), if language == "vi" { vi } else { en });
            }
        }
        i18n::set_current(previous);
    }
}
