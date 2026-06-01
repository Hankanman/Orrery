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
import { ipc, isTauri } from "@/lib/ipc";
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
  refresh: () => void;
  toggleFavorite: (repo: Repo) => void;
  openIde: (repo: Repo) => void;
  openAgent: (repo: Repo) => void;
}

const ReposContext = createContext<ReposContextValue | null>(null);

export function ReposProvider({ children }: { children: ReactNode }) {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [loading, setLoading] = useState(false);
  const [ready, setReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [lastScan, setLastScan] = useState<number | null>(null);
  // Guards against overlapping scans (startup scan + a Rescan click racing).
  const scanning = useRef(false);
  // Generation token so a superseded enrichment run can't clobber a newer one.
  const enrichGen = useRef(0);

  // Fetch host enrichment (stars/topics/issues/release) in small concurrent
  // batches, merging each result into the matching repo. Cheap to re-run: the
  // Rust side caches for 6h and falls back to cache when offline.
  const enrichAll = useCallback((list: Repo[]) => {
    if (!isTauri()) return;
    const gen = ++enrichGen.current;
    const targets = list.filter((r) => r.host && r.slug && (r.remoteHost || r.host === "github"));
    const CHUNK = 6;
    void (async () => {
      for (let i = 0; i < targets.length; i += CHUNK) {
        if (enrichGen.current !== gen) return; // superseded by a newer scan/enrich
        const chunk = targets.slice(i, i + CHUNK);
        const results = await Promise.all(
          chunk.map((r) =>
            ipc
              .enrichRepo(r.host as "github" | "gitlab", r.remoteHost ?? "github.com", r.slug!)
              .then((info) => ({ id: r.id, info }))
              .catch(() => null),
          ),
        );
        setRepos((prev) => {
          if (enrichGen.current !== gen) return prev; // discard stale results
          return prev.map((r) => {
            const hit = results.find((x) => x && x.id === r.id);
            return hit
              ? {
                  ...r,
                  stars: hit.info.stars,
                  topics: hit.info.topics,
                  openIssues: hit.info.openIssues,
                  latestRelease: hit.info.latestRelease,
                }
              : r;
          });
        });
      }
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
        setRepos(next);
        setLastScan(Date.now());
        enrichAll(next);
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        scanning.current = false;
        setLoading(false);
        setReady(true);
      });
  }, [enrichAll]);

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
        }
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) refresh();
      });
    return () => {
      cancelled = true;
    };
  }, [refresh, enrichAll]);

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

  const openAgent = useCallback((repo: Repo) => {
    if (isTauri()) ipc.openAgent(repo.id).catch((e) => console.error("[orrery] open agent failed:", e));
    else console.log("[orrery] start agent:", repo.path);
  }, []);

  const value = useMemo<ReposContextValue>(
    () => ({ repos, loading, ready, error, lastScan, refresh, toggleFavorite, openIde, openAgent }),
    [repos, loading, ready, error, lastScan, refresh, toggleFavorite, openIde, openAgent],
  );

  return <ReposContext.Provider value={value}>{children}</ReposContext.Provider>;
}

export function useRepos(): ReposContextValue {
  const ctx = useContext(ReposContext);
  if (!ctx) throw new Error("useRepos must be used within <ReposProvider>");
  return ctx;
}
