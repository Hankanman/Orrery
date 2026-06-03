import { useState } from "react";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import {
  Compass,
  HardDrive,
  Inbox,
  LayoutGrid,
  PanelLeftClose,
  PanelLeftOpen,
  Rss,
  Scissors,
  Settings,
  Wrench,
} from "lucide-react";
import { timeAgo } from "@/lib/format";
import { useNavCounts, type NavCounts } from "@/lib/nav-counts";
import { useRepos } from "@/lib/repos-context";
import { useSidebarSlotValue } from "@/lib/sidebar-slot";
import { cn } from "@/lib/utils";

// `badge` names the useNavCounts field whose value is shown as a count on the
// item (omitted = no badge). The badge is hidden when its count is 0.
const NAV = [
  { to: "/", icon: LayoutGrid, label: "Mission Control", badge: "attention" },
  { to: "/inbox", icon: Inbox, label: "Inbox", badge: "inbox" },
  { to: "/feed", icon: Rss, label: "Feed" },
  { to: "/explore", icon: Compass, label: "Explore" },
  { to: "/tools", icon: Wrench, label: "Dev Tools" },
  { to: "/janitor", icon: Scissors, label: "Cleanup", badge: "prunable" },
  { to: "/settings", icon: Settings, label: "Settings" },
] as const satisfies ReadonlyArray<{
  to: string;
  icon: typeof LayoutGrid;
  label: string;
  badge?: keyof NavCounts;
}>;

const COLLAPSE_KEY = "orr.sidebar.collapsed";

/**
 * Persistent left rail. The top section (primary navigation, with live count
 * badges) is the same on every screen; everything below is a per-screen slot
 * the active page fills (grid filters, settings sections, …) via useSidebarSlot.
 * The rail collapses to an icon-only strip (persisted in localStorage).
 */
export function Sidebar() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const navigate = useNavigate();
  const { lastScan } = useRepos();
  const slot = useSidebarSlotValue();
  const counts = useNavCounts();
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem(COLLAPSE_KEY) === "1");

  const toggleCollapsed = () =>
    setCollapsed((c) => {
      const next = !c;
      localStorage.setItem(COLLAPSE_KEY, next ? "1" : "0");
      return next;
    });

  return (
    <aside className={cn("orr-sidebar", collapsed && "collapsed")}>
      <div className="orr-sb-sec">
        {NAV.map(({ to, icon: Icon, label, ...rest }) => {
          const badge = "badge" in rest ? rest.badge : undefined;
          const count = badge ? counts[badge] : 0;
          return (
            <button
              key={to}
              type="button"
              title={collapsed ? label : undefined}
              className={cn("orr-sb-item", pathname === to && "active")}
              onClick={() => navigate({ to })}
            >
              <Icon className="size-4 shrink-0" />
              <span className="nm">{label}</span>
              {count > 0 && <span className="orr-sb-badge">{count}</span>}
            </button>
          );
        })}
      </div>

      <div className="orr-sb-slot">{slot}</div>

      <div className="orr-sb-foot">
        <button
          type="button"
          className="orr-sb-collapse"
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          onClick={toggleCollapsed}
        >
          {collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
        </button>
        {!collapsed && (
          <span className="orr-sb-foot-scan">
            <HardDrive className="size-3.5" />
            {lastScan ? `Scanned ${timeAgo(Math.floor(lastScan / 1000))}` : "Not scanned yet"}
          </span>
        )}
      </div>
    </aside>
  );
}
