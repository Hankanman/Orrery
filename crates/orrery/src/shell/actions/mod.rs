//! The shell's user-facing actions, grouped by surface. Each module is an
//! `impl OrreryApp` block: the handlers views and overlays call, plus the
//! async work (network, git, AI) they kick off.

mod ai;
mod palette;
