import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowUp,
  ArrowUpDown,
  CircleDot,
  Clock,
  CloudDownload,
  FolderSearch,
  LayoutGrid,
  List,
  RefreshCw,
  Sparkles,
  Star,
  TriangleAlert,
  X,
} from "lucide-react";
import { ContributionGraph } from "@/components/ContributionGraph";
import { RepoCard, type RepoView } from "@/components/RepoCard";
import { Sidebar } from "@/components/layout/Sidebar";
import { ipc, isTauri, type Briefing } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { repoStatus } from "@/lib/format";
import type { Repo } from "@/types";
import { cn } from "@/lib/utils";

// Defer the drawer (and its react-markdown/remark-gfm deps) until a repo opens.
const RepoDrawer = lazy(() => import("@/components/RepoDrawer").then((m) => ({ default: m.RepoDrawer })));

type SortKey = "activity" | "name" | "stars";
type Chip = "dirty" | "ahead" | "starred" | "stale";

const SORTS: { key: SortKey; label: string }[] = [
  { key: "activity", label: "Activity" },
  { key: "name", label: "Name" },
  { key: "stars", label: "Stars" },
];

const CHIPS: { key: Chip; label: string; icon: typeof CircleDot }[] = [
  { key: "dirty", label: "Dirty", icon: CircleDot },
  { key: "ahead", label: "Ahead", icon: ArrowUp },
  { key: "starred", label: "Starred", icon: Star },
  { key: "stale", label: "Stale", icon: Clock },
];

function matchesChip(repo: Repo, chip: Chip): boolean {
  switch (chip) {
    case "dirty":
      return repo.git.dirty > 0;
    case "ahead":
      return repo.git.ahead > 0;
    case "starred":
      return repo.favorite;
    case "stale":
      return repoStatus(repo) === "stale";
  }
}

/** A repo needs attention if it has uncommitted work, unpushed/behind commits, or is stale. */
function needsAttention(repo: Repo): boolean {
  return repo.git.dirty > 0 || repo.git.ahead > 0 || repo.git.behind > 0 || repoStatus(repo) === "stale";
}

export function GridView() {
  const { repos, loading, ready, error, fetching, activeAgents, refresh, fetchAll, toggleFavorite, openIde, openAgent } =
    useRepos();

  const [activeRoot, setActiveRoot] = useState("all");
  const [langFilter, setLangFilter] = useState<string | null>(null);
  const [chips, setChips] = useState<Set<Chip>>(new Set());
  const [attentionOnly, setAttentionOnly] = useState(false);
  const [sort, setSort] = useState<SortKey>("activity");
  const [view, setView] = useState<RepoView>("grid");
  const [selected, setSelected] = useState<Repo | null>(null);
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  const [briefingDismissed, setBriefingDismissed] = useState(false);
  const [showContrib, setShowContrib] = useState(() => {
    try {
      return localStorage.getItem("orr.contrib") !== "0";
    } catch {
      return true;
    }
  });
  const briefedRef = useRef(false);
  // Stagger card entrance on the first paint only. We drop the `stagger` class
  // once the cascade has finished starting, so later mounts (filtering, sorting)
  // animate uniformly instead of re-staggering.
  const [stagger, setStagger] = useState(true);
  const staggerStartedRef = useRef(false);

  const toggleContrib = () =>
    setShowContrib((v) => {
      const next = !v;
      try {
        localStorage.setItem("orr.contrib", next ? "1" : "0");
      } catch {
        // ignore — private mode / no storage
      }
      return next;
    });

  const visible = useMemo(() => {
    const filtered = repos.filter((r) => {
      if (activeRoot !== "all" && r.root !== activeRoot) return false;
      if (langFilter && r.language !== langFilter) return false;
      if (attentionOnly && !needsAttention(r)) return false;
      for (const chip of chips) if (!matchesChip(r, chip)) return false;
      return true;
    });
    const sorted = [...filtered];
    sorted.sort((a, b) => {
      if (sort === "name") return a.displayName.localeCompare(b.displayName);
      if (sort === "stars") return b.stars - a.stars;
      return b.lastCommitUnix - a.lastCommitUnix;
    });
    return sorted;
  }, [repos, activeRoot, langFilter, chips, attentionOnly, sort]);

  const attentionCount = useMemo(() => repos.filter(needsAttention).length, [repos]);

  // All repo ids (paths) for the workspace-wide contribution graph — stable
  // across filters so the overview doesn't jump when you narrow the grid.
  const allIds = useMemo(() => repos.map((r) => r.id), [repos]);

  // One-shot daily briefing once repos are loaded.
  useEffect(() => {
    if (!isTauri() || briefedRef.current || !ready || repos.length === 0) return;
    briefedRef.current = true;
    ipc.dailyBriefing(repos).then(setBriefing).catch(() => {});
  }, [ready, repos]);

  // Drop the stagger class once the first batch of cards is on screen and the
  // cascade has had time to start on each — after that, mounts animate plainly.
  useEffect(() => {
    if (staggerStartedRef.current || !ready || visible.length === 0) return;
    staggerStartedRef.current = true;
    const t = setTimeout(() => setStagger(false), 650);
    return () => clearTimeout(t);
  }, [ready, visible.length]);

  const toggleChip = (chip: Chip) =>
    setChips((prev) => {
      const next = new Set(prev);
      next.has(chip) ? next.delete(chip) : next.add(chip);
      return next;
    });

  const cycleSort = () =>
    setSort((prev) => {
      const i = SORTS.findIndex((s) => s.key === prev);
      return SORTS[(i + 1) % SORTS.length].key;
    });

  const clearFilters = () => {
    setActiveRoot("all");
    setLangFilter(null);
    setChips(new Set());
  };

  const title = activeRoot === "all" ? "All repos" : activeRoot;
  const sortLabel = SORTS.find((s) => s.key === sort)!.label;

  return (
    <div className="orr-body">
      <div className="orr-starfield" aria-hidden />
      <Sidebar
        repos={repos}
        activeRoot={activeRoot}
        onSelectRoot={setActiveRoot}
        langFilter={langFilter}
        onSelectLang={setLangFilter}
      />

      <div className="orr-main">
        {ready && repos.length > 0 && showContrib && <ContributionGraph ids={allIds} onHide={toggleContrib} />}

        <div className="orr-toolbar">
          <span className="title">{title}</span>
          <span className="sub">
            {visible.length} {visible.length === 1 ? "repo" : "repos"}
          </span>
          <span className="ml-auto" />
          <button
            type="button"
            className={cn("orr-sortpill", showContrib && "on")}
            onClick={toggleContrib}
            aria-pressed={showContrib}
            title={showContrib ? "Hide activity graph" : "Show activity graph"}
          >
            <Activity className="size-3.5" />
          </button>
          <button
            type="button"
            className={cn("orr-sortpill", attentionOnly && "on")}
            onClick={() => setAttentionOnly((v) => !v)}
            title="Show only repos needing attention"
          >
            <TriangleAlert className="size-3.5" />
            Attention{attentionCount > 0 ? ` ${attentionCount}` : ""}
          </button>
          <button type="button" className="orr-sortpill" onClick={fetchAll} disabled={fetching} title="Fetch all repos">
            <CloudDownload className={cn("size-3.5", fetching && "animate-pulse")} />
            {fetching ? "Fetching…" : "Fetch all"}
          </button>
          <button type="button" className="orr-sortpill" onClick={cycleSort}>
            <ArrowUpDown className="size-3.5" />
            {sortLabel}
          </button>
          <div className="orr-seg">
            <button type="button" className={cn(view === "grid" && "on")} aria-label="Grid view" onClick={() => setView("grid")}>
              <LayoutGrid className="size-4" />
            </button>
            <button type="button" className={cn(view === "list" && "on")} aria-label="List view" onClick={() => setView("list")}>
              <List className="size-4" />
            </button>
          </div>
        </div>

        {briefing && !briefingDismissed && briefing.repoCount > 0 && (
          <div className="orr-briefing">
            <Sparkles className="size-4 shrink-0 text-primary" />
            <p className="min-w-0 flex-1">{briefing.text}</p>
            <button type="button" aria-label="Dismiss briefing" onClick={() => setBriefingDismissed(true)}>
              <X className="size-4" />
            </button>
          </div>
        )}

        <div className="orr-chiprow">
          {CHIPS.map(({ key, label, icon: Icon }) => (
            <button type="button" key={key} className={cn("orr-chip", chips.has(key) && "on")} onClick={() => toggleChip(key)}>
              <Icon className="size-3.5" />
              {label}
            </button>
          ))}
        </div>

        {!ready ? (
          <div className={cn("orr-grid", view === "list" && "list")}>
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="orr-card orr-skel" aria-hidden>
                <div className="orr-skel-line w-2/3" />
                <div className="orr-skel-line w-1/2" />
                <div className="orr-skel-line w-full" />
                <div className="orr-skel-line w-3/4" />
              </div>
            ))}
          </div>
        ) : error && repos.length === 0 ? (
          <div className="orr-empty">
            <FolderSearch className="size-8 opacity-60" />
            <p className="t">Couldn’t scan your repositories</p>
            <p className="s">{error}</p>
            <button type="button" className="orr-sortpill" onClick={refresh}>
              <RefreshCw className="size-3.5" /> Try again
            </button>
          </div>
        ) : repos.length === 0 ? (
          <div className="orr-empty">
            <FolderSearch className="size-8 opacity-60" />
            <p className="t">No repositories found</p>
            <p className="s">Add a workspace directory in settings, then rescan.</p>
            <button type="button" className="orr-sortpill" onClick={refresh} disabled={loading}>
              <RefreshCw className={cn("size-3.5", loading && "animate-spin")} /> Rescan
            </button>
          </div>
        ) : visible.length === 0 ? (
          <div className="orr-empty">
            <FolderSearch className="size-8 opacity-60" />
            <p className="t">No repos match these filters</p>
            <button type="button" className="orr-sortpill" onClick={clearFilters}>
              Clear filters
            </button>
          </div>
        ) : (
          <div className={cn("orr-grid", view === "list" && "list", stagger && "stagger")}>
            {visible.map((repo) => (
              <RepoCard
                key={repo.id}
                repo={repo}
                view={view}
                agentActive={activeAgents.includes(repo.id)}
                onOpen={setSelected}
                onToggleFavorite={toggleFavorite}
                onOpenIde={openIde}
                onOpenAgent={openAgent}
              />
            ))}
          </div>
        )}
      </div>

      {selected && (
        <Suspense fallback={null}>
          <RepoDrawer repo={selected} onClose={() => setSelected(null)} />
        </Suspense>
      )}
    </div>
  );
}
