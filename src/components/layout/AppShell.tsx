import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { Orbit, RefreshCw, Search, Settings } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { TooltipProvider } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export function AppShell() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col">
        <header className="sticky top-0 z-20 flex h-14 items-center gap-3 border-b border-border/70 bg-background/80 px-4 backdrop-blur">
          <Link to="/" className="flex items-center gap-2 font-semibold tracking-tight">
            <Orbit className="size-5 text-primary" />
            <span>Orrery</span>
          </Link>

          <div className="relative ml-4 hidden max-w-md flex-1 sm:block">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="Search repos…   ⌘K"
              className="h-9 bg-secondary/40 pl-8"
              aria-label="Search repositories"
            />
          </div>

          <div className="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="icon" aria-label="Refresh">
              <RefreshCw className="size-4" />
            </Button>
            <Button
              asChild
              variant="ghost"
              size="icon"
              aria-label="Settings"
              className={cn(pathname === "/settings" && "bg-secondary text-foreground")}
            >
              <Link to="/settings">
                <Settings className="size-4" />
              </Link>
            </Button>
          </div>
        </header>

        <main className="flex-1 overflow-y-auto">
          <Outlet />
        </main>
      </div>
    </TooltipProvider>
  );
}
