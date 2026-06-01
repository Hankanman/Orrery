import { ArrowDown, ArrowUp, GitBranch, History, Pencil, TerminalSquare } from "lucide-react";
import type { Repo } from "@/types";
import { ACTIVITY_META, languageColor, timeAgo } from "@/lib/format";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

interface RepoCardProps {
  repo: Repo;
  onOpenIde?: (repo: Repo) => void;
  onOpenAgent?: (repo: Repo) => void;
}

export function RepoCard({ repo, onOpenIde, onOpenAgent }: RepoCardProps) {
  const { git } = repo;
  const activity = ACTIVITY_META[repo.activity];

  return (
    <Card className="group relative gap-0 overflow-hidden border-border/70 bg-card/80 p-4 backdrop-blur transition-colors hover:border-primary/40">
      {/* faint orbital accent that lights up on hover */}
      <div className="pointer-events-none absolute -right-16 -top-16 h-40 w-40 rounded-full bg-primary/0 blur-2xl transition-colors duration-300 group-hover:bg-primary/10" />

      {/* Header: name + language */}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span
              className="size-2.5 shrink-0 rounded-full ring-2 ring-background"
              style={{ backgroundColor: languageColor(repo.language) }}
              title={repo.language ?? "unknown"}
            />
            <h3 className="truncate text-base font-semibold tracking-tight">{repo.displayName}</h3>
          </div>
          <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
            {repo.slug ?? "no remote"} · {repo.path}
          </p>
        </div>
        {repo.language && (
          <span className="shrink-0 rounded-md border border-border/70 bg-secondary/60 px-2 py-0.5 text-xs text-secondary-foreground">
            {repo.language}
          </span>
        )}
      </div>

      {/* Description */}
      <p className="mt-3 line-clamp-2 min-h-[2.5rem] text-sm text-muted-foreground">
        {repo.description ?? "No README description."}
      </p>

      {/* Git status row */}
      <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-1.5 font-mono text-xs">
        <span className="flex items-center gap-1.5 text-foreground/90">
          <GitBranch className="size-3.5 text-muted-foreground" />
          {git.branch}
        </span>
        <span className={cn("flex items-center gap-2", git.ahead || git.behind ? "text-foreground/90" : "text-muted-foreground")}>
          <span className="flex items-center gap-0.5">
            <ArrowUp className="size-3.5" />
            {git.ahead}
          </span>
          <span className="flex items-center gap-0.5">
            <ArrowDown className="size-3.5" />
            {git.behind}
          </span>
        </span>
        <span className={cn("flex items-center gap-1.5", git.dirty > 0 ? "text-warn" : "text-muted-foreground")}>
          <span className={cn("size-1.5 rounded-full", git.dirty > 0 ? "bg-warn" : "bg-muted-foreground/50")} />
          {git.dirty > 0 ? `${git.dirty} changes` : "clean"}
        </span>
      </div>

      {/* Activity */}
      <div className="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground">
        <History className="size-3.5" />
        <span>last commit {timeAgo(repo.lastCommitUnix)}</span>
        <span className={cn("ml-auto font-medium", activity.className)}>{activity.label}</span>
      </div>

      {/* Launchers */}
      <div className="mt-4 flex gap-2">
        <Button size="sm" variant="secondary" className="flex-1" onClick={() => onOpenIde?.(repo)}>
          <Pencil className="size-3.5" />
          Open in IDE
        </Button>
        <Button size="sm" variant="outline" className="flex-1" onClick={() => onOpenAgent?.(repo)}>
          <TerminalSquare className="size-3.5" />
          Agent
        </Button>
      </div>
    </Card>
  );
}
