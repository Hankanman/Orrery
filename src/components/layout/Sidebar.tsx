import { useMemo } from "react";
import { useNavigate } from "@tanstack/react-router";
import { FolderGit2, HardDrive, Inbox, LayoutGrid, Plus } from "lucide-react";
import type { Repo } from "@/types";
import { languageColor, timeAgo } from "@/lib/format";
import { useRepos } from "@/lib/repos-context";
import { cn } from "@/lib/utils";

interface SidebarProps {
  repos: Repo[];
  activeRoot: string; // "all" or a root path
  onSelectRoot: (root: string) => void;
  langFilter: string | null;
  onSelectLang: (lang: string | null) => void;
}

export function Sidebar({ repos, activeRoot, onSelectRoot, langFilter, onSelectLang }: SidebarProps) {
  const { lastScan } = useRepos();
  const navigate = useNavigate();

  // Derived facets recompute only when the repo list changes — not on every
  // parent re-render (enrich/summarize update repos in batches at startup).
  const { roots, langs } = useMemo(() => {
    // Roots, in first-seen order, with live counts.
    const roots: { path: string; count: number }[] = [];
    for (const r of repos) {
      const found = roots.find((x) => x.path === r.root);
      if (found) found.count += 1;
      else roots.push({ path: r.root, count: 1 });
    }
    // Language facets, most common first.
    const langCounts = new Map<string, number>();
    for (const r of repos) {
      if (r.language) langCounts.set(r.language, (langCounts.get(r.language) ?? 0) + 1);
    }
    const langs = [...langCounts.entries()].sort((a, b) => b[1] - a[1]);
    return { roots, langs };
  }, [repos]);

  return (
    <aside className="orr-sidebar">
      <div className="orr-sb-sec">
        <button type="button" className="orr-sb-item active">
          <LayoutGrid className="size-4" /> Mission Control
        </button>
        <button type="button" className="orr-sb-item" onClick={() => navigate({ to: "/inbox" })}>
          <Inbox className="size-4" /> Feed
        </button>
      </div>

      <div className="orr-sb-sec">
        <div className="orr-sb-lead">
          Roots
          <button
            type="button"
            className="add"
            title="Add a workspace directory"
            aria-label="Add a workspace directory"
            onClick={() => navigate({ to: "/settings" })}
          >
            <Plus className="size-3.5" />
          </button>
        </div>
        <button
          type="button"
          className={cn("orr-sb-item", activeRoot === "all" && "active")}
          onClick={() => onSelectRoot("all")}
        >
          <FolderGit2 className="size-4" />
          <span className="nm">All repos</span>
          <span className="count">{repos.length}</span>
        </button>
        {roots.map((r) => (
          <button
            type="button"
            key={r.path}
            className={cn("orr-sb-item", activeRoot === r.path && "active")}
            onClick={() => onSelectRoot(r.path)}
          >
            <FolderGit2 className="size-4" />
            <span className="nm">{r.path}</span>
            <span className="count">{r.count}</span>
          </button>
        ))}
      </div>

      <div className="orr-sb-sec">
        <div className="orr-sb-lead">Languages</div>
        {langs.map(([lang, count]) => (
          <button
            type="button"
            key={lang}
            className={cn("orr-sb-item", langFilter === lang && "active")}
            onClick={() => onSelectLang(langFilter === lang ? null : lang)}
          >
            <span
              className="dot"
              style={{ background: languageColor(lang), boxShadow: `0 0 7px ${languageColor(lang)}` }}
            />
            <span className="nm">{lang}</span>
            <span className="count">{count}</span>
          </button>
        ))}
      </div>

      <div className="orr-sb-foot">
        <HardDrive className="size-3.5" />
        {lastScan ? `Scanned ${timeAgo(Math.floor(lastScan / 1000))}` : "Not scanned yet"}
      </div>
    </aside>
  );
}
