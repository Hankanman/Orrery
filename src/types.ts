// Shared domain types. These mirror the shapes the Rust core will return over
// IPC (see DESIGN.md > Architecture). Kept in one place so the eventual
// `serde`-serialized structs and the UI stay in lockstep.

export type Activity = "active" | "idle" | "stale";

/** Derived working-tree state, drives the card's status accent. */
export type RepoStatus = "clean" | "dirty" | "behind" | "stale";

/** Where a repo's remote lives (host stats + icon on the card). */
export type Host = "github" | "gitlab" | null;

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
  /** Root grouping (the scanned parent dir, e.g. "~/dev/personal"). */
  root: string;
  /** Origin host, if the repo has a remote. */
  host: Host;
  /** Remote host domain (e.g. "github.com", "gitlab.acme.io"), if any. */
  remoteHost?: string | null;
  /** Host star count (0 if none / no remote). */
  stars: number;
  /** Host topics/labels (enrichment). */
  topics?: string[];
  /** Open issues on the host (enrichment). */
  openIssues?: number;
  /** Latest release tag on the host (enrichment). */
  latestRelease?: string | null;
  /** User-favorited (host amber star on the card). */
  favorite: boolean;
  /** Local-AI synthesized blurb (Phase 3); presence lights the violet indicator. */
  aiSummary: string | null;
}
