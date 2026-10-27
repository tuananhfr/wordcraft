//! The host's `SaveResult`, read as JSON so it can be checked without a browser.

use serde_json::Value;
use wordcraft_ui_egui::host_docs::SaveOutcome;

/// Largest integer a JS number holds exactly.
pub(crate) const MAX_SAFE: u64 = 9_007_199_254_740_991;

/// A JS number that is a whole, non-negative, exactly representable count.
pub(crate) fn whole(n: f64) -> Option<u64> {
    (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= MAX_SAFE as f64).then_some(n as u64)
}

fn number(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_f64).and_then(whole)
}

/// `None` for anything the contract doesn't allow.
pub(crate) fn save_outcome(v: &Value) -> Option<SaveOutcome> {
    match v.get("status")?.as_str()? {
        "saved" => Some(SaveOutcome::Saved { version: number(v, "version")? }),
        "conflict" => Some(SaveOutcome::Conflict { latest_version: number(v, "latestVersion")? }),
        "offline" => Some(SaveOutcome::Offline),
        "failed" => Some(SaveOutcome::Failed {
            retryable: v.get("retryable")?.as_bool()?,
            message: v.get("message").and_then(Value::as_str).unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

/// A result outside the contract (a rejected promise, a malformed answer): keep the document
/// unsaved and AutoSave on, as for a retryable failure, rather than stop or claim it saved.
pub(crate) fn unreadable() -> SaveOutcome {
    SaveOutcome::Failed { retryable: true, message: String::new() }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_save_result_the_contract_allows_is_read() {
        assert_eq!(save_outcome(&json!({"status": "saved", "version": 4})), Some(SaveOutcome::Saved { version: 4 }));
        assert_eq!(save_outcome(&json!({"status": "conflict", "latestVersion": 9})), Some(SaveOutcome::Conflict { latest_version: 9 }));
        assert_eq!(save_outcome(&json!({"status": "offline"})), Some(SaveOutcome::Offline));
        assert_eq!(
            save_outcome(&json!({"status": "failed", "retryable": false, "message": "Tệp quá lớn"})),
            Some(SaveOutcome::Failed { retryable: false, message: "Tệp quá lớn".into() })
        );
        assert_eq!(
            save_outcome(&json!({"status": "failed", "retryable": true, "message": "Máy chủ bận"})),
            Some(SaveOutcome::Failed { retryable: true, message: "Máy chủ bận".into() })
        );
    }

    #[test]
    fn malformed_results_are_not_read_as_anything() {
        for v in [
            json!(null),
            json!("saved"),
            json!({}),
            json!({"status": "ok", "version": 1}),
            json!({"status": "saved"}),
            json!({"status": "saved", "version": -1}),
            json!({"status": "saved", "version": 1.5}),
            json!({"status": "saved", "version": "2"}),
            json!({"status": "conflict"}),
            json!({"status": "failed", "message": "x"}),
        ] {
            assert_eq!(save_outcome(&v), None, "{v}");
        }
        assert!(matches!(unreadable(), SaveOutcome::Failed { retryable: true, .. }));
    }

    #[test]
    fn whole_numbers_fit_a_js_number() {
        assert_eq!(whole(0.0), Some(0));
        assert_eq!(whole(MAX_SAFE as f64), Some(MAX_SAFE));
        assert_eq!(whole(f64::NAN), None);
        assert_eq!(whole(f64::INFINITY), None);
        assert_eq!(whole(1e300), None);
    }
}
