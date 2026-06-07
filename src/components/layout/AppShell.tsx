import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { Link, Outlet } from "@tanstack/react-router";
import { Folder, Orbit, Plus, RefreshCw, Search } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ScanProgress } from "@/components/ScanProgress";
import { NewProjectDialog } from "@/components/NewProjectDialog";
import { Sidebar } from "@/components/layout/Sidebar";
import { ReposProvider, useRepos } from "@/lib/repos-context";
import { SidebarSlotProvider } from "@/lib/sidebar-slot";
import { useSystemAppearance } from "@/hooks/useSystemAppearance";
import { cn } from "@/lib/utils";

// Defer the command palette (and its cmdk + dialog deps) until first ⌘K.
const CommandPalette = lazy(() =>
  import("@/components/CommandPalette").then((m) => ({ default: m.CommandPalette })),
);

function Shell() {
  const { repos, loading, refresh } = useRepos();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [newOpen, setNewOpen] = useState(false);

  const rootCount = useMemo(() => new Set(repos.map((r) => r.root)).size, [repos]);

  // App-launch choreography: the frame "builds itself" once. The class is on
  // from first paint and removed after the sequence, so navigating back to a
  // view later doesn't replay the entrance.
  const [booting, setBooting] = useState(true);
  useEffect(() => {
    const t = setTimeout(() => setBooting(false), 1200);
    return () => clearTimeout(t);
  }, []);

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
    <div className={cn("flex h-full flex-col", booting && "orr-booting")}>
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
          title="New project / clone"
          aria-label="New project or clone"
          onClick={() => setNewOpen(true)}
        >
          <Plus className="size-4" />
        </button>

        <button
          type="button"
          className="orr-iconbtn"
          title="Rescan"
          aria-label="Rescan"
          onClick={() => refresh(true)}
          disabled={loading}
        >
          <RefreshCw className={cn("size-4", loading && "animate-spin")} />
        </button>
      </header>

      <div className="orr-body">
        <Sidebar />
        <Outlet />
      </div>

      {paletteOpen && (
        <Suspense fallback={null}>
          <CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} onNewProject={() => setNewOpen(true)} />
        </Suspense>
      )}

      <NewProjectDialog open={newOpen} onOpenChange={setNewOpen} />
    </div>
  );
}

export function AppShell() {
  useSystemAppearance();
  return (
    <TooltipProvider delayDuration={300}>
      <ReposProvider>
        <SidebarSlotProvider>
          <Shell />
        </SidebarSlotProvider>
      </ReposProvider>
    </TooltipProvider>
  );
}
