//! The shell's user-facing actions, grouped by surface. Each module is an
//! `impl OrreryApp` block: the handlers views and overlays call, plus the
//! async work (network, git, AI) they kick off. The repo drawer's actions
//! live with the drawer (`crate::drawer::actions`).

mod agents;
mod ai;
mod cleanup;
mod explore;
mod github;
mod palette;
mod settings;
mod views;

pub(crate) use github::draft_and_create_pr;
