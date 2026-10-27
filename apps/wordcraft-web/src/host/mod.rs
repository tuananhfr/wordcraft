//! The office shell's online documents, `window.erpHost` v1 (contract `erp-host.md` in
//! erpcons-office): on only when the shell defines it, at version 1, before the app starts.
//! The saving itself is [`wordcraft_ui_egui::host_docs`]; this wires it to the page.

#[cfg(target_arch = "wasm32")]
mod bridge;
#[cfg(target_arch = "wasm32")]
mod page;
pub(crate) mod result;

#[cfg(target_arch = "wasm32")]
pub(crate) use page::{Host, connect};
