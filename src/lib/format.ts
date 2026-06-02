import type { Activity, Repo, RepoStatus } from "@/types";

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
    HTML: "#e34c26",
    CSS: "#563d7c",
    PHP: "#4f5d95",
    Dart: "#00b4ab",
    Elixir: "#6e4a7e",
    Haskell: "#5e5086",
    Scala: "#c22d40",
    Lua: "#000080",
    Zig: "#ec915c",
    Nim: "#ffc200",
    "C#": "#178600",
    Vue: "#41b883",
    Svelte: "#ff3e00",
    Clojure: "#db5855",
    Elm: "#60b5cc",
    Erlang: "#b83998",
    OCaml: "#3be133",
    Perl: "#0298c3",
    R: "#198ce7",
    Julia: "#a270ba",
    Crystal: "#c8ccd1",
    Nix: "#7e7eff",
    Solidity: "#aa6746",
    Markdown: "#083fa1",
    Dockerfile: "#384d54",
    Makefile: "#427819",
    TOML: "#9c4221",
    JSON: "#40484f",
    YAML: "#cb171e",
  };
  return (language && map[language]) || "#8b949e";
}

export const ACTIVITY_META: Record<Activity, { label: string; className: string }> = {
  active: { label: "active", className: "text-ok" },
  idle: { label: "idle", className: "text-warn" },
  stale: { label: "stale", className: "text-muted-foreground" },
};

/**
 * Derive the single status that drives a card's accent, in priority order:
 * uncommitted work > behind upstream > no recent activity > clean.
 */
export function repoStatus(repo: Repo): RepoStatus {
  if (repo.git.dirty > 0) return "dirty";
  if (repo.git.behind > 0) return "behind";
  if (repo.activity === "stale") return "stale";
  return "clean";
}

/** Compact star count: 842 → "842", 1284 → "1.3k". */
export function formatStars(stars: number): string {
  return stars >= 1000 ? `${(stars / 1000).toFixed(1)}k` : String(stars);
}
