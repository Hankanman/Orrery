import { RepoCard, type RepoView } from "@/components/RepoCard";
import { VirtualGrid } from "@/components/VirtualGrid";
import { cn } from "@/lib/utils";
import type { Repo } from "@/types";

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
  onOpenFolder: (repo: Repo) => void;
  onOpenHost: (repo: Repo) => void;
  onSummarize: (repo: Repo) => void;
}

/** Windowed repo grid (Mission Control). Thin wrapper over VirtualGrid. */
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
  onOpenFolder,
  onOpenHost,
  onSummarize,
}: VirtualRepoGridProps) {
  const list = view === "list";
  return (
    <VirtualGrid
      items={repos}
      className={cn("orr-grid", list && "list")}
      minColWidth={344}
      columns={list ? 1 : undefined}
      colGap={14}
      rowGap={list ? 8 : 14}
      estimateRow={list ? 76 : 230}
      renderItem={(repo) => (
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
          onOpenFolder={onOpenFolder}
          onOpenHost={onOpenHost}
          onSummarize={aiReady ? onSummarize : undefined}
        />
      )}
    />
  );
}
