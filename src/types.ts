// Shared domain types. These mirror the shapes the Rust core will return over
// IPC (see DESIGN.md > Architecture). Kept in one place so the eventual
// `serde`-serialized structs and the UI stay in lockstep.

export type Activity = "active" | "idle" | "stale";

export interface GitStatus {
  branch: string;
  ahead: number;
  behind: number;
  /** Count of uncommitted changes in the working tree. */
  dirty: number;
}

export interface Repo {
  /** Stable id — absolute path on disk for now. */
  id: string;
  /** Human display name: README H1 → host description → slug → dir name. */
  displayName: string;
  /** owner/repo slug parsed from the origin remote, if any. */
  slug: string | null;
  /** Absolute path, shown abbreviated (~/dev/...). */
  path: string;
  /** First line / paragraph of the README, if present. */
  description: string | null;
  /** Detected primary language (heuristic in Phase 1). */
  language: string | null;
  git: GitStatus;
  /** Seconds since the last commit (UTC). UI derives "4h ago". */
  lastCommitUnix: number;
  activity: Activity;
}
