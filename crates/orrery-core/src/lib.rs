//! orrery-core — Orrery's logic, free of any UI.
//!
//! The GPUI app (`crates/orrery`) and `orrery-platform` use these modules
//! directly as `orrery_core::*`.

pub mod activity;
pub mod ai;
pub mod attention;
pub mod cache;
pub mod ci;
pub mod config;
pub mod dispatch;
pub mod enrich;
pub mod fleet;
pub mod forge;
pub mod git_ops;
pub mod http;
pub mod inbox;
pub mod launch;
pub mod llama;
pub mod model;
pub mod oauth;
pub mod scan;
pub mod search;
pub mod semantic;
pub mod summarize;
