import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { Folder, Orbit, RefreshCw, Search, Settings } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useSystemAppearance } from "@/hooks/useSystemAppearance";
import { MOCK_REPOS } from "@/lib/mock-repos";
import { cn } from "@/lib/utils";

const ROOT_COUNT = new Set(MOCK_REPOS.map((r) => r.root)).size;

export function AppShell() {
  useSystemAppearance();
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col">
        <header className="orr-header">
          <Link to="/" className="orr-brand">
            <Orbit className="orr-mark size-6" />
            <span>Orrery</span>
          </Link>
          <div className="orr-roots">
            <Folder className="size-3.5" />
            <span>
              {ROOT_COUNT} roots · {MOCK_REPOS.length} repos
            </span>
          </div>

          <div className="ml-auto" />

          {/* Command bar — wires to the ⌘K palette in a later phase. */}
          <div className="orr-search" role="button" tabIndex={0} aria-label="Search repos, run a command">
            <Search className="size-4" />
            <span className="ph">Search repos, run a command…</span>
            <span className="kbd">⌘K</span>
          </div>

          <button type="button" className="orr-iconbtn" title="Rescan" aria-label="Rescan">
            <RefreshCw className="size-4" />
          </button>
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
      </div>
    </TooltipProvider>
  );
}
