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
      })
      .catch((e) => setError(String(e)))
      .finally(() => {
        scanning.current = false;
        setLoading(false);
        setReady(true);
      });
  }, []);

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
        }
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) refresh();
      });
    return () => {
      cancelled = true;
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
