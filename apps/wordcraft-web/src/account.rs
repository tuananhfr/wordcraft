//! The office shell's PI ID session, `window.erpAccount` (version 1), as the app's
//! [`AccountHook`](wordcraft_ui_egui::account::AccountHook). The shell owns sign-in (its popup), tokens
//! and storage; this maps its snapshot and forwards the user's clicks. Without the shell, with its
//! flag off or in embed mode (`status: 'disabled'`) there is no hook.

use serde_json::Value;
use wordcraft_ui_egui::account::{AccountError, AccountSnapshot, AccountView, LicenceView, OrgView, SignedInView, is_pro_feature};

#[cfg(target_arch = "wasm32")]
pub(crate) use bridge::connect;

/// The shell's snapshot (its JSON) as the app's view. Shapes it doesn't expect read as a guest.
pub(crate) fn snapshot_from_json(value: &Value) -> AccountSnapshot {
    let error = value.get("error").and_then(error_from_json);
    let view = match value.get("status").and_then(Value::as_str) {
        Some("signing-in") => AccountView::SigningIn,
        Some("signed-in") => AccountView::SignedIn(signed_in(value, error)),
        _ => AccountView::Guest,
    };
    AccountSnapshot { view, busy: false, error }
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn signed_in(value: &Value, error: Option<AccountError>) -> SignedInView {
    let user = value.get("user");
    let orgs = value.get("orgs").and_then(Value::as_array).map(|orgs| orgs.iter().filter_map(org).collect()).unwrap_or_default();
    // Snapshot v1 sends the active organization as an object; a bare id is accepted too.
    let active_org = match value.get("activeOrg") {
        Some(Value::String(id)) => Some(id.clone()),
        Some(org) => text(org.get("id")),
        None => None,
    };
    let features: Vec<String> = value
        .get("entitlements")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(|e| text(e.get("feature"))).collect())
        .unwrap_or_default();
    let licence = if features.is_empty() { LicenceView::Free } else { LicenceView::Online { pro: features.iter().any(|f| is_pro_feature(f)) } };
    SignedInView {
        name: text(user.and_then(|u| u.get("name"))).unwrap_or_default(),
        email: text(user.and_then(|u| u.get("email"))).filter(|e| !e.is_empty()),
        orgs,
        active_org,
        licence,
        warning: None,
        online: error != Some(AccountError::Network),
        session_only: false,
        reauth_required: false,
    }
}

fn org(value: &Value) -> Option<OrgView> {
    let id = text(value.get("id"))?;
    let name = text(value.get("name")).unwrap_or_else(|| id.clone());
    Some(OrgView { id, name, role: text(value.get("role")).filter(|r| !r.is_empty()) })
}

fn error_from_json(value: &Value) -> Option<AccountError> {
    let code = value.get("code")?.as_str()?;
    let retry_after = value.get("retryAfter").and_then(Value::as_f64).filter(|s| s.is_finite() && *s >= 0.0).map(|s| s.ceil() as u64);
    Some(error_from_code(code, retry_after))
}

/// The shell's error codes (`editor-shell/account.js`) as the app's errors.
pub(crate) fn error_from_code(code: &str, retry_after_secs: Option<u64>) -> AccountError {
    match code {
        "cancelled" => AccountError::Cancelled,
        "network" | "discovery_failed" => AccountError::Network,
        "rate_limited" => AccountError::RateLimited { retry_after_secs },
        "dependency_unavailable" => AccountError::ServiceUnavailable,
        "session_expired" | "insufficient_scope" | "unauthenticated" | "invalid_grant" => AccountError::ReauthRequired,
        "user_suspended" => AccountError::Suspended,
        "membership_inactive" | "unknown_org" => AccountError::OrgUnavailable,
        "forbidden" | "quota_exceeded" => AccountError::Denied,
        _ => AccountError::Other,
    }
}

#[cfg(target_arch = "wasm32")]
mod bridge {
    use std::cell::RefCell;
    use std::rc::Rc;

    use serde_json::Value;
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use wordcraft_ui_egui::account::{AccountError, AccountHook, AccountSnapshot, is_pro_feature};

    use super::{error_from_code, snapshot_from_json};

    #[derive(Default)]
    struct State {
        snapshot: AccountSnapshot,
        /// Promises of `signIn`/`signOut`/`selectOrg` not settled yet.
        pending: u32,
        ctx: Option<egui::Context>,
    }

    /// Callbacks can run while the app is calling into the shell (it notifies synchronously), so
    /// nothing holds a borrow across a call and a busy state is skipped rather than panicking.
    fn update(state: &Rc<RefCell<State>>, f: impl FnOnce(&mut State)) {
        let ctx = match state.try_borrow_mut() {
            Ok(mut state) => {
                f(&mut state);
                state.snapshot.busy = state.pending > 0;
                state.ctx.clone()
            }
            Err(_) => None,
        };
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }

    fn to_json(value: &JsValue) -> Option<Value> {
        let text = js_sys::JSON::stringify(value).ok()?.as_string()?;
        serde_json::from_str(&text).ok()
    }

    fn call(api: &JsValue, name: &str, arg: Option<&JsValue>) -> Option<JsValue> {
        let function = js_sys::Reflect::get(api, &JsValue::from_str(name)).ok()?.dyn_into::<js_sys::Function>().ok()?;
        let result = match arg {
            None => function.call0(api),
            Some(arg) => function.call1(api, arg),
        };
        result.map_err(|error| log::warn!("erpAccount.{name} failed: {error:?}")).ok()
    }

    pub(crate) struct WebAccount {
        api: JsValue,
        state: Rc<RefCell<State>>,
        /// `onChange` keeps calling it for the page's lifetime.
        _listener: Closure<dyn FnMut(JsValue)>,
    }

    /// The hook when the shell offers an enabled v1 `erpAccount`.
    pub(crate) fn connect() -> Option<Box<dyn AccountHook>> {
        let window = web_sys::window()?;
        let api = js_sys::Reflect::get(&window, &JsValue::from_str("erpAccount")).ok()?;
        if !api.is_object() || js_sys::Reflect::get(&api, &JsValue::from_str("version")).ok()?.as_f64() != Some(1.0) {
            return None;
        }
        let first = to_json(&call(&api, "snapshot", None)?)?;
        if first.get("status").and_then(Value::as_str) == Some("disabled") {
            return None;
        }
        let state = Rc::new(RefCell::new(State { snapshot: snapshot_from_json(&first), ..State::default() }));
        let listened = state.clone();
        let listener = Closure::<dyn FnMut(JsValue)>::new(move |snapshot: JsValue| {
            if let Some(json) = to_json(&snapshot) {
                update(&listened, |state| state.snapshot = snapshot_from_json(&json));
            }
        });
        call(&api, "onChange", Some(listener.as_ref()))?;
        Some(Box::new(WebAccount { api, state, _listener: listener }))
    }

    impl WebAccount {
        /// Calls a shell method returning a promise of its snapshot; the snapshot it settles with replaces ours.
        fn run(&self, name: &str, arg: Option<&JsValue>) {
            let Some(promise) = call(&self.api, name, arg).and_then(|r| r.dyn_into::<js_sys::Promise>().ok()) else {
                update(&self.state, |state| state.snapshot.error = Some(AccountError::Other));
                return;
            };
            update(&self.state, |state| state.pending += 1);
            let state = self.state.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let outcome = wasm_bindgen_futures::JsFuture::from(promise).await;
                update(&state, |state| {
                    state.pending = state.pending.saturating_sub(1);
                    match outcome {
                        Ok(snapshot) => {
                            if let Some(json) = to_json(&snapshot) {
                                state.snapshot = snapshot_from_json(&json);
                            }
                        }
                        Err(error) => {
                            let code = js_sys::Reflect::get(&error, &JsValue::from_str("code")).ok().and_then(|c| c.as_string());
                            state.snapshot.error = Some(code.map_or(AccountError::Other, |c| error_from_code(&c, None)));
                        }
                    }
                });
            });
        }
    }

    impl AccountHook for WebAccount {
        fn snapshot(&self) -> AccountSnapshot {
            self.state.try_borrow().map(|state| state.snapshot.clone()).unwrap_or_default()
        }

        // Runs in the click's frame: the shell opens its popup synchronously, as browsers require.
        fn sign_in(&self) {
            self.run("signIn", None);
        }

        /// Closing the popup cancels; the shell notices that itself.
        fn cancel_sign_in(&self) {}

        fn sign_out(&self) {
            self.run("signOut", None);
        }

        fn select_org(&self, org_id: Option<String>) {
            self.run("selectOrg", Some(&org_id.map_or(JsValue::NULL, |id| JsValue::from_str(&id))));
        }

        /// The web has no device activation.
        fn retry_activation(&self) {}

        fn allowed(&self, feature: &str) -> bool {
            call(&self.api, "allowed", Some(&JsValue::from_str(feature))).and_then(|r| r.as_bool()).unwrap_or(!is_pro_feature(feature))
        }

        /// The web has no lease, so no expiry warning to record.
        fn warning_shown(&self, _days: u32) {}

        fn attach(&self, ctx: &egui::Context) {
            if let Ok(mut state) = self.state.try_borrow_mut() {
                state.ctx = Some(ctx.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wordcraft_ui_egui::account::{AccountError, AccountView, LicenceView, OrgView};

    use super::{error_from_code, snapshot_from_json};

    fn signed_in(extra: serde_json::Value) -> serde_json::Value {
        let mut base = json!({
            "version": 1,
            "status": "signed-in",
            "user": {"id": "usr_1", "name": "Nguyễn An", "email": "an@example.com"},
            "orgs": [{"id": "org_a", "name": "Công ty A", "role": "admin"}, {"id": "org_b", "name": "Công ty B", "role": null}],
            "activeOrg": {"id": "org_b", "name": "Công ty B", "role": null},
            "product": "vanban",
            "entitlements": [],
            "entitlementsFresh": true,
            "error": null
        });
        if let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        base
    }

    #[test]
    fn a_signed_in_snapshot_maps_user_orgs_and_licence() {
        let snapshot = snapshot_from_json(&signed_in(json!({})));
        assert_eq!(snapshot.error, None);
        let AccountView::SignedIn(view) = snapshot.view else { panic!("signed in") };
        assert_eq!(view.name, "Nguyễn An");
        assert_eq!(view.email.as_deref(), Some("an@example.com"));
        assert_eq!(
            view.orgs,
            vec![
                OrgView { id: "org_a".into(), name: "Công ty A".into(), role: Some("admin".into()) },
                OrgView { id: "org_b".into(), name: "Công ty B".into(), role: None },
            ]
        );
        assert_eq!(view.active_org.as_deref(), Some("org_b"));
        assert_eq!(view.licence, LicenceView::Free);
        assert!(view.online && view.warning.is_none());

        let free = signed_in(json!({"activeOrg": null, "entitlements": [{"id": "e1", "feature": "vanban.use"}]}));
        let AccountView::SignedIn(view) = snapshot_from_json(&free).view else { panic!("signed in") };
        assert_eq!(view.active_org, None, "personal context");
        assert_eq!(view.licence, LicenceView::Online { pro: false });
        let pro = signed_in(json!({"activeOrg": "org_a", "entitlements": [{"feature": "vanban.use"}, {"feature": "vanban.advanced"}]}));
        let AccountView::SignedIn(view) = snapshot_from_json(&pro).view else { panic!("signed in") };
        assert_eq!(view.active_org.as_deref(), Some("org_a"), "a bare id is accepted");
        assert_eq!(view.licence, LicenceView::Online { pro: true });
    }

    #[test]
    fn statuses_errors_and_odd_shapes() {
        assert_eq!(snapshot_from_json(&json!({"status": "signed-out"})).view, AccountView::Guest);
        assert_eq!(snapshot_from_json(&json!({"status": "signing-in"})).view, AccountView::SigningIn);
        for odd in [json!(null), json!(42), json!({"status": 3}), json!({"status": "who-knows"}), json!([])] {
            let snapshot = snapshot_from_json(&odd);
            assert_eq!((snapshot.view, snapshot.error), (AccountView::Guest, None), "{odd}");
        }
        let odd_user = snapshot_from_json(&json!({"status": "signed-in", "user": 7, "orgs": [1, {"name": "no id"}], "entitlements": "x"}));
        let AccountView::SignedIn(view) = odd_user.view else { panic!("signed in") };
        assert_eq!((view.name.as_str(), view.email, view.orgs.len(), view.licence), ("", None, 0, LicenceView::Free));

        let limited = snapshot_from_json(&json!({"status": "signed-out", "error": {"code": "rate_limited", "message": "…", "retryAfter": 89.2}}));
        assert_eq!(limited.error, Some(AccountError::RateLimited { retry_after_secs: Some(90) }));
        let offline = signed_in(json!({"error": {"code": "network", "message": "…", "retryAfter": null}}));
        let snapshot = snapshot_from_json(&offline);
        assert_eq!(snapshot.error, Some(AccountError::Network));
        let AccountView::SignedIn(view) = snapshot.view else { panic!("signed in") };
        assert!(!view.online);
    }

    #[test]
    fn every_shell_error_code_maps() {
        let cases = [
            ("cancelled", AccountError::Cancelled),
            ("network", AccountError::Network),
            ("discovery_failed", AccountError::Network),
            ("rate_limited", AccountError::RateLimited { retry_after_secs: None }),
            ("dependency_unavailable", AccountError::ServiceUnavailable),
            ("session_expired", AccountError::ReauthRequired),
            ("insufficient_scope", AccountError::ReauthRequired),
            ("user_suspended", AccountError::Suspended),
            ("membership_inactive", AccountError::OrgUnavailable),
            ("unknown_org", AccountError::OrgUnavailable),
            ("forbidden", AccountError::Denied),
            ("quota_exceeded", AccountError::Denied),
            ("popup_blocked", AccountError::Other),
            ("storage", AccountError::Other),
            ("state_mismatch", AccountError::Other),
            ("sign_in_failed", AccountError::Other),
            ("something new", AccountError::Other),
        ];
        for (code, expected) in cases {
            assert_eq!(error_from_code(code, None), expected, "{code}");
        }
    }
}
