import { useMemo } from "react";
import { useNavigate } from "@tanstack/react-router";
import { FolderGit2, Plus } from "lucide-react";
import type { Repo } from "@/types";
import { LangIcon } from "@/components/LangIcon";
import { cn } from "@/lib/utils";

interface GridFacetsProps {
  repos: Repo[];
  activeRoot: string; // "all" or a root path
  onSelectRoot: (root: string) => void;
  langFilter: string | null;
  onSelectLang: (lang: string | null) => void;
}

/** Mission Control's sidebar content: workspace-root and language filters. */
export function GridFacets({ repos, activeRoot, onSelectRoot, langFilter, onSelectLang }: GridFacetsProps) {
  const navigate = useNavigate();

  const { roots, langs } = useMemo(() => {
    const roots: { path: string; count: number }[] = [];
    for (const r of repos) {
      const found = roots.find((x) => x.path === r.root);
      if (found) found.count += 1;
      else roots.push({ path: r.root, count: 1 });
    }
    const langCounts = new Map<string, number>();
    for (const r of repos) {
      if (r.language) langCounts.set(r.language, (langCounts.get(r.language) ?? 0) + 1);
    }
    const langs = [...langCounts.entries()].sort((a, b) => b[1] - a[1]);
    return { roots, langs };
  }, [repos]);

  return (
    <>
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

      {langs.length > 0 && (
        <div className="orr-sb-sec">
          <div className="orr-sb-lead">Languages</div>
          {langs.map(([lang, count]) => (
            <button
              type="button"
              key={lang}
              className={cn("orr-sb-item", langFilter === lang && "active")}
              onClick={() => onSelectLang(langFilter === lang ? null : lang)}
            >
              <LangIcon language={lang} className="size-3.5" />
              <span className="nm">{lang}</span>
              <span className="count">{count}</span>
            </button>
          ))}
        </div>
      )}
    </>
  );
}
