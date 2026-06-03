import { useMemo, useState } from "react";
import { Search, Wrench } from "lucide-react";
import { TOOLS, TOOL_CATEGORIES, type ToolCategory } from "@/components/tools/registry";
import { useSidebarSlot } from "@/lib/sidebar-slot";
import { cn } from "@/lib/utils";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

type Filter = "all" | ToolCategory;

/** Dev Tools — an offline utility belt (UUIDs, encoders, converters, …). */
export function ToolsView() {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<Filter>("all");

  const q = query.trim().toLowerCase();
  const searching = q.length > 0;

  const visible = useMemo(() => {
    return TOOLS.filter((t) => {
      // A search matches anywhere and spans every category; otherwise honour the
      // selected category filter from the sidebar.
      if (searching) {
        return (
          t.name.toLowerCase().includes(q) ||
          t.description.toLowerCase().includes(q) ||
          t.keywords.some((k) => k.includes(q))
        );
      }
      return category === "all" || t.category === category;
    });
  }, [q, searching, category]);

  // Sidebar content: filter the grid by category (mirrors Settings' sections).
  useSidebarSlot(
    useMemo(
      () => (
        <div className="orr-sb-sec">
          <div className="orr-sb-lead">Categories</div>
          <button
            type="button"
            className={cn("orr-sb-item", category === "all" && "active")}
            onClick={() => setCategory("all")}
          >
            <span className="nm">All tools</span>
            <span className="count">{TOOLS.length}</span>
          </button>
          {TOOL_CATEGORIES.map((c) => (
            <button
              key={c}
              type="button"
              className={cn("orr-sb-item", category === c && "active")}
              onClick={() => setCategory(c)}
            >
              <span className="nm">{c}</span>
              <span className="count">{TOOLS.filter((t) => t.category === c).length}</span>
            </button>
          ))}
        </div>
      ),
      [category],
    ),
  );

  return (
    <div className="orr-feed">
      <header className="orr-feed-head">
        <h1>Dev Tools</h1>
        <p>Little utilities you'd otherwise reach for a shell or a website — all offline.</p>
        <div className="relative mt-3 max-w-sm">
          <Search className="absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
          <input
            className="w-full rounded-md border border-border bg-background/50 py-2 pl-8 pr-2 text-sm outline-none focus:border-primary/50"
            placeholder="Search tools…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            autoFocus
          />
        </div>
      </header>

      <div className="orr-feed-scroll">
        {visible.length === 0 ? (
          <div className="orr-empty">
            <Wrench className="size-8 opacity-60" />
            <p className="t">No tools match “{query}”</p>
          </div>
        ) : (
          <div className="grid gap-3 p-4 pt-2 [grid-template-columns:repeat(auto-fill,minmax(340px,1fr))]">
            {visible.map(({ id, name, description, Component }) => (
              <Card key={id}>
                <CardHeader className="pb-3">
                  <CardTitle className="text-sm">{name}</CardTitle>
                  <CardDescription className="text-xs">{description}</CardDescription>
                </CardHeader>
                <CardContent>
                  <Component />
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
