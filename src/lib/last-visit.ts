import { useSyncExternalStore } from "react";

// Per-view "last visited" timestamps (epoch seconds), so a page can highlight
// what's new since you last looked and the sidebar can badge the count. Backed
// by localStorage and a tiny external store so the badge and the page stay in
// sync the moment a view is marked visited.

export type VisitKey = "feed" | "inbox";

const KEY = "orr.lastVisit";

function parse(): Partial<Record<VisitKey, number>> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

let store = parse();
const listeners = new Set<() => void>();

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Epoch seconds of the last visit to `key`, or 0 if never visited. */
export function lastVisit(key: VisitKey): number {
  return store[key] ?? 0;
}

/** Reactive `lastVisit` — re-renders when the view is marked visited. */
export function useLastVisit(key: VisitKey): number {
  return useSyncExternalStore(
    subscribe,
    () => store[key] ?? 0,
    () => 0,
  );
}

/** Stamp `key` as visited now (clears its "new since last visit" badge). */
export function markVisited(key: VisitKey): void {
  store = { ...store, [key]: Math.floor(Date.now() / 1000) };
  localStorage.setItem(KEY, JSON.stringify(store));
  listeners.forEach((fn) => fn());
}
