// Saved Mission Control filter presets, persisted in localStorage (which the
// Tauri webview keeps across launches, same as the contrib-graph toggle). No
// backend needed, and it works in the browser demo too.

import { useCallback, useState } from "react";
import type { Chip, SortKey, Visibility } from "@/lib/repo-filter";

export interface SavedView {
  id: string;
  name: string;
  root: string; // "all" or a root path
  lang: string | null;
  /** Project/tag filter, if any. Optional for views saved before tags existed. */
  tag?: string | null;
  chips: Chip[];
  visibility: Visibility;
  attention: boolean;
  sort: SortKey;
}

const KEY = "orr.views";

/** Parse the stored JSON into a clean SavedView[] (tolerant of junk). */
export function parseViews(raw: string | null): SavedView[] {
  if (!raw) return [];
  try {
    const data = JSON.parse(raw);
    if (!Array.isArray(data)) return [];
    return data.filter(
      (v): v is SavedView => v && typeof v.id === "string" && typeof v.name === "string" && Array.isArray(v.chips),
    );
  } catch {
    return [];
  }
}

function read(): SavedView[] {
  try {
    return parseViews(localStorage.getItem(KEY));
  } catch {
    return [];
  }
}

function write(views: SavedView[]) {
  try {
    localStorage.setItem(KEY, JSON.stringify(views));
  } catch {
    // ignore — private mode / no storage
  }
}

function makeId(): string {
  return typeof crypto !== "undefined" && crypto.randomUUID ? crypto.randomUUID() : `v_${Date.now()}`;
}

export function useSavedViews() {
  const [views, setViews] = useState<SavedView[]>(read);

  const save = useCallback((view: Omit<SavedView, "id">) => {
    setViews((cur) => {
      const next = [...cur, { ...view, id: makeId() }];
      write(next);
      return next;
    });
  }, []);

  const remove = useCallback((id: string) => {
    setViews((cur) => {
      const next = cur.filter((v) => v.id !== id);
      write(next);
      return next;
    });
  }, []);

  return { views, save, remove };
}
