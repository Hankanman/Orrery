import { useMemo } from "react";
import { useNavigate } from "@tanstack/react-router";
import { Bookmark, FolderGit2, Plus, X } from "lucide-react";
import type { Repo } from "@/types";
import type { SavedView } from "@/lib/saved-views";
import { LangIcon } from "@/components/LangIcon";
import { cn } from "@/lib/utils";

interface GridFacetsProps {
  repos: Repo[];
  activeRoot: string; // "all" or a root path
  onSelectRoot: (root: string) => void;
  langFilter: string | null;
  onSelectLang: (lang: string | null) => void;
  savedViews: SavedView[];
  onApplyView: (v: SavedView) => void;
  onSaveView: (name: string) => void;
  onDeleteView: (id: string) => void;
}

/** Mission Control's sidebar content: saved views + workspace-root and language filters. */
export function GridFacets({
  repos,
  activeRoot,
  onSelectRoot,
  langFilter,
  onSelectLang,
  savedViews,
  onApplyView,
  onSaveView,
  onDeleteView,
}: GridFacetsProps) {
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
          Views
          <button
            type="button"
            className="add"
            title="Save the current filters as a view"
            aria-label="Save the current filters as a view"
            onClick={() => {
              const name = window.prompt("Name this view")?.trim();
              if (name) onSaveView(name);
            }}
          >
            <Plus className="size-3.5" />
          </button>
        </div>
        {savedViews.length === 0 ? (
          <p className="px-3 py-1 text-xs text-muted-foreground">Save the current filters as a quick view.</p>
        ) : (
          savedViews.map((v) => (
            <button type="button" key={v.id} className="orr-sb-item" onClick={() => onApplyView(v)}>
              <Bookmark className="size-4" />
              <span className="nm">{v.name}</span>
              <span
                className="count cursor-pointer hover:text-danger"
                role="button"
                tabIndex={0}
                aria-label={`Delete view ${v.name}`}
                onClick={(e) => {
                  e.stopPropagation();
                  onDeleteView(v.id);
                }}
              >
                <X className="size-3" />
              </span>
            </button>
          ))
        )}
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
