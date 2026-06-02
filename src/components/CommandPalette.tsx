import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { Code, FolderGit2, RefreshCw, Settings, Sparkles, SquareTerminal } from "lucide-react";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { ipc, isTauri } from "@/lib/ipc";
import { useRepos } from "@/lib/repos-context";
import type { Repo } from "@/types";

export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { repos, openIde, openAgent, refresh, aiReady } = useRepos();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [hitIds, setHitIds] = useState<string[]>([]);

  // Debounced semantic search — surfaces conceptually-related repos that a
  // plain name filter would miss. Depends only on `query`, so repo updates
  // (enrich/summarize batches) don't re-fire the IPC call.
  useEffect(() => {
    if (!isTauri() || !aiReady || query.trim().length < 3) {
      setHitIds([]);
      return;
    }
    let cancelled = false;
    const handle = setTimeout(async () => {
      try {
        const hits = await ipc.semanticSearch(query);
        if (!cancelled) setHitIds(hits.map((h) => h.id)); // a newer query supersedes via cancel
      } catch {
        if (!cancelled) setHitIds([]);
      }
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [query, aiReady]);

  // Map hit ids → repos separately, so a repo update just re-maps (cheap)
  // rather than re-running the search. Results are force-mounted below so
  // cmdk's literal filter doesn't hide them.
  const matches = useMemo(() => {
    const byId = new Map(repos.map((r) => [r.id, r]));
    return hitIds.map((id) => byId.get(id)).filter((r): r is Repo => Boolean(r)).slice(0, 5);
  }, [hitIds, repos]);

  const run = (fn: () => void) => {
    onOpenChange(false);
    fn();
  };

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Search repos, run a command">
      <CommandInput placeholder="Search repos, run a command…" value={query} onValueChange={setQuery} />
      <CommandList>
        <CommandEmpty>No matches.</CommandEmpty>

        {matches.length > 0 && (
          <CommandGroup heading="Best matches" forceMount>
            {matches.map((repo) => (
              <CommandItem key={`sem-${repo.id}`} value={`sem-${repo.id}`} forceMount onSelect={() => run(() => openIde(repo))}>
                <Sparkles className="text-primary" />
                <span className="font-medium">{repo.displayName}</span>
                <span className="text-muted-foreground truncate text-xs">{repo.slug ?? repo.path}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        )}

        <CommandGroup heading="Actions">
          <CommandItem value="rescan refresh repos" onSelect={() => run(() => refresh(true))}>
            <RefreshCw />
            <span>Rescan repositories</span>
            <CommandShortcut>R</CommandShortcut>
          </CommandItem>
          <CommandItem value="settings preferences" onSelect={() => run(() => navigate({ to: "/settings" }))}>
            <Settings />
            <span>Open settings</span>
          </CommandItem>
        </CommandGroup>

        <CommandSeparator />

        <CommandGroup heading="Repositories">
          {repos.map((repo) => (
            <CommandItem
              key={repo.id}
              value={`${repo.displayName} ${repo.slug ?? ""} ${repo.path}`}
              onSelect={() => run(() => openIde(repo))}
            >
              <FolderGit2 />
              <span className="font-medium">{repo.displayName}</span>
              <span className="text-muted-foreground truncate text-xs">{repo.slug ?? repo.path}</span>
              <CommandShortcut className="flex items-center gap-2">
                <Code className="size-3.5" />
                IDE
                <button
                  type="button"
                  className="hover:text-foreground"
                  aria-label={`Start agent in ${repo.displayName}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    run(() => openAgent(repo));
                  }}
                >
                  <SquareTerminal className="size-3.5" />
                </button>
              </CommandShortcut>
            </CommandItem>
          ))}
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  );
}
