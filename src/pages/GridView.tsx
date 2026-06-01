import { useMemo, useState } from "react";
import { ArrowUp, ArrowUpDown, CircleDot, Clock, LayoutGrid, List, Star } from "lucide-react";
import { MOCK_REPOS } from "@/lib/mock-repos";
import { RepoCard, type RepoView } from "@/components/RepoCard";
import { Sidebar } from "@/components/layout/Sidebar";
import { repoStatus } from "@/lib/format";
import type { Repo } from "@/types";
import { cn } from "@/lib/utils";

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

export function GridView() {
  const [activeRoot, setActiveRoot] = useState("all");
  const [langFilter, setLangFilter] = useState<string | null>(null);
  const [chips, setChips] = useState<Set<Chip>>(new Set());
  const [sort, setSort] = useState<SortKey>("activity");
  const [view, setView] = useState<RepoView>("grid");
  const [favorites, setFavorites] = useState<Set<string>>(
    () => new Set(MOCK_REPOS.filter((r) => r.favorite).map((r) => r.id)),
  );

  const repos = useMemo(
    () => MOCK_REPOS.map((r) => ({ ...r, favorite: favorites.has(r.id) })),
    [favorites],
  );

  const visible = useMemo(() => {
    const filtered = repos.filter((r) => {
      if (activeRoot !== "all" && r.root !== activeRoot) return false;
      if (langFilter && r.language !== langFilter) return false;
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
  }, [repos, activeRoot, langFilter, chips, sort]);

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

  const toggleFavorite = (repo: Repo) =>
    setFavorites((prev) => {
      const next = new Set(prev);
      next.has(repo.id) ? next.delete(repo.id) : next.add(repo.id);
      return next;
    });

  const openRepo = (r: Repo) => console.log("[orrery] open repo:", r.path);
  const openIde = (r: Repo) => console.log("[orrery] open in IDE:", r.path);
  const openAgent = (r: Repo) => console.log("[orrery] start agent:", r.path);

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
        <div className="orr-toolbar">
          <span className="title">{title}</span>
          <span className="sub">
            {visible.length} {visible.length === 1 ? "repo" : "repos"}
          </span>
          <span className="ml-auto" />
          <button type="button" className="orr-sortpill" onClick={cycleSort}>
            <ArrowUpDown className="size-3.5" />
            {sortLabel}
          </button>
          <div className="orr-seg">
            <button
              type="button"
              className={cn(view === "grid" && "on")}
              aria-label="Grid view"
              onClick={() => setView("grid")}
            >
              <LayoutGrid className="size-4" />
            </button>
            <button
              type="button"
              className={cn(view === "list" && "on")}
              aria-label="List view"
              onClick={() => setView("list")}
            >
              <List className="size-4" />
            </button>
          </div>
        </div>

        <div className="orr-chiprow">
          {CHIPS.map(({ key, label, icon: Icon }) => (
            <button
              type="button"
              key={key}
              className={cn("orr-chip", chips.has(key) && "on")}
              onClick={() => toggleChip(key)}
            >
              <Icon className="size-3.5" />
              {label}
            </button>
          ))}
        </div>

        <div className={cn("orr-grid", view === "list" && "list")}>
          {visible.map((repo) => (
            <RepoCard
              key={repo.id}
              repo={repo}
              view={view}
              onOpen={openRepo}
              onToggleFavorite={toggleFavorite}
              onOpenIde={openIde}
              onOpenAgent={openAgent}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
