import { useNavigate, useRouterState } from "@tanstack/react-router";
import { Compass, HardDrive, Inbox, LayoutGrid, Rss, Settings } from "lucide-react";
import { timeAgo } from "@/lib/format";
import { useRepos } from "@/lib/repos-context";
import { useSidebarSlotValue } from "@/lib/sidebar-slot";
import { cn } from "@/lib/utils";

const NAV = [
  { to: "/", icon: LayoutGrid, label: "Mission Control" },
  { to: "/inbox", icon: Inbox, label: "Inbox" },
  { to: "/feed", icon: Rss, label: "Feed" },
  { to: "/explore", icon: Compass, label: "Explore" },
  { to: "/settings", icon: Settings, label: "Settings" },
] as const;

/**
 * Persistent left rail. The top section (primary navigation) is the same on
 * every screen; everything below is a per-screen slot the active page fills
 * (grid filters, settings sections, …) via useSidebarSlot.
 */
export function Sidebar() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const navigate = useNavigate();
  const { lastScan } = useRepos();
  const slot = useSidebarSlotValue();

  return (
    <aside className="orr-sidebar">
      <div className="orr-sb-sec">
        {NAV.map(({ to, icon: Icon, label }) => (
          <button
            key={to}
            type="button"
            className={cn("orr-sb-item", pathname === to && "active")}
            onClick={() => navigate({ to })}
          >
            <Icon className="size-4" /> {label}
          </button>
        ))}
      </div>

      <div className="orr-sb-slot">{slot}</div>

      <div className="orr-sb-foot">
        <HardDrive className="size-3.5" />
        {lastScan ? `Scanned ${timeAgo(Math.floor(lastScan / 1000))}` : "Not scanned yet"}
      </div>
    </aside>
  );
}
