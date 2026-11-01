//! The office shell's online documents, `window.erpHost` v1 and v2 (contract `erp-host.md` in
//! erpcons-office): on only when the shell defines it, at version 1 or 2, before the app starts.
//! A shell that also has `regionWillEdit` (v3) gets region co-editing (`regions`).
//! The saving and the access modes are [`wordcraft_ui_egui::host_docs`]; this wires them to the
//! page.

pub(crate) mod access;
#[cfg(target_arch = "wasm32")]
mod bridge;
#[cfg(target_arch = "wasm32")]
mod page;
#[cfg(target_arch = "wasm32")]
mod regions;
pub(crate) mod result;

#[cfg(target_arch = "wasm32")]
pub(crate) use page::{Host, connect};
