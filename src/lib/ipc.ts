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
}

export interface SearchHit {
  id: string;
  score: number;
}

export interface Briefing {
  text: string;
  repoCount: number;
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

export interface AiStatus {
  available: boolean;
  model: string | null;
  models: string[];
}

export interface HostInfo {
  stars: number;
  topics: string[];
  openIssues: number;
  latestRelease: string | null;
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
  openAgent: (id: string) => invoke<void>("open_agent", { id }),
  enrichRepo: (host: "github" | "gitlab", domain: string, slug: string) =>
    invoke<HostInfo>("enrich_repo", { host, domain, slug }),
  githubLoginStart: () => invoke<DeviceStart>("github_login_start"),
  githubLoginPoll: (deviceCode: string) => invoke<{ status: string }>("github_login_poll", { deviceCode }),
  githubAuthStatus: () => invoke<boolean>("github_auth_status"),
  githubSignOut: () => invoke<void>("github_sign_out"),
  aiStatus: () => invoke<AiStatus>("ai_status"),
  summarizeRepo: (repo: Repo, refresh = false) => invoke<string>("summarize_repo", { repo, refresh }),
  fetchAll: (ids: string[]) => invoke<FetchOutcome[]>("fetch_all", { ids }),
  fetchRepo: (id: string) => invoke<GitStatus>("fetch_repo", { id }),
  listBranches: (id: string) => invoke<BranchInfo[]>("list_branches", { id }),
  switchBranch: (id: string, name: string) => invoke<void>("switch_branch", { id, name }),
  pruneBranches: (id: string) => invoke<string[]>("prune_branches", { id }),
  listWorktrees: (id: string) => invoke<WorktreeInfo[]>("list_worktrees", { id }),
  addWorktree: (id: string, name: string, dest: string) => invoke<string>("add_worktree", { id, name, dest }),
  removeWorktree: (id: string, name: string) => invoke<void>("remove_worktree", { id, name }),
  repoLog: (id: string, limit = 20) => invoke<CommitInfo[]>("repo_log", { id, limit }),
  repoDiff: (id: string) => invoke<string>("repo_diff", { id }),
  repoStagedDiff: (id: string) => invoke<string>("repo_staged_diff", { id }),
  repoReadme: (id: string) => invoke<string | null>("repo_readme", { id }),
  generateCommitMessage: (id: string) => invoke<string>("generate_commit_message", { id }),
  commitStaged: (id: string, message: string) => invoke<string>("commit_staged", { id, message }),
  generateChangelog: (id: string, limit = 20) => invoke<string>("generate_changelog", { id, limit }),
  indexRepos: (repos: Repo[]) => invoke<number>("index_repos", { repos }),
  semanticSearch: (query: string) => invoke<SearchHit[]>("semantic_search", { query }),
  dailyBriefing: (repos: Repo[]) => invoke<Briefing>("daily_briefing", { repos }),
  getInbox: () => invoke<InboxItem[]>("get_inbox"),
  getNotifications: () => invoke<NotificationItem[]>("get_notifications"),
  ciStatus: (slug: string) => invoke<CiStatus>("ci_status", { slug }),
  listStarred: () => invoke<RemoteRepo[]>("list_starred"),
  cloneRepo: (url: string, destRoot: string) => invoke<string>("clone_repo", { url, destRoot }),
  activeAgents: () => invoke<string[]>("active_agents"),
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
};
