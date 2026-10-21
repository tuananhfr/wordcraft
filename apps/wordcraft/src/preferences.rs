use std::io::Write;
use std::path::Path;
use wordcraft_ui_egui::UiState;

pub fn resolve(bytes: Option<&[u8]>, language_override: Option<&str>) -> UiState {
    let mut ui = bytes.and_then(UiState::from_json).unwrap_or_default();
    // Deserialization defaults a missing field to auto; only an explicit saved auto follows the system.
    let saved: Option<serde_json::Value> = bytes.and_then(|b| serde_json::from_slice(b).ok());
    let saved_language = saved.as_ref().and_then(|v| v.get("language")).and_then(|v| v.as_str());
    ui.language = saved_language.unwrap_or(if language_override.is_some() { "auto" } else { "vi" }).into();
    ui
}

pub fn language_changed(previous: &str, current: &str, save: impl FnOnce()) {
    if previous != current {
        save();
    }
}

pub fn save(path: &Path, ui: &UiState) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "settings path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(ui).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&bytes).map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_saved_defaults_to_vietnamese() {
        for bytes in [None, Some(b"{}".as_slice()), Some(br#"{"darkPage":true}"#.as_slice())] {
            assert_eq!(resolve(bytes, None).language, "vi");
        }
        assert!(resolve(Some(br#"{"darkPage":true}"#), None).dark_page);
    }

    #[test]
    fn saved_language_including_auto_is_respected() {
        for code in ["en", "vi", "auto"] {
            let bytes = serde_json::to_vec(&serde_json::json!({"language":code})).unwrap();
            assert_eq!(resolve(Some(&bytes), None).language, code);
        }
    }

    #[test]
    fn runtime_override_only_selects_auto_when_no_language_is_saved() {
        for value in ["en-US", "auto", "invalid", ""] {
            for bytes in [None, Some(b"{}".as_slice()), Some(br#"{"darkPage":true}"#.as_slice())] {
                assert_eq!(resolve(bytes, Some(value)).language, "auto");
            }
            for code in ["en", "vi", "auto"] {
                let bytes = serde_json::to_vec(&serde_json::json!({"language":code})).unwrap();
                assert_eq!(resolve(Some(&bytes), Some(value)).language, code);
            }
        }
    }

    #[test]
    fn saving_with_runtime_override_does_not_persist_its_language() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.json");
        let ui = resolve(None, Some("en-US"));
        save(&path, &ui).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved.get("language").and_then(|v| v.as_str()), Some("auto"));
        assert_eq!(resolve(Some(&bytes), None).language, "auto");
    }

    #[test]
    fn language_change_saves_immediately_and_replaces_existing_preferences() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings/ui.json");
        let mut ui = resolve(None, None);
        save(&path, &ui).unwrap();
        for code in ["en", "auto", "vi"] {
            let previous = ui.language.clone();
            ui.language = code.into();
            language_changed(&previous, &ui.language, || save(&path, &ui).unwrap());
            let bytes = std::fs::read(&path).unwrap();
            assert_eq!(resolve(Some(&bytes), None).language, code);
        }
        language_changed("vi", "vi", || panic!("unchanged language should not save"));
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn save_failure_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked");
        std::fs::write(&path, b"keep").unwrap();
        assert!(save(&path.join("ui.json"), &UiState::default()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"keep");
    }
}
