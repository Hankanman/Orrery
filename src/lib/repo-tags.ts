// Project/tag grouping for repos, persisted in localStorage. Backed by a tiny
// external store (useSyncExternalStore) so the sidebar facet and the drawer
// editor share one source of truth and stay in sync — no backend, works in the
// browser demo too.

import { useSyncExternalStore } from "react";

export type TagMap = Record<string, string[]>; // repo id → tags

const KEY = "orr.tags";

export function parseTags(raw: string | null): TagMap {
  if (!raw) return {};
  try {
    const data = JSON.parse(raw);
    if (!data || typeof data !== "object" || Array.isArray(data)) return {};
    const out: TagMap = {};
    for (const [id, tags] of Object.entries(data)) {
      if (Array.isArray(tags)) {
        const clean = tags.filter((t): t is string => typeof t === "string" && t.trim().length > 0);
        if (clean.length) out[id] = clean;
      }
    }
    return out;
  } catch {
    return {};
  }
}

/** Trim, drop empties, dedupe (case-sensitive), preserving order. */
export function normalizeTags(tags: string[]): string[] {
  return [...new Set(tags.map((t) => t.trim()).filter(Boolean))];
}

/** [tag, count] pairs, by count desc then name. */
export function tagCounts(map: TagMap): [string, number][] {
  const counts = new Map<string, number>();
  for (const tags of Object.values(map)) for (const t of tags) counts.set(t, (counts.get(t) ?? 0) + 1);
  return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

function load(): TagMap {
  try {
    return parseTags(localStorage.getItem(KEY));
  } catch {
    return {};
  }
}

let state: TagMap = load();
const listeners = new Set<() => void>();

const subscribe = (cb: () => void) => {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
};
const getSnapshot = () => state;

/** Set (or clear) a repo's tags and notify subscribers + localStorage. */
export function setRepoTags(id: string, tags: string[]) {
  const clean = normalizeTags(tags);
  const next = { ...state };
  if (clean.length) next[id] = clean;
  else delete next[id];
  state = next;
  try {
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    // ignore — private mode / no storage
  }
  listeners.forEach((l) => l());
}

export function useRepoTags(): TagMap {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
