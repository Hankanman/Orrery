import {
  ArrowDown,
  ArrowUp,
  Check,
  CircleDot,
  Clock,
  Code,
  GitBranch,
  Sparkles,
  SquareTerminal,
  Star,
} from "lucide-react";
import type { Repo } from "@/types";
import { formatStars, languageColor, repoStatus, timeAgo } from "@/lib/format";
import { HostIcon } from "@/components/HostIcon";
import { cn } from "@/lib/utils";

export type RepoView = "grid" | "list";

interface RepoCardProps {
  repo: Repo;
  view?: RepoView;
  onOpen?: (repo: Repo) => void;
  onToggleFavorite?: (repo: Repo) => void;
  onOpenIde?: (repo: Repo) => void;
  onOpenAgent?: (repo: Repo) => void;
}

/** Mono git-state line: ⎇ branch · ↑↓ · ● changes / ✓ clean */
function StatusRow({ repo }: { repo: Repo }) {
  const { git } = repo;
  const diverged = git.ahead > 0 || git.behind > 0;
  return (
    <div className="orr-card-status">
      <span className="orr-st muted">
        <GitBranch className="size-3.5" />
        {git.branch}
      </span>
      {diverged && (
        <span className={cn("orr-st", git.behind > 0 ? "behind" : "clean")}>
          <ArrowUp className="size-3" />
          {git.ahead}
          <ArrowDown className="ml-1 size-3" />
          {git.behind}
        </span>
      )}
      {git.dirty > 0 ? (
        <span className="orr-st dirty">
          <CircleDot className="size-3.5" />
          {git.dirty}
        </span>
      ) : (
        <span className="orr-st clean">
          <Check className="size-3.5" />
          clean
        </span>
      )}
    </div>
  );
}

export function RepoCard({
  repo,
  view = "grid",
  onOpen,
  onToggleFavorite,
  onOpenIde,
  onOpenAgent,
}: RepoCardProps) {
  const stale = repoStatus(repo) === "stale";

  const launchers = (
    <div className="orr-card-acts">
      <button
        type="button"
        className="orr-cbtn ide"
        onClick={(e) => {
          e.stopPropagation();
          onOpenIde?.(repo);
        }}
      >
        <Code className="size-3.5" />
        {view === "grid" ? "Open in IDE" : "IDE"}
      </button>
      <button
        type="button"
        className="orr-cbtn agent"
        onClick={(e) => {
          e.stopPropagation();
          onOpenAgent?.(repo);
        }}
      >
        <SquareTerminal className="size-3.5" />
        Agent
      </button>
    </div>
  );

  return (
    <button type="button" className="orr-card" onClick={() => onOpen?.(repo)}>
      <div className="orr-card-head">
        <div className="orr-card-name">
          <span
            className="ldot"
            style={{ background: languageColor(repo.language), color: languageColor(repo.language) }}
          />
          <span className="nm">{repo.displayName}</span>
        </div>
        {view === "grid" ? (
          <button
            type="button"
            className={cn("orr-card-fav", repo.favorite && "on")}
            aria-label={repo.favorite ? "Unfavorite" : "Favorite"}
            onClick={(e) => {
              e.stopPropagation();
              onToggleFavorite?.(repo);
            }}
          >
            <Star className="size-4" fill={repo.favorite ? "currentColor" : "none"} />
          </button>
        ) : (
          repo.language && <span className="orr-card-badge">{repo.language}</span>
        )}
      </div>

      {view === "grid" ? (
        <>
          <div className="orr-card-slug">
            {repo.slug ?? "no remote"} · {repo.path}
          </div>
          <div className="orr-card-desc">{repo.description ?? "No README description."}</div>
          {repo.aiSummary && (
            <div className="orr-card-ai">
              <Sparkles className="size-3" />
              {stale ? "Dormant" : "AI summary ready"}
            </div>
          )}
          <StatusRow repo={repo} />
          <div className="orr-card-host">
            {repo.host && (
              <span className="orr-st star">
                <Star className="size-3.5" />
                {formatStars(repo.stars)}
              </span>
            )}
            <span className="orr-st">
              <Clock className="size-3.5 opacity-70" />
              {timeAgo(repo.lastCommitUnix)}
            </span>
            {repo.host && (
              <span className="orr-st ml-auto opacity-70">
                <HostIcon host={repo.host} />
              </span>
            )}
          </div>
          {launchers}
        </>
      ) : (
        <>
          <div className="l-desc">{repo.description ?? "No README description."}</div>
          <StatusRow repo={repo} />
          <span className="orr-st muted ml-auto whitespace-nowrap">{timeAgo(repo.lastCommitUnix)}</span>
          {launchers}
        </>
      )}
    </button>
  );
}
