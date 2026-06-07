import { invoke } from "@tauri-apps/api/core";
import type { GitStatus, Repo } from "@/types";

export interface FetchOutcome {
  id: string;
  status: GitStatus | null;
  error: string | null;
}

export interface BranchInfo {
  name: string;
  isHead: boolean;
  upstream: string | null;
  gone: boolean;
  merged: boolean;
}

/** Prunable branches for one repo (branch janitor). */
export interface RepoPrunable {
  id: string;
  branches: BranchInfo[];
}

export interface CommitInfo {
  id: string;
  summary: string;
  author: string;
  timeUnix: number;
}

export interface WorktreeInfo {
  name: string;
  path: string;
}

/** One day's commit count, keyed by epoch day (author-local). */
export interface DayCount {
  day: number;
  count: number;
}

/** Mirrors the Rust `AppConfig` (see src-tauri/src/model.rs). */
export interface AppConfig {
  roots: string[];
  scanDepth: number;
  ignore: string[];
  ideCommand: string;
  agentCommand: string;
  githubClientId: string;
  gitlabHosts: string[];
  aiModel: string;
  aiEnabled: boolean;
  embedModel: string;
  ollamaHost: string;
  notifyEnabled: boolean;
  notifyNewPr: boolean;
  notifyReviewRequested: boolean;
  notifyCiFailure: boolean;
}

export interface SearchHit {
  id: string;
  score: number;
}

/** A ripgrep content-search hit (cross-repo code search). */
export interface CodeHit {
  repo: string;
  file: string;
  abs: string;
  line: number;
  text: string;
}

export interface Briefing {
  text: string;
  repoCount: number;
}

export interface ResumeSummary {
  /** AI catch-up text; empty when AI is off or nothing changed. */
  text: string;
  /** Commits since the user last looked. */
  commitCount: number;
  /** First time opening this repo — no prior cursor, nothing to catch up on. */
  firstVisit: boolean;
}

export interface InboxItem {
  kind: "pr" | "review" | "issue";
  title: string;
  repo: string;
  url: string;
  number: number;
  draft: boolean;
  host: "github" | "gitlab";
}

export interface NotificationItem {
  title: string;
  repo: string;
  reason: string;
  kind: string;
}

export interface RemoteRepo {
  slug: string;
  description: string | null;
  stars: number;
  language: string | null;
  cloneUrl: string;
  host: "github" | "gitlab";
}

export interface CiStatus {
  state: "success" | "failure" | "pending" | "none";
}

export interface CheckRun {
  name: string;
  state: "success" | "failure" | "pending" | "neutral";
  url: string | null;
}

export interface PrReview {
  author: string;
  state: "approved" | "changes_requested";
}

export interface PrDetail {
  number: number;
  title: string;
  url: string;
  draft: boolean;
  base: string;
  head: string;
  author: string | null;
  mergeable: "clean" | "conflicting" | "unknown";
  reviewDecision: "approved" | "changes_requested" | "review_required" | "none";
  checksState: "success" | "failure" | "pending" | "none";
  checks: CheckRun[];
  reviews: PrReview[];
}

export type MergeMethod = "squash" | "rebase" | "merge";

export interface PrPanel {
  mergeMethods: MergeMethod[];
  prs: PrDetail[];
}

export interface FeedItem {
  kind: "release" | "starred" | "created" | "forked" | "public";
  actor: string | null;
  repo: string;
  title: string;
  tag: string;
  detail: string;
  url: string;
  timestamp: number;
  prerelease: boolean;
  host: "github" | "gitlab";
}

export interface AiStatus {
  /** Ollama server reachable at `endpoint`. */
  reachable: boolean;
  /** Summaries enabled in config. */
  enabled: boolean;
  /** Ollama base URL in use. */
  endpoint: string;
  /** Chat model that would actually be used. */
  model: string | null;
  /** Configured embedding model. */
  embedModel: string;
  /** Whether the embedding model is installed. */
  embedInstalled: boolean;
  /** Installed model names. */
  models: string[];
  /** Reason it's unusable, if any. */
  error: string | null;
}

export interface AiTest {
  chatOk: boolean;
  embedOk: boolean;
  ms: number;
  error: string | null;
}

export interface ClearResult {
  summaries: number;
  embeddings: number;
}

export interface HostInfo {
  stars: number;
  topics: string[];
  openIssues: number;
  latestRelease: string | null;
  private: boolean;
}

export interface DeviceStart {
  userCode: string;
  verificationUri: string;
  deviceCode: string;
  interval: number;
}

/** True when running inside the Tauri webview (vs. a plain browser preview). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export const ipc = {
  getConfig: () => invoke<AppConfig>("get_config"),
  setConfig: (config: AppConfig) => invoke<void>("set_config", { config }),
  cachedRepos: () => invoke<Repo[]>("cached_repos"),
  scanRepos: () => invoke<Repo[]>("scan_repos"),
  setFavorite: (id: string, favorite: boolean) => invoke<boolean>("set_favorite", { id, favorite }),
  openInIde: (id: string) => invoke<void>("open_in_ide", { id }),
  openFolder: (id: string) => invoke<void>("open_folder", { id }),
  openAgent: (id: string) => invoke<void>("open_agent", { id }),
  enrichRepo: (host: "github" | "gitlab", domain: string, slug: string, refresh = false) =>
    invoke<HostInfo>("enrich_repo", { host, domain, slug, refresh }),
  githubLoginStart: () => invoke<DeviceStart>("github_login_start"),
  githubLoginPoll: (deviceCode: string) => invoke<{ status: string }>("github_login_poll", { deviceCode }),
  githubAuthStatus: () => invoke<boolean>("github_auth_status"),
  githubSignOut: () => invoke<void>("github_sign_out"),
  aiStatus: () => invoke<AiStatus>("ai_status"),
  aiTest: () => invoke<AiTest>("ai_test"),
  pullModel: (model: string) => invoke<void>("pull_model", { model }),
  clearAiCache: () => invoke<ClearResult>("clear_ai_cache"),
  summarizeRepo: (repo: Repo, refresh = false) => invoke<string>("summarize_repo", { repo, refresh }),
  fetchAll: (ids: string[]) => invoke<FetchOutcome[]>("fetch_all", { ids }),
  fetchRepo: (id: string) => invoke<GitStatus>("fetch_repo", { id }),
  listBranches: (id: string) => invoke<BranchInfo[]>("list_branches", { id }),
  switchBranch: (id: string, name: string) => invoke<void>("switch_branch", { id, name }),
  pruneBranches: (id: string) => invoke<string[]>("prune_branches", { id }),
  prunableBranches: (paths: string[]) => invoke<RepoPrunable[]>("prunable_branches", { paths }),
  listWorktrees: (id: string) => invoke<WorktreeInfo[]>("list_worktrees", { id }),
  addWorktree: (id: string, name: string, dest: string) => invoke<string>("add_worktree", { id, name, dest }),
  removeWorktree: (id: string, name: string) => invoke<void>("remove_worktree", { id, name }),
  repoLog: (id: string, limit = 20) => invoke<CommitInfo[]>("repo_log", { id, limit }),
  contributionGraph: (ids: string[]) => invoke<DayCount[]>("contribution_graph", { ids }),
  repoDiff: (id: string) => invoke<string>("repo_diff", { id }),
  repoStagedDiff: (id: string) => invoke<string>("repo_staged_diff", { id }),
  repoReadme: (id: string) => invoke<string | null>("repo_readme", { id }),
  generateCommitMessage: (id: string) => invoke<string>("generate_commit_message", { id }),
  commitStaged: (id: string, message: string) => invoke<string>("commit_staged", { id, message }),
  generateChangelog: (id: string, limit = 20) => invoke<string>("generate_changelog", { id, limit }),
  getNote: (id: string) => invoke<string>("get_note", { id }),
  setNote: (id: string, text: string) => invoke<void>("set_note", { id, text }),
  markSeen: (id: string) => invoke<void>("mark_seen", { id }),
  resumeSummary: (id: string) => invoke<ResumeSummary>("resume_summary", { id }),
  indexRepos: (repos: Repo[]) => invoke<number>("index_repos", { repos }),
  semanticSearch: (query: string) => invoke<SearchHit[]>("semantic_search", { query }),
  searchCode: (query: string, paths: string[]) => invoke<CodeHit[]>("search_code", { query, paths }),
  dailyBriefing: (repos: Repo[]) => invoke<Briefing>("daily_briefing", { repos }),
  getInbox: () => invoke<InboxItem[]>("get_inbox"),
  getNotifications: () => invoke<NotificationItem[]>("get_notifications"),
  ciStatus: (slug: string) => invoke<CiStatus>("ci_status", { slug }),
  prPanel: (slug: string, refresh = false) => invoke<PrPanel>("pr_panel", { slug, refresh }),
  mergePr: (slug: string, number: number, method: MergeMethod) =>
    invoke<void>("merge_pr", { slug, number, method }),
  approvePr: (slug: string, number: number) => invoke<void>("approve_pr", { slug, number }),
  listStarred: () => invoke<RemoteRepo[]>("list_starred"),
  getFeed: (refresh = false) => invoke<FeedItem[]>("get_feed", { refresh }),
  cloneRepo: (url: string, destRoot: string) => invoke<string>("clone_repo", { url, destRoot }),
  activeAgents: () => invoke<string[]>("active_agents"),
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
};
