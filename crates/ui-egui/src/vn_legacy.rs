//! Ribbon entries and localized errors for Vietnamese legacy conversion.

use serde_json::json;
use wordcraft_engine::CmdError;

use crate::{WordApp, i18n};

pub(crate) fn menu(ui: &mut egui::Ui, app: &mut WordApp) {
    for (label, command) in [
        ("Convert TCVN3/VNI → Unicode", "tools.convert_vietnamese_legacy"),
        ("Convert TCVN3 → Unicode", "tools.convert_vietnamese_legacy_tcvn3"),
        ("Convert VNI → Unicode", "tools.convert_vietnamese_legacy_vni"),
    ] {
        crate::ribbon::mi(ui, app, label, command, json!({}));
    }
}

pub(crate) fn command_error(id: &str, error: CmdError) -> String {
    localized_error(id, &error, i18n::current())
}

fn localized_error(id: &str, error: &CmdError, language: i18n::Lang) -> String {
    if matches!(id, "tools.convert_vietnamese_legacy" | "tools.convert_vietnamese_legacy_tcvn3" | "tools.convert_vietnamese_legacy_vni") {
        match error {
            CmdError::Failed(message) => return i18n::tr(language, message).into(),
            CmdError::Disabled(message) => {
                let reason = message.strip_prefix(id).and_then(|s| s.strip_prefix(": ")).unwrap_or(message);
                return i18n::tr(language, reason).into();
            }
            _ => {}
        }
    }
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_errors_are_localized_without_changing_other_commands() {
        let vi = i18n::Lang::from_code("vi").unwrap();
        let error = CmdError::Failed("The document contains invalid text formatting.".into());
        assert_eq!(localized_error("tools.convert_vietnamese_legacy", &error, vi), "Tài liệu có định dạng văn bản không hợp lệ.");
        assert_eq!(localized_error("text.insert", &error, vi), error.to_string());
        let error = CmdError::Disabled("tools.convert_vietnamese_legacy_vni: Select text to choose a legacy encoding.".into());
        assert_eq!(localized_error("tools.convert_vietnamese_legacy_vni", &error, vi), "Chọn văn bản để chọn bảng mã cũ.");
    }
}
