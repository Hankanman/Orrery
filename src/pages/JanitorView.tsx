import { useEffect, useMemo, useState } from "react";
import { GitBranch, Scissors, Sparkles } from "lucide-react";
import { ipc, isTauri, type RepoPrunable } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import { HostIcon } from "@/components/HostIcon";
import { Spinner } from "@/components/Spinner";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";

// Fictional data for the browser demo (no Tauri/git backend).
const MOCK_PRUNABLE: RepoPrunable[] = [
  {
    id: "/home/seb/dev/personal/Orrery",
    branches: [
      { name: "feat/old-card-layout", isHead: false, upstream: null, gone: false, merged: true },
      { name: "fix/typo", isHead: false, upstream: "origin/fix/typo", gone: true, merged: false },
    ],
  },
  {
    id: "/home/seb/dev/personal/synth",
    branches: [{ name: "experiment/wavetables", isHead: false, upstream: null, gone: false, merged: true }],
  },
];

/** Branch janitor — prunable branches (merged / gone upstream) across all repos. */
export function JanitorView() {
  const { repos, refresh } = useRepos();
  const [groups, setGroups] = useState<RepoPrunable[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const repoById = useMemo(() => new Map(repos.map((r) => [r.id, r])), [repos]);
  const ids = useMemo(() => repos.map((r) => r.id), [repos]);
  const idsKey = ids.join("|");

  // Re-fetch only when the set of repos changes (not on every enrich batch).
  useEffect(() => {
    if (!isTauri()) {
      setGroups(MOCK_PRUNABLE);
      return;
    }
    let alive = true;
    setGroups(null);
    ipc
      .prunableBranches(ids)
      .then((g) => alive && setGroups(g))
      .catch(() => alive && setGroups([]));
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idsKey]);

  const prune = async (g: RepoPrunable) => {
    const repo = repoById.get(g.id);
    const label = repo?.displayName ?? g.id;
    const names = g.branches.map((b) => b.name);
    if (!window.confirm(`Delete ${names.length} branch(es) in ${label}?\n\n${names.join("\n")}`)) return;
    setBusy(g.id);
    try {
      if (isTauri()) await ipc.pruneBranches(g.id);
      setGroups((cur) => (cur ?? []).filter((x) => x.id !== g.id));
      refresh();
    } finally {
      setBusy(null);
    }
  };

  const total = (groups ?? []).reduce((n, g) => n + g.branches.length, 0);

  return (
    <div className="orr-feed">
      <header className="orr-feed-head">
        <h1>Cleanup</h1>
        <p>
          Branches that are merged or whose upstream is gone — safe to prune.{" "}
          <code>main</code>/<code>master</code> and the current branch are never touched.
        </p>
      </header>

      {groups === null ? (
        <div className="grid place-items-center py-16">
          <Spinner />
        </div>
      ) : groups.length === 0 ? (
        <div className="orr-empty">
          <Sparkles className="size-8 opacity-60" />
          <p className="t">Nothing to clean up</p>
          <p className="s">No merged or gone-upstream branches across your repos.</p>
        </div>
      ) : (
        <div className="grid gap-3 p-4 pt-2 [grid-template-columns:repeat(auto-fill,minmax(360px,1fr))]">
          {groups.map((g) => {
            const repo = repoById.get(g.id);
            return (
              <Card key={g.id}>
                <CardHeader className="pb-3">
                  <CardTitle className="flex items-center gap-2 text-sm">
                    {repo?.host && <HostIcon host={repo.host} className="size-4 opacity-70" />}
                    <span className="truncate">{repo?.displayName ?? g.id}</span>
                    <span className="ml-auto text-xs font-normal text-muted-foreground">{g.branches.length}</span>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-2">
                  <div className="space-y-1">
                    {g.branches.map((b) => (
                      <div key={b.name} className="flex items-center gap-2 text-sm">
                        <GitBranch className="size-3.5 shrink-0 text-muted-foreground" />
                        <span className="min-w-0 flex-1 truncate font-mono text-xs">{b.name}</span>
                        <span className={b.merged ? "text-xs text-muted-foreground" : "text-xs text-warn"}>
                          {b.merged ? "merged" : "gone"}
                        </span>
                      </div>
                    ))}
                  </div>
                  <Button size="sm" variant="outline" disabled={busy === g.id} onClick={() => prune(g)}>
                    <Scissors className="size-4" /> Prune {g.branches.length}
                  </Button>
                </CardContent>
              </Card>
            );
          })}
        </div>
      )}

      {total > 0 && <p className="px-4 pb-4 text-xs text-muted-foreground">{total} branch(es) prunable across {groups!.length} repo(s).</p>}
    </div>
  );
}
