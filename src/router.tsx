import {
  createRootRoute,
  createRoute,
  createRouter,
  lazyRouteComponent,
} from "@tanstack/react-router";
import { AppShell } from "@/components/layout/AppShell";
import { GridView } from "@/pages/GridView";

const rootRoute = createRootRoute({
  component: AppShell,
});

// GridView is the landing route, so it stays eager. Inbox and Settings load
// their code on first navigation, keeping them out of the initial bundle.
const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: GridView,
});

const inboxRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/inbox",
  component: lazyRouteComponent(() => import("@/pages/InboxView"), "InboxView"),
});

const feedRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/feed",
  component: lazyRouteComponent(() => import("@/pages/FeedView"), "FeedView"),
});

const exploreRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/explore",
  component: lazyRouteComponent(() => import("@/pages/ExploreView"), "ExploreView"),
});

const toolsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/tools",
  component: lazyRouteComponent(() => import("@/pages/ToolsView"), "ToolsView"),
});

const janitorRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/janitor",
  component: lazyRouteComponent(() => import("@/pages/JanitorView"), "JanitorView"),
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: lazyRouteComponent(() => import("@/pages/SettingsView"), "SettingsView"),
});

const routeTree = rootRoute.addChildren([indexRoute, inboxRoute, feedRoute, exploreRoute, toolsRoute, janitorRoute, settingsRoute]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
