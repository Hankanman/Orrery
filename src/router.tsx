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

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: lazyRouteComponent(() => import("@/pages/SettingsView"), "SettingsView"),
});

const routeTree = rootRoute.addChildren([indexRoute, inboxRoute, feedRoute, settingsRoute]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
