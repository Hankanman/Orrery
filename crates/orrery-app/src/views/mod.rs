//! The non-grid views rendered in the shell's main column (Inbox, Feed, Explore,
//! Agents, Dev Tools, Cleanup, Settings). Each owns its render + any async state,
//! loaded lazily when its nav item is selected.

pub mod inbox;
