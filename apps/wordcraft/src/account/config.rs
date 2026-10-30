//! The account configuration of this build: where PI is and which keys verify its leases.

use erpcons_account::{Config, Product};

// A release that cannot verify a lease would lock every user out of editing.
#[cfg(not(debug_assertions))]
const _: () = assert!(!erpcons_account::PLATFORM_KEYS.is_empty());

/// `None` unless the build names a PI issuer: `ERPCONS_PI_ISSUER` (and optionally
/// `ERPCONS_PI_API_BASE`) at compile time, set by the release build scripts.
pub(super) fn from_build() -> Option<Config> {
    let issuer = option_env!("ERPCONS_PI_ISSUER").filter(|issuer| !issuer.trim().is_empty())?;
    let mut config = for_issuer(issuer, env!("CARGO_PKG_VERSION"));
    config.api_base = option_env!("ERPCONS_PI_API_BASE").filter(|base| !base.trim().is_empty()).map(Into::into);
    #[cfg(debug_assertions)]
    add_dev_lease_key(&mut config);
    Some(config)
}

pub(super) fn for_issuer(issuer: &str, client_version: &str) -> Config {
    let mut config = Config::new(issuer, Product::VanBan, client_version);
    // Editing needs a lease from PI's device protocol (erpcons-account ADR 0013); release builds
    // verify it with the pinned `PLATFORM_KEYS` only.
    config.offline_licensing = true;
    config
}

/// Debug builds only: `ERPCONS_PI_DEV_LEASE_KEY` (`kid:hex32`) trusts a development lease key
/// such as pi-mock's, to run the app against it. Release builds never read it.
#[cfg(debug_assertions)]
fn add_dev_lease_key(config: &mut Config) {
    let Some(value) = std::env::var("ERPCONS_PI_DEV_LEASE_KEY").ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) else { return };
    match parse_dev_lease_key(&value) {
        Some(key) => config.platform_keys.push(key),
        None => log::warn!("ERPCONS_PI_DEV_LEASE_KEY is not kid:<64 hex digits>; ignored"),
    }
}

/// `kid:hex32`: a lease-signing key id and its Ed25519 public key in 64 hex digits.
#[cfg(debug_assertions)]
pub(super) fn parse_dev_lease_key(value: &str) -> Option<erpcons_account::PinnedKey> {
    let (kid, hex) = value.split_once(':')?;
    // `from_str_radix` alone would accept a leading `+`.
    if kid.is_empty() || hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut key = [0u8; 32];
    for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(erpcons_account::PinnedKey { kid: kid.to_owned(), key })
}
