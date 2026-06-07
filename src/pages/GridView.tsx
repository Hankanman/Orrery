import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowUp,
  ArrowUpDown,
  CircleDot,
  Clock,
  CloudDownload,
  FolderSearch,
  Globe,
  LayoutGrid,
  List,
  Lock,
  RefreshCw,
  Sparkles,
  Star,
  TriangleAlert,
  X,
} from "lucide-react";
import { ContributionGraph } from "@/components/ContributionGraph";
import { FleetBar } from "@/components/FleetBar";
import { type RepoView } from "@/components/RepoCard";
import { VirtualRepoGrid } from "@/components/VirtualRepoGrid";
import { GridFacets } from "@/components/layout/GridFacets";
import { ipc, isTauri, type Briefing } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { useSidebarSlot } from "@/lib/sidebar-slot";
import {
  matchesChip,
  matchesVisibility,
  needsAttention,
  type Chip,
  type SortKey,
  type Visibility,
} from "@/lib/repo-filter";
import { useSavedViews, type SavedView } from "@/lib/saved-views";
import { useRepoTags } from "@/lib/repo-tags";
import type { Repo } from "@/types";
import { cn } from "@/lib/utils";

// Defer the drawer (and its react-markdown/remark-gfm deps) until a repo opens.
const RepoDrawer = lazy(() => import("@/components/RepoDrawer").then((m) => ({ default: m.RepoDrawer })));

const VIS_OPTIONS: { key: Visibility; label: string; icon: typeof Globe | null }[] = [
  { key: "all", label: "All", icon: null },
  { key: "public", label: "Public", icon: Globe },
  { key: "private", label: "Private", icon: Lock },
];

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

export function GridView() {
  const {
    repos,
    loading,
    ready,
    error,
    fetching,
    activeAgents,
    summarizing,
    aiReady,
    refresh,
    fetchAll,
    summarizeRepo,
    summarizeMissing,
    toggleFavorite,
    openIde,
    openAgent,
    openFolder,
    openHost,
    ideBrand,
    ideName,
    agentBrand,
    agentName,
  } = useRepos();

  const [activeRoot, setActiveRoot] = useState("all");
  const [langFilter, setLangFilter] = useState<string | null>(null);
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const tagMap = useRepoTags();
  const [chips, setChips] = useState<Set<Chip>>(new Set());
  const [visibility, setVisibility] = useState<Visibility>("all");
  const [attentionOnly, setAttentionOnly] = useState(false);
  const [sort, setSort] = useState<SortKey>("activity");
  const [view, setView] = useState<RepoView>("grid");
  const [selected, setSelected] = useState<Repo | null>(null);
  // Fleet multi-select (repo ids). Local to the grid — see #63.
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const toggleSelect = useCallback((repo: Repo) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(repo.id)) next.delete(repo.id);
      else next.add(repo.id);
      return next;
    });
  }, []);
  const clearSelection = useCallback(() => setSelectedIds(new Set()), []);
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
      if (activeTag && !tagMap[r.id]?.includes(activeTag)) return false;
      if (!matchesVisibility(r, visibility)) return false;
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
  }, [repos, activeRoot, langFilter, activeTag, tagMap, chips, visibility, attentionOnly, sort]);

  const attentionCount = useMemo(() => repos.filter(needsAttention).length, [repos]);
  const missingSummaries = useMemo(() => repos.filter((r) => !r.aiSummary).length, [repos]);

  // Fleet select-all toggles over the currently-visible (filtered) repos.
  const visibleIds = useMemo(() => visible.map((r) => r.id), [visible]);
  const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => selectedIds.has(id));
  const selectAllVisible = useCallback(() => {
    setSelectedIds((prev) => {
      const all = visibleIds.length > 0 && visibleIds.every((id) => prev.has(id));
      return all ? new Set() : new Set(visibleIds);
    });
  }, [visibleIds]);

  // All repo ids (paths) for the workspace-wide contribution graph — stable
  // across filters so the overview doesn't jump when you narrow the grid.
  const allIds = useMemo(() => repos.map((r) => r.id), [repos]);

  // Saved filter presets (persisted in localStorage). A ref holds the current
  // filter snapshot so "save view" captures the latest without re-creating the
  // sidebar node on every filter change.
  const { views, save: saveView, remove: removeView } = useSavedViews();
  const filtersRef = useRef<Omit<SavedView, "id" | "name">>(null!);
  filtersRef.current = {
    root: activeRoot,
    lang: langFilter,
    tag: activeTag,
    chips: [...chips],
    visibility,
    attention: attentionOnly,
    sort,
  };
  const applyView = useCallback((v: SavedView) => {
    setActiveRoot(v.root);
    setLangFilter(v.lang);
    setActiveTag(v.tag ?? null);
    setChips(new Set(v.chips));
    setVisibility(v.visibility);
    setAttentionOnly(v.attention);
    setSort(v.sort);
  }, []);
  const saveCurrentView = useCallback((name: string) => saveView({ name, ...filtersRef.current }), [saveView]);

  // Mission Control's sidebar content: saved views + projects + root + language filters.
  useSidebarSlot(
    useMemo(
      () => (
        <GridFacets
          repos={repos}
          activeRoot={activeRoot}
          onSelectRoot={setActiveRoot}
          langFilter={langFilter}
          onSelectLang={setLangFilter}
          activeTag={activeTag}
          onSelectTag={setActiveTag}
          savedViews={views}
          onApplyView={applyView}
          onSaveView={saveCurrentView}
          onDeleteView={removeView}
        />
      ),
      [repos, activeRoot, langFilter, activeTag, views, applyView, saveCurrentView, removeView],
    ),
  );

  // One-shot daily briefing once repos are loaded — only when AI is usable.
  useEffect(() => {
    if (!isTauri() || briefedRef.current || !ready || repos.length === 0 || !aiReady) return;
    briefedRef.current = true;
    ipc.dailyBriefing(repos).then(setBriefing).catch(() => {});
  }, [ready, repos, aiReady]);


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
    setActiveTag(null);
    setChips(new Set());
    setVisibility("all");
  };

  const title = activeRoot === "all" ? "All repos" : activeRoot;
  const sortLabel = SORTS.find((s) => s.key === sort)!.label;

  return (
    <>
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
          {aiReady && (
            <button
              type="button"
              className="orr-sortpill"
              onClick={summarizeMissing}
              disabled={missingSummaries === 0}
              title="Generate AI summaries for repos without one"
            >
              <Sparkles className="size-3.5" />
              Summarize{missingSummaries > 0 ? ` ${missingSummaries}` : ""}
            </button>
          )}
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
          <div className="orr-seg text" role="group" aria-label="Filter by visibility">
            {VIS_OPTIONS.map(({ key, label, icon: Icon }) => (
              <button
                type="button"
                key={key}
                className={cn(visibility === key && "on")}
                aria-pressed={visibility === key}
                onClick={() => setVisibility(key)}
              >
                {Icon && <Icon className="size-3.5" />}
                {label}
              </button>
            ))}
          </div>
          {CHIPS.map(({ key, label, icon: Icon }) => (
            <button type="button" key={key} className={cn("orr-chip", chips.has(key) && "on")} onClick={() => toggleChip(key)}>
              <Icon className="size-3.5" />
              {label}
            </button>
          ))}
        </div>

        <FleetBar
          selectedIds={selectedIds}
          visibleCount={visible.length}
          allSelected={allVisibleSelected}
          onSelectAllVisible={selectAllVisible}
          onClear={clearSelection}
        />

        {!ready ? (
          <div className="orr-grid-skel">
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
            <button type="button" className="orr-sortpill" onClick={() => refresh(true)}>
              <RefreshCw className="size-3.5" /> Try again
            </button>
          </div>
        ) : repos.length === 0 ? (
          <div className="orr-empty">
            <FolderSearch className="size-8 opacity-60" />
            <p className="t">No repositories found</p>
            <p className="s">Add a workspace directory in settings, then rescan.</p>
            <button type="button" className="orr-sortpill" onClick={() => refresh(true)} disabled={loading}>
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
          <VirtualRepoGrid
            repos={visible}
            view={view}
            activeAgents={activeAgents}
            summarizing={summarizing}
            aiReady={aiReady}
            onOpen={setSelected}
            onToggleFavorite={toggleFavorite}
            onOpenIde={openIde}
            onOpenAgent={openAgent}
            onOpenFolder={openFolder}
            onOpenHost={openHost}
            ideBrand={ideBrand}
            ideName={ideName}
            agentBrand={agentBrand}
            agentName={agentName}
            onSummarize={summarizeRepo}
            selectedIds={selectedIds}
            onToggleSelect={toggleSelect}
          />
        )}
      </div>

      {selected && (
        <Suspense fallback={null}>
          <RepoDrawer repo={selected} onClose={() => setSelected(null)} />
        </Suspense>
      )}
    </>
  );
}
