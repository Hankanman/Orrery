import { FolderGit2, HardDrive, LayoutGrid, Plus, Sparkles } from "lucide-react";
import type { Repo } from "@/types";
import { languageColor } from "@/lib/format";
import { cn } from "@/lib/utils";

interface SidebarProps {
  repos: Repo[];
  activeRoot: string; // "all" or a root path
  onSelectRoot: (root: string) => void;
  langFilter: string | null;
  onSelectLang: (lang: string | null) => void;
}

export function Sidebar({ repos, activeRoot, onSelectRoot, langFilter, onSelectLang }: SidebarProps) {
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

  return (
    <aside className="orr-sidebar">
      <div className="orr-sb-sec">
        <button type="button" className="orr-sb-item active">
          <LayoutGrid className="size-4" /> Mission Control
        </button>
        <button type="button" className="orr-sb-item opacity-50" title="Coming soon" disabled>
          <Sparkles className="size-4" /> Feed
        </button>
      </div>

      <div className="orr-sb-sec">
        <div className="orr-sb-lead">
          Roots
          <span className="add" title="Add a directory">
            <Plus className="size-3.5" />
          </span>
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
        <HardDrive className="size-3.5" /> Cache synced · 2m ago
      </div>
    </aside>
  );
}
