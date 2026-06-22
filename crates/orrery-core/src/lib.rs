//! orrery-core — Orrery's logic, free of any UI or Tauri.
//!
//! Modules are re-exported as-is from the original single-crate layout, so the
//! inter-module `crate::model` / `crate::config` paths keep resolving here. The
//! legacy Tauri app re-exports these under its own `crate::` namespace; the
//! native GPUI app depends on them directly as `orrery_core::*`.

pub mod ai;
pub mod cache;
pub mod config;
pub mod forge;
pub mod git_ops;
pub mod inbox;
pub mod launch;
pub mod llama;
pub mod model;
pub mod oauth;
pub mod scan;
pub mod search;
pub mod semantic;
