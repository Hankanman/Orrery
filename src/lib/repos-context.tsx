import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ipc, isTauri, type AiStatus } from "@/lib/ipc";
import { detectAgent, detectIde } from "@/lib/launchers";
import { MOCK_REPOS } from "@/lib/mock-repos";
import type { Repo } from "@/types";

interface ReposContextValue {
  repos: Repo[];
  /** A scan is in flight. */
  loading: boolean;
  /** First load finished (cache or scan) — distinguishes empty from not-yet-loaded. */
  ready: boolean;
  error: string | null;
  /** Epoch ms of the last successful scan, or null. */
  lastScan: number | null;
  /** A fetch-all is in flight. */
  fetching: boolean;
  /** Repo ids with a running terminal-agent session. */
  activeAgents: string[];
  /** Repo ids with an on-demand AI summary in flight. */
  summarizing: string[];
  /** AI is enabled, reachable, and has a usable chat model — gate all AI UI on
   *  this so features are hidden (not broken) when AI is off/misconfigured. */
  aiReady: boolean;
  /** Re-query Ollama status (call after changing AI settings). */
  refreshAiStatus: () => void;
  refresh: () => void;
  /** Fetch every repo's origin and merge refreshed ahead/behind into the grid. */
  fetchAll: () => void;
  /** Regenerate one repo's AI summary on demand. */
  summarizeRepo: (repo: Repo) => void;
  /** Generate summaries for every repo that lacks one. */
  summarizeMissing: () => void;
  /** Drop in-memory AI summaries (after clearing the cache) so the grid reflects it. */
  clearSummaries: () => void;
  toggleFavorite: (repo: Repo) => void;
  openIde: (repo: Repo) => void;
  openAgent: (repo: Repo) => void;
  /** Brand id of the configured IDE (for the card button logo), or "". */
  ideBrand: string;
  /** Display name of the configured IDE (for the card button label), or "". */
  ideName: string;
  /** Brand id of the configured terminal agent, or "". */
  agentBrand: string;
  /** Display name of the configured terminal agent, or "". */
  agentName: string;
  /** Re-read launcher config (call after saving settings). */
  refreshLaunchers: () => void;
  /** Reveal the repo's folder in the system file manager. */
  openFolder: (repo: Repo) => void;
  /** Open the repo on its remote host (GitHub/GitLab) in the browser. */
  openHost: (repo: Repo) => void;
}

const ReposContext = createContext<ReposContextValue | null>(null);

/** Background activity, for the global progress indicator. Kept in its own
 *  context so per-batch progress ticks don't re-render repo grid consumers. */
export interface ScanStatus {
  /** A repo scan is in flight. */
  scanning: boolean;
  /** A fetch-all (git fetch across repos) is in flight. */
  fetching: boolean;
  /** Host-enrichment progress, or null when idle. */
  enrich: { done: number; total: number } | null;
  /** AI-summary progress, or null when idle. */
  summarize: { done: number; total: number } | null;
}

const ScanStatusContext = createContext<ScanStatus | null>(null);

export function ReposProvider({ children }: { children: ReactNode }) {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [loading, setLoading] = useState(false);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastScan, setLastScan] = useState<number | null>(null);
  const [fetching, setFetching] = useState(false);
  const [activeAgents, setActiveAgents] = useState<string[]>([]);
  const [enrichProgress, setEnrichProgress] = useState<{ done: number; total: number } | null>(null);
  const [summarizeProgress, setSummarizeProgress] = useState<{ done: number; total: number } | null>(null);
  // Repo ids with an on-demand summary in flight (drives the per-card spinner).
  const [summarizing, setSummarizing] = useState<string[]>([]);
  const [aiStatus, setAiStatus] = useState<AiStatus | null>(null);
  const [ideBrand, setIdeBrand] = useState("");
  const [ideName, setIdeName] = useState("");
  const [agentBrand, setAgentBrand] = useState("");
  const [agentName, setAgentName] = useState("");
  // Guards against overlapping scans (startup scan + a Rescan click racing).
  const scanning = useRef(false);
  // Generation tokens so a superseded enrich/summarize run can't clobber a newer one.
  const enrichGen = useRef(0);
  const summaryGen = useRef(0);

  // Fetch host enrichment (stars/topics/issues/release) in small concurrent
  // batches, merging each result into the matching repo. Cheap to re-run: the
  // Rust side caches for 6h and falls back to cache when offline.
  const enrichAll = useCallback((list: Repo[]) => {
    if (!isTauri()) return;
    const gen = ++enrichGen.current;
    const targets = list.filter((r) => r.host && r.slug && (r.remoteHost || r.host === "github"));
    const CHUNK = 6;
    if (targets.length === 0) {
      setEnrichProgress(null);
      return;
    }
    setEnrichProgress({ done: 0, total: targets.length });
    void (async () => {
      for (let i = 0; i < targets.length; i += CHUNK) {
        if (enrichGen.current !== gen) return; // superseded by a newer scan/enrich
        const chunk = targets.slice(i, i + CHUNK);
        const results = await Promise.all(
          chunk.map(async (r) => {
            // Enrichment and CI run in parallel per repo (independent calls).
            const [info, ci] = await Promise.all([
              ipc.enrichRepo(r.host as "github" | "gitlab", r.remoteHost ?? "github.com", r.slug!).catch(() => null),
              r.host === "github" && r.slug
                ? ipc.ciStatus(r.slug).then((c) => c.state).catch(() => null)
                : Promise.resolve(null),
            ]);
            return { id: r.id, info, ci };
          }),
        );
        setRepos((prev) => {
          if (enrichGen.current !== gen) return prev; // discard stale results
          return prev.map((r) => {
            const hit = results.find((x) => x && x.id === r.id);
            if (!hit) return r;
            const next = { ...r, ci: hit.ci ?? r.ci };
            return hit.info
              ? {
                  ...next,
                  stars: hit.info.stars,
                  topics: hit.info.topics,
                  openIssues: hit.info.openIssues,
                  latestRelease: hit.info.latestRelease,
                }
              : next;
          });
        });
        setEnrichProgress((p) =>
          enrichGen.current === gen ? { done: Math.min(i + chunk.length, targets.length), total: targets.length } : p,
        );
      }
      if (enrichGen.current === gen) setEnrichProgress(null);
    })();
  }, []);

  // Generate local AI summaries in the background (low concurrency — inference
  // is heavier than HTTP). No-ops if Ollama isn't available; cached per repo.
  const summarizeAll = useCallback((list: Repo[]) => {
    if (!isTauri()) return;
    const gen = ++summaryGen.current;
    const targets = list.filter((r) => !r.aiSummary); // skip ones we already have
    if (targets.length === 0) {
      setSummarizeProgress(null);
      return;
    }
    void (async () => {
      const status = await ipc.aiStatus().catch(() => null);
      if (!status?.reachable || !status.enabled || summaryGen.current !== gen) {
        if (summaryGen.current === gen) setSummarizeProgress(null);
        return;
      }
      setSummarizeProgress({ done: 0, total: targets.length });
      const CHUNK = 2;
      for (let i = 0; i < targets.length; i += CHUNK) {
        if (summaryGen.current !== gen) return;
        const chunk = targets.slice(i, i + CHUNK);
        const results = await Promise.all(
          chunk.map((r) =>
            ipc
              .summarizeRepo(r)
              .then((summary) => ({ id: r.id, summary }))
              .catch(() => null),
          ),
        );
        setRepos((prev) => {
          if (summaryGen.current !== gen) return prev;
          return prev.map((r) => {
            const hit = results.find((x) => x && x.id === r.id);
            return hit && hit.summary ? { ...r, aiSummary: hit.summary } : r;
          });
        });
        setSummarizeProgress((p) =>
          summaryGen.current === gen ? { done: Math.min(i + chunk.length, targets.length), total: targets.length } : p,
        );
      }
      if (summaryGen.current === gen) setSummarizeProgress(null);
    })();
  }, []);

  const refresh = useCallback(() => {
    if (!isTauri()) {
      setRepos(MOCK_REPOS);
      setReady(true);
      return;
    }
    if (scanning.current) return;
    scanning.current = true;
    setLoading(true);
    setError(null);
    ipc
      .scanRepos()
      .then((next) => {
        // A fresh scan has stars/summary cleared; carry over already-known
        // enrichment + AI summary so they don't flicker away while the
        // (cached) enrich/summarize passes re-confirm them.
        setRepos((prev) => {
          const prior = new Map(prev.map((r) => [r.id, r]));
          return next.map((r) => {
            const old = prior.get(r.id);
            return old
              ? {
                  ...r,
                  stars: old.stars,
                  topics: old.topics,
                  openIssues: old.openIssues,
                  latestRelease: old.latestRelease,
                  ci: old.ci,
                  aiSummary: old.aiSummary,
                }
              : r;
          });
        });
        setLastScan(Date.now());
        enrichAll(next);
        summarizeAll(next);
        // Refresh the semantic-search index in the background (best-effort).
        ipc.indexRepos(next).catch(() => {});
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        scanning.current = false;
        setLoading(false);
        setReady(true);
      });
  }, [enrichAll, summarizeAll]);

  // Latest repos, readable from stable callbacks without re-creating them.
  const reposRef = useRef(repos);
  reposRef.current = repos;

  const fetchAll = useCallback(() => {
    if (!isTauri() || scanning.current) return;
    const ids = reposRef.current.map((r) => r.id);
    if (ids.length === 0) return;
    setFetching(true);
    ipc
      .fetchAll(ids)
      .then((outcomes) => {
        const byId = new Map(outcomes.map((o) => [o.id, o]));
        setRepos((prev) =>
          prev.map((r) => {
            const o = byId.get(r.id);
            return o?.status ? { ...r, git: o.status } : r;
          }),
        );
        const behind = outcomes.filter((o) => o.status && o.status.behind > 0).length;
        if (behind > 0) {
          ipc.notify("Orrery", `${behind} repo${behind === 1 ? "" : "s"} behind upstream after fetch`).catch(() => {});
        }
      })
      .catch(() => {})
      .finally(() => setFetching(false));
  }, []);

  // Regenerate one repo's summary on demand (force, ignoring the cache).
  const summarizeRepo = useCallback((repo: Repo) => {
    if (!isTauri()) return;
    setSummarizing((s) => (s.includes(repo.id) ? s : [...s, repo.id]));
    ipc
      .summarizeRepo(repo, true)
      .then((summary) => {
        if (summary) setRepos((prev) => prev.map((r) => (r.id === repo.id ? { ...r, aiSummary: summary } : r)));
      })
      .catch((e) => console.error("[orrery] summarize failed:", e))
      .finally(() => setSummarizing((s) => s.filter((id) => id !== repo.id)));
  }, []);

  // Fill in summaries for every repo that lacks one (toolbar "Summarize all").
  const summarizeMissing = useCallback(() => summarizeAll(reposRef.current), [summarizeAll]);

  // Drop in-memory summaries (paired with clearing the on-disk AI cache).
  const clearSummaries = useCallback(() => {
    summaryGen.current += 1; // cancel any in-flight batch so it can't repopulate
    setRepos((prev) => prev.map((r) => (r.aiSummary ? { ...r, aiSummary: null } : r)));
  }, []);

  const refreshAiStatus = useCallback(() => {
    if (isTauri()) ipc.aiStatus().then(setAiStatus).catch(() => {});
  }, []);

  // Resolve the configured IDE/agent to a brand id so cards can show its logo.
  const refreshLaunchers = useCallback(() => {
    if (!isTauri()) return;
    ipc
      .getConfig()
      .then((c) => {
        const ide = detectIde(c.ideCommand);
        setIdeBrand(ide ? ide.brand ?? ide.id : "");
        setIdeName(ide?.name ?? "");
        const agent = detectAgent(c.agentCommand);
        setAgentBrand(agent?.id ?? "");
        setAgentName(agent?.name ?? "");
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshLaunchers();
  }, [refreshLaunchers]);

  // AI is usable when it's enabled, Ollama is reachable, and a chat model
  // resolved. Everything AI-related in the UI is hidden unless this holds.
  const aiReady = !!aiStatus && aiStatus.enabled && aiStatus.reachable && !!aiStatus.model;

  useEffect(() => {
    refreshAiStatus();
  }, [refreshAiStatus]);

  useEffect(() => {
    let cancelled = false;
    if (!isTauri()) {
      setRepos(MOCK_REPOS);
      setReady(true);
      return;
    }
    // Paint the cached snapshot instantly, then kick off a fresh scan.
    ipc
      .cachedRepos()
      .then((cached) => {
        if (!cancelled && cached.length) {
          setRepos(cached);
          setReady(true);
          enrichAll(cached); // populate from host cache (instant/offline)
          summarizeAll(cached); // cached summaries paint instantly
        }
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) refresh();
      });
    return () => {
      cancelled = true;
    };
  }, [refresh, enrichAll, summarizeAll]);

  // Live-watch: the Rust watcher emits `repos-changed` (debounced) on disk
  // changes; rescan when it fires. The scan guard coalesces bursts.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    listen("repos-changed", () => refresh())
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);

  const toggleFavorite = useCallback((repo: Repo) => {
    const next = !repo.favorite;
    setRepos((prev) => prev.map((r) => (r.id === repo.id ? { ...r, favorite: next } : r)));
    if (isTauri()) ipc.setFavorite(repo.id, next).catch(() => {});
  }, []);

  const openIde = useCallback((repo: Repo) => {
    if (isTauri()) ipc.openInIde(repo.id).catch((e) => console.error("[orrery] open IDE failed:", e));
    else console.log("[orrery] open in IDE:", repo.path);
  }, []);

  const openFolder = useCallback((repo: Repo) => {
    if (isTauri()) ipc.openFolder(repo.id).catch((e) => console.error("[orrery] open folder failed:", e));
    else console.log("[orrery] open folder:", repo.path);
  }, []);

  const openHost = useCallback((repo: Repo) => {
    if (!repo.slug) return;
    const host = repo.remoteHost ?? (repo.host === "gitlab" ? "gitlab.com" : "github.com");
    const url = `https://${host}/${repo.slug}`;
    if (isTauri()) openUrl(url).catch(() => {});
    else window.open(url, "_blank");
  }, []);

  const refreshAgents = useCallback(() => {
    if (!isTauri()) return;
    ipc
      .activeAgents()
      .then((next) => {
        // Keep the same array reference when nothing changed (the usual case:
        // no agents). Otherwise this 10s poll churns the context value and
        // re-renders every consumer. Order-insensitive set compare.
        setActiveAgents((prev) => {
          if (prev.length === next.length) {
            const set = new Set(prev);
            if (next.every((id) => set.has(id))) return prev;
          }
          return next;
        });
      })
      .catch(() => {});
  }, []);

  const openAgent = useCallback(
    (repo: Repo) => {
      if (!isTauri()) {
        console.log("[orrery] start agent:", repo.path);
        return;
      }
      ipc
        .openAgent(repo.id)
        .then(() => refreshAgents())
        .catch((e) => console.error("[orrery] open agent failed:", e));
    },
    [refreshAgents],
  );

  // Poll active agent sessions so badges clear when an agent exits.
  useEffect(() => {
    if (!isTauri()) return;
    refreshAgents();
    const handle = setInterval(refreshAgents, 10_000);
    return () => clearInterval(handle);
  }, [refreshAgents]);

  const value = useMemo<ReposContextValue>(
    () => ({
      repos,
      loading,
      ready,
      error,
      lastScan,
      fetching,
      activeAgents,
      summarizing,
      aiReady,
      refreshAiStatus,
      refresh,
      fetchAll,
      summarizeRepo,
      summarizeMissing,
      clearSummaries,
      toggleFavorite,
      openIde,
      openAgent,
      openFolder,
      openHost,
      ideBrand,
      ideName,
      agentBrand,
      agentName,
      refreshLaunchers,
    }),
    [repos, loading, ready, error, lastScan, fetching, activeAgents, summarizing, aiReady, refreshAiStatus, refresh, fetchAll, summarizeRepo, summarizeMissing, clearSummaries, toggleFavorite, openIde, openAgent, openFolder, openHost, ideBrand, ideName, agentBrand, agentName, refreshLaunchers],
  );

  // Separate value so progress ticks (enrich/summarize batches) only re-render
  // the progress indicator, not the repo grid. `loading` is also in the repos
  // value, but it toggles at most twice per scan.
  const scanStatus = useMemo<ScanStatus>(
    () => ({ scanning: loading, fetching, enrich: enrichProgress, summarize: summarizeProgress }),
    [loading, fetching, enrichProgress, summarizeProgress],
  );

  return (
    <ReposContext.Provider value={value}>
      <ScanStatusContext.Provider value={scanStatus}>{children}</ScanStatusContext.Provider>
    </ReposContext.Provider>
  );
}

export function useRepos(): ReposContextValue {
  const ctx = useContext(ReposContext);
  if (!ctx) throw new Error("useRepos must be used within <ReposProvider>");
  return ctx;
}

export function useScanStatus(): ScanStatus {
  const ctx = useContext(ScanStatusContext);
  if (!ctx) throw new Error("useScanStatus must be used within <ReposProvider>");
  return ctx;
}
