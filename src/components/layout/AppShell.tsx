import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { Folder, Inbox, Orbit, RefreshCw, Search, Settings } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ScanProgress } from "@/components/ScanProgress";
import { ReposProvider, useRepos } from "@/lib/repos-context";
import { useSystemAppearance } from "@/hooks/useSystemAppearance";
import { cn } from "@/lib/utils";

// Defer the command palette (and its cmdk + dialog deps) until first ⌘K.
const CommandPalette = lazy(() =>
  import("@/components/CommandPalette").then((m) => ({ default: m.CommandPalette })),
);

function Shell() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const { repos, loading, refresh } = useRepos();
  const [paletteOpen, setPaletteOpen] = useState(false);

  const rootCount = useMemo(() => new Set(repos.map((r) => r.root)).size, [repos]);

  // Global ⌘K / Ctrl-K to open the command palette.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setPaletteOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-full flex-col">
      <header className="orr-header">
        <Link to="/" className="orr-brand">
          <Orbit className="orr-mark size-6" />
          <span>Orrery</span>
        </Link>
        <div className="orr-roots">
          <Folder className="size-3.5" />
          <span>
            {rootCount} {rootCount === 1 ? "root" : "roots"} · {repos.length} repos
          </span>
        </div>

        <ScanProgress />

        <div className="ml-auto" />

        <div
          className="orr-search"
          role="button"
          tabIndex={0}
          aria-label="Search repos, run a command"
          onClick={() => setPaletteOpen(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") setPaletteOpen(true);
          }}
        >
          <Search className="size-4" />
          <span className="ph">Search repos, run a command…</span>
          <span className="kbd">⌘K</span>
        </div>

        <button
          type="button"
          className="orr-iconbtn"
          title="Rescan"
          aria-label="Rescan"
          onClick={refresh}
          disabled={loading}
        >
          <RefreshCw className={cn("size-4", loading && "animate-spin")} />
        </button>
        <Link
          to="/inbox"
          className={cn("orr-iconbtn", pathname === "/inbox" && "active")}
          title="Inbox"
          aria-label="Inbox"
        >
          <Inbox className="size-4" />
        </Link>
        <Link
          to="/settings"
          className={cn("orr-iconbtn", pathname === "/settings" && "active")}
          title="Settings"
          aria-label="Settings"
        >
          <Settings className="size-4" />
        </Link>
      </header>

      <div className="flex min-h-0 flex-1">
        <Outlet />
      </div>

      {paletteOpen && (
        <Suspense fallback={null}>
          <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} />
        </Suspense>
      )}
    </div>
  );
}

export function AppShell() {
  useSystemAppearance();
  return (
    <TooltipProvider delayDuration={300}>
      <ReposProvider>
        <Shell />
      </ReposProvider>
    </TooltipProvider>
  );
}
