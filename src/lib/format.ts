import type { Activity } from "@/types";

/** "4h ago" style relative time from a UTC unix-seconds timestamp. */
export function timeAgo(unixSeconds: number, nowSeconds = Math.floor(Date.now() / 1000)): string {
  const s = Math.max(0, nowSeconds - unixSeconds);
  const units: [number, string][] = [
    [60, "s"],
    [60, "m"],
    [24, "h"],
    [7, "d"],
    [4.345, "w"],
    [12, "mo"],
    [Number.POSITIVE_INFINITY, "y"],
  ];
  let value = s;
  let unit = "s";
  for (const [size, label] of units) {
    if (value < size) {
      unit = label;
      break;
    }
    value = value / size;
    unit = label;
  }
  return `${Math.floor(value)}${unit} ago`;
}

/** A representative accent colour per language for the card's status dot. */
export function languageColor(language: string | null): string {
  const map: Record<string, string> = {
    Rust: "#dea584",
    Go: "#00add8",
    TypeScript: "#3178c6",
    JavaScript: "#f1e05a",
    Python: "#3572a5",
    "C++": "#f34b7d",
    C: "#555555",
    Shell: "#89e051",
    Ruby: "#701516",
    Java: "#b07219",
    Swift: "#f05138",
    Kotlin: "#a97bff",
  };
  return (language && map[language]) || "#8b949e";
}

export const ACTIVITY_META: Record<Activity, { label: string; className: string }> = {
  active: { label: "active", className: "text-ok" },
  idle: { label: "idle", className: "text-warn" },
  stale: { label: "stale", className: "text-muted-foreground" },
};
