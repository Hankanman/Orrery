//! Shared domain types serialized to the frontend. These mirror `src/types.ts`
//! (camelCase over the wire) and `AppConfig` mirrors the TOML on disk.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    Active,
    Idle,
    Stale,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Github,
    Gitlab,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    /// Count of uncommitted changes in the working tree.
    pub dirty: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// Stable id — absolute path on disk.
    pub id: String,
    /// Human display name: README H1 → slug → directory name.
    pub display_name: String,
    /// owner/repo slug parsed from the origin remote, if any.
    pub slug: Option<String>,
    /// Absolute path, abbreviated with ~ for display.
    pub path: String,
    /// First line/paragraph of the README, if present.
    pub description: Option<String>,
    /// Detected primary language (heuristic).
    pub language: Option<String>,
    pub git: GitStatus,
    /// Seconds since the Unix epoch (UTC) of the last commit.
    pub last_commit_unix: i64,
    pub activity: Activity,
    /// The scanned root this repo was found under (abbreviated).
    pub root: String,
    /// Origin host, if the repo has a recognized remote.
    pub host: Option<Host>,
    /// Host star count (filled in Phase 2; 0 for now).
    pub stars: u32,
    /// User-favorited (persisted locally).
    pub favorite: bool,
    /// Local-AI summary (Phase 3).
    pub ai_summary: Option<String>,
}

/// User configuration, persisted as TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// Directories scanned for git repos.
    pub roots: Vec<String>,
    /// How deep to descend into each root looking for `.git`.
    pub scan_depth: usize,
    /// Directory names/globs skipped while scanning.
    pub ignore: Vec<String>,
    /// Command template to open a repo in the IDE. `{path}` is substituted.
    pub ide_command: String,
    /// Command template to open a terminal coding agent in the repo.
    pub agent_command: String,
}
