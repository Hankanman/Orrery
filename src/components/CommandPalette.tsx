import { useNavigate } from "@tanstack/react-router";
import { Code, FolderGit2, RefreshCw, Settings, SquareTerminal } from "lucide-react";
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
import { useRepos } from "@/lib/repos-context";

export function CommandPalette({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { repos, openIde, openAgent, refresh } = useRepos();
  const navigate = useNavigate();

  const run = (fn: () => void) => {
    onOpenChange(false);
    fn();
  };

  return (
    <CommandDialog open={open} onOpenChange={onOpenChange} title="Search repos, run a command">
      <CommandInput placeholder="Search repos, run a command…" />
      <CommandList>
        <CommandEmpty>No matches.</CommandEmpty>

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
