import { useMemo, useState } from "react";
import { Search, Wrench } from "lucide-react";
import { TOOLS } from "@/components/tools/registry";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

/** Dev Tools — an offline utility belt (UUIDs, encoders, converters, …). */
export function ToolsView() {
  const [query, setQuery] = useState("");

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return TOOLS;
    return TOOLS.filter(
      (t) =>
        t.name.toLowerCase().includes(q) ||
        t.description.toLowerCase().includes(q) ||
        t.keywords.some((k) => k.includes(q)),
    );
  }, [query]);

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
  );
}
