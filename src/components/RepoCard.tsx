import { memo } from "react";
import {
  ArrowDown,
  ArrowUp,
  CircleDot,
  Clock,
  Code,
  GitBranch,
  RefreshCw,
  Sparkles,
  SquareTerminal,
  Star,
  Tag,
} from "lucide-react";
import type { Repo } from "@/types";
import { formatStars, languageColor, timeAgo } from "@/lib/format";
import { HostIcon } from "@/components/HostIcon";
import { cn } from "@/lib/utils";

export type RepoView = "grid" | "list";

interface RepoCardProps {
  repo: Repo;
  view?: RepoView;
  agentActive?: boolean;
  /** An on-demand AI summary is being generated for this repo. */
  summarizing?: boolean;
  onOpen?: (repo: Repo) => void;
  onToggleFavorite?: (repo: Repo) => void;
  onOpenIde?: (repo: Repo) => void;
  onOpenAgent?: (repo: Repo) => void;
  /** Generate/regenerate this repo's AI summary. */
  onSummarize?: (repo: Repo) => void;
}

/** Mono git-state line: ⎇ branch · ↑↓ divergence · ● changes. Clean repos show
 *  just the branch — no badge, since clean is the unremarkable default. */
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
      {git.dirty > 0 && (
        <span className="orr-st dirty">
          <CircleDot className="size-3.5" />
          {git.dirty}
        </span>
      )}
    </div>
  );
}

function RepoCardImpl({
  repo,
  view = "grid",
  agentActive,
  summarizing,
  onOpen,
  onToggleFavorite,
  onOpenIde,
  onOpenAgent,
  onSummarize,
}: RepoCardProps) {

  const launchers = (
    <div className="orr-card-acts">
      <button
        type="button"
        className="orr-cbtn agent"
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
          {agentActive && (
            <SquareTerminal className="size-3.5 shrink-0 animate-pulse text-primary" aria-label="Agent session running" />
          )}
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
          {summarizing ? (
            <div className="orr-card-ai">
              <Sparkles className="size-3 shrink-0 animate-pulse" />
              <span className="opacity-70">Summarizing…</span>
            </div>
          ) : repo.aiSummary ? (
            <div className="orr-card-ai" title={repo.aiSummary}>
              <Sparkles className="size-3 shrink-0" />
              <span className="line-clamp-2">{repo.aiSummary}</span>
              {onSummarize && (
                <button
                  type="button"
                  className="orr-card-ai-act"
                  aria-label="Regenerate summary"
                  title="Regenerate summary"
                  onClick={(e) => {
                    e.stopPropagation();
                    onSummarize(repo);
                  }}
                >
                  <RefreshCw className="size-3" />
                </button>
              )}
            </div>
          ) : (
            onSummarize && (
              <button
                type="button"
                className="orr-card-ai gen"
                onClick={(e) => {
                  e.stopPropagation();
                  onSummarize(repo);
                }}
              >
                <Sparkles className="size-3 shrink-0" />
                <span>Generate summary</span>
              </button>
            )
          )}
          <StatusRow repo={repo} />
          <div className="orr-card-host">
            {repo.ci && repo.ci !== "none" && (
              <span className="orr-st" title={`CI: ${repo.ci}`}>
                <span
                  className={cn(
                    "size-2 rounded-full",
                    repo.ci === "success" ? "bg-ok" : repo.ci === "failure" ? "bg-danger" : "bg-warn",
                  )}
                />
              </span>
            )}
            {repo.host && (
              <span className="orr-st star">
                <Star className="size-3.5" />
                {formatStars(repo.stars)}
              </span>
            )}
            {repo.latestRelease && (
              <span className="orr-st" title="Latest release">
                <Tag className="size-3.5" />
                {repo.latestRelease}
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

// Memoized: during startup, enrich/summarize update repos in batches; with
// stable context callbacks and preserved object identity for unchanged repos,
// only the cards that actually changed re-render.
export const RepoCard = memo(RepoCardImpl);
