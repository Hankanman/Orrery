import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { RepoCard, type RepoView } from "@/components/RepoCard";
import type { Repo } from "@/types";
import { cn } from "@/lib/utils";

// Keep in sync with the grid CSS: card floor width and the row gaps.
const MIN_CARD = 344;
const COL_GAP = 14;
const GRID_ROW_GAP = 14;
const LIST_ROW_GAP = 8;

interface VirtualRepoGridProps {
  repos: Repo[];
  view: RepoView;
  activeAgents: string[];
  summarizing: string[];
  aiReady: boolean;
  onOpen: (repo: Repo) => void;
  onToggleFavorite: (repo: Repo) => void;
  onOpenIde: (repo: Repo) => void;
  onOpenAgent: (repo: Repo) => void;
  onSummarize: (repo: Repo) => void;
}

/**
 * Windowed repo grid: only the visible rows are in the DOM, so it stays smooth
 * with hundreds of repos. Columns are derived from the container width (grid
 * view) or fixed at 1 (list view); rows are measured dynamically so variable
 * card heights work.
 */
export function VirtualRepoGrid({
  repos,
  view,
  activeAgents,
  summarizing,
  aiReady,
  onOpen,
  onToggleFavorite,
  onOpenIde,
  onOpenAgent,
  onSummarize,
}: VirtualRepoGridProps) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [columns, setColumns] = useState(1);

  // Derive the column count from the scroll container's width.
  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const compute = () => {
      if (view === "list") {
        setColumns(1);
        return;
      }
      const styles = getComputedStyle(el);
      const padX = parseFloat(styles.paddingLeft) + parseFloat(styles.paddingRight);
      const inner = el.clientWidth - padX;
      setColumns(Math.max(1, Math.floor((inner + COL_GAP) / (MIN_CARD + COL_GAP))));
    };
    compute();
    const ro = new ResizeObserver(compute);
    ro.observe(el);
    return () => ro.disconnect();
  }, [view]);

  const rowCount = Math.ceil(repos.length / columns);
  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => (view === "list" ? 76 : 230),
    overscan: 6,
    gap: view === "list" ? LIST_ROW_GAP : GRID_ROW_GAP,
  });

  return (
    <div ref={parentRef} className={cn("orr-grid", view === "list" && "list")}>
      <div className="orr-grid-sizer" style={{ height: rowVirtualizer.getTotalSize() }}>
        {rowVirtualizer.getVirtualItems().map((row) => {
          const start = row.index * columns;
          const rowRepos = repos.slice(start, start + columns);
          return (
            <div
              key={row.key}
              ref={rowVirtualizer.measureElement}
              data-index={row.index}
              className="orr-grid-row"
              style={{
                transform: `translateY(${row.start}px)`,
                gridTemplateColumns: view === "list" ? "1fr" : `repeat(${columns}, minmax(0, 1fr))`,
              }}
            >
              {rowRepos.map((repo) => (
                <RepoCard
                  key={repo.id}
                  repo={repo}
                  view={view}
                  agentActive={activeAgents.includes(repo.id)}
                  summarizing={summarizing.includes(repo.id)}
                  onOpen={onOpen}
                  onToggleFavorite={onToggleFavorite}
                  onOpenIde={onOpenIde}
                  onOpenAgent={onOpenAgent}
                  onSummarize={aiReady ? onSummarize : undefined}
                />
              ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}
