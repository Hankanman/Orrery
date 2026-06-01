import { useEffect, useState } from "react";
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
  const { repos, openIde, openAgent, refresh } = useRepos();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<Repo[]>([]);

  // Debounced semantic search — surfaces conceptually-related repos that a
  // plain name filter would miss. Results are force-mounted so cmdk's literal
  // filter doesn't hide them.
  useEffect(() => {
    if (!isTauri() || query.trim().length < 3) {
      setMatches([]);
      return;
    }
    const handle = setTimeout(async () => {
      try {
        const hits = await ipc.semanticSearch(query);
        const byId = new Map(repos.map((r) => [r.id, r]));
        setMatches(hits.map((h) => byId.get(h.id)).filter((r): r is Repo => Boolean(r)).slice(0, 5));
      } catch {
        setMatches([]);
      }
    }, 250);
    return () => clearTimeout(handle);
  }, [query, repos]);

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
          <CommandItem value="rescan refresh repos" onSelect={() => run(refresh)}>
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
