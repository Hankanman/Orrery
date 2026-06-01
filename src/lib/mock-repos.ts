import type { Repo } from "@/types";

// Placeholder data so the grid renders during Phase 0. Replaced by the real
// scanner (Rust IPC) in Phase 1 — see the "Repo scanner" issue.
const HOUR = 3600;
const DAY = 24 * HOUR;
const now = Math.floor(Date.UTC(2026, 5, 1) / 1000); // fixed for deterministic UI

export const MOCK_REPOS: Repo[] = [
  {
    id: "/home/seb/dev/personal/Orrery",
    displayName: "Orrery",
    slug: "Hankanman/Orrery",
    path: "~/dev/personal/Orrery",
    description: "every repo in orbit — a Linux-native command center for your git repos.",
    language: "Rust",
    git: { branch: "main", ahead: 2, behind: 0, dirty: 7 },
    lastCommitUnix: now - 1 * HOUR,
    activity: "active",
  },
  {
    id: "/home/seb/dev/personal/dotfiles",
    displayName: "dotfiles",
    slug: "Hankanman/dotfiles",
    path: "~/dev/personal/dotfiles",
    description: "My Linux environment — zsh, neovim, hyprland, the works.",
    language: "Shell",
    git: { branch: "main", ahead: 0, behind: 0, dirty: 0 },
    lastCommitUnix: now - 5 * DAY,
    activity: "idle",
  },
  {
    id: "/home/seb/dev/work/api-gateway",
    displayName: "API Gateway",
    slug: "acme/api-gateway",
    path: "~/dev/work/api-gateway",
    description: "Edge gateway: auth, rate limiting, and request routing.",
    language: "Go",
    git: { branch: "feat/jwt-rotation", ahead: 4, behind: 1, dirty: 3 },
    lastCommitUnix: now - 3 * HOUR,
    activity: "active",
  },
  {
    id: "/home/seb/dev/play/raymarcher",
    displayName: "raymarcher",
    slug: null,
    path: "~/dev/play/raymarcher",
    description: "A toy SDF raymarcher. No remote — local experiment.",
    language: "C++",
    git: { branch: "main", ahead: 0, behind: 0, dirty: 12 },
    lastCommitUnix: now - 40 * DAY,
    activity: "stale",
  },
  {
    id: "/home/seb/dev/work/web-dashboard",
    displayName: "Web Dashboard",
    slug: "acme/web-dashboard",
    path: "~/dev/work/web-dashboard",
    description: "Customer-facing analytics dashboard.",
    language: "TypeScript",
    git: { branch: "main", ahead: 0, behind: 3, dirty: 0 },
    lastCommitUnix: now - 8 * HOUR,
    activity: "active",
  },
  {
    id: "/home/seb/dev/personal/synth",
    displayName: "synth",
    slug: "Hankanman/synth",
    path: "~/dev/personal/synth",
    description: "Modular software synthesizer with a node-based patch editor.",
    language: "Rust",
    git: { branch: "dev", ahead: 11, behind: 0, dirty: 1 },
    lastCommitUnix: now - 2 * DAY,
    activity: "idle",
  },
];
