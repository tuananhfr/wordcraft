//! The interface language on the ERPCons office site: Vietnamese until the visitor picks another
//! one, remembered in one localStorage key every editor of the site reads, so a choice made in
//! one editor carries over to the others.

const KEY: &str = "erpcons-ui-language";
const DEFAULT: &str = "vi";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The language setting to start with: the visitor's last choice, else Vietnamese.
pub fn initial() -> String {
    storage().and_then(|s| s.get_item(KEY).ok().flatten()).filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT.into())
}

/// Remember a language the visitor picked; storage may be off (private mode), which only costs the
/// choice on reload.
pub fn remember(setting: &str) {
    if let Some(s) = storage()
        && let Err(e) = s.set_item(KEY, setting)
    {
        log::warn!("couldn't remember the interface language: {e:?}");
    }
}
