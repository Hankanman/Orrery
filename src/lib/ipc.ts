import { invoke } from "@tauri-apps/api/core";
import type { Repo } from "@/types";

/** Mirrors the Rust `AppConfig` (see src-tauri/src/model.rs). */
export interface AppConfig {
  roots: string[];
  scanDepth: number;
  ignore: string[];
  ideCommand: string;
  agentCommand: string;
  githubClientId: string;
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
};
