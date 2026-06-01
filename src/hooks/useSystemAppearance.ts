import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { applyAppearance, type Appearance } from "@/lib/appearance";

/**
 * Keep the document in sync with the desktop's theme + accent colour.
 *
 * - On mount, asks the Rust core for the current appearance (XDG portal).
 * - Subscribes to `appearance-changed` so live theme/accent flips apply.
 * - Falls back to `prefers-color-scheme` when not running under Tauri or when
 *   the desktop reports "no preference".
 */
export function useSystemAppearance(): void {
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;

    const refetch = () => {
      invoke<Appearance>("get_appearance")
        .then(applyAppearance)
        .catch(() =>
          applyAppearance({
            colorScheme: null,
            accent: null,
            windowBg: null,
            windowFg: null,
            baseBg: null,
          }),
        );
    };

    refetch();

    listen<Appearance>("appearance-changed", (event) => applyAppearance(event.payload))
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* not in Tauri — the media-query fallback below covers us */
      });

    // Covers the "no preference" case and the non-Tauri (browser) preview.
    const mql = window.matchMedia("(prefers-color-scheme: dark)");
    mql.addEventListener("change", refetch);

    return () => {
      disposed = true;
      unlisten?.();
      mql.removeEventListener("change", refetch);
    };
  }, []);
}
