// Fictional fixtures for the demo / browser build (no Tauri backend). The
// Inbox, Feed, and Explore views fall back to these when `isTauri()` is false,
// so the web demo — and the documentation screenshots — show realistic data.
// All entries are invented and public; nothing here reflects real accounts.

import type { FeedItem, InboxItem, NotificationItem, RemoteRepo } from "@/lib/ipc";

const NOW = Math.floor(Date.now() / 1000);
const HOUR = 3600;
const DAY = 86400;

export const MOCK_INBOX: InboxItem[] = [
  { kind: "pr", title: "Borrow desktop surfaces from kdeglobals", repo: "Hankanman/Orrery", url: "#", number: 142, draft: false, host: "github" },
  { kind: "review", title: "Virtualize the repo grid and branch lists", repo: "Hankanman/Orrery", url: "#", number: 138, draft: false, host: "github" },
  { kind: "pr", title: "Cache host enrichment across restarts", repo: "acme/api-gateway", url: "#", number: 77, draft: true, host: "gitlab" },
  { kind: "issue", title: "Card actions need distinct colours per launcher", repo: "Hankanman/Orrery", url: "#", number: 131, draft: false, host: "github" },
  { kind: "issue", title: "Full-colour language logos on cards", repo: "Hankanman/Orrery", url: "#", number: 129, draft: false, host: "github" },
];

export const MOCK_NOTIFICATIONS: NotificationItem[] = [
  { title: "You were mentioned in #142", repo: "Hankanman/Orrery", reason: "mention", kind: "Issue" },
  { title: "Review requested on #138", repo: "Hankanman/Orrery", reason: "review_requested", kind: "PullRequest" },
  { title: "CI passed on main", repo: "acme/web-dashboard", reason: "ci_activity", kind: "CheckSuite" },
];

export const MOCK_FEED: FeedItem[] = [
  { kind: "release", actor: null, repo: "tauri-apps/tauri", title: "Tauri 2.1.0", tag: "v2.1.0", detail: "Window effects, improved tray APIs, and a slimmer runtime.", url: "#", timestamp: NOW - 2 * HOUR, prerelease: false, host: "github" },
  { kind: "release", actor: null, repo: "vitejs/vite", title: "Vite 7.0", tag: "v7.0.0", detail: "Rolldown-powered builds and faster cold starts.", url: "#", timestamp: NOW - 6 * HOUR, prerelease: false, host: "github" },
  { kind: "starred", actor: "octocat", repo: "rust-lang/rustlings", title: "", tag: "", detail: "Small exercises to get you used to reading and writing Rust.", url: "#", timestamp: NOW - 9 * HOUR, prerelease: false, host: "github" },
  { kind: "release", actor: null, repo: "ollama/ollama", title: "Ollama 0.5.0", tag: "v0.5.0", detail: "Structured outputs and an embeddings endpoint.", url: "#", timestamp: NOW - 1 * DAY, prerelease: false, host: "github" },
  { kind: "created", actor: "torvalds", repo: "torvalds/uemacs", title: "", tag: "", detail: "A tiny, fast Emacs clone.", url: "#", timestamp: NOW - 2 * DAY, prerelease: false, host: "github" },
  { kind: "forked", actor: "octocat", repo: "tldr-pages/tldr", title: "", tag: "", detail: "Collaborative cheatsheets for console commands.", url: "#", timestamp: NOW - 3 * DAY, prerelease: false, host: "github" },
];

export const MOCK_STARRED: RemoteRepo[] = [
  { slug: "tauri-apps/tauri", description: "Build smaller, faster, and more secure desktop applications with a web frontend.", stars: 82000, language: "Rust", cloneUrl: "https://github.com/tauri-apps/tauri.git", host: "github" },
  { slug: "vitejs/vite", description: "Next generation frontend tooling. It's fast!", stars: 68000, language: "TypeScript", cloneUrl: "https://github.com/vitejs/vite.git", host: "github" },
  { slug: "ollama/ollama", description: "Get up and running with large language models locally.", stars: 96000, language: "Go", cloneUrl: "https://github.com/ollama/ollama.git", host: "github" },
  { slug: "rust-lang/rust", description: "Empowering everyone to build reliable and efficient software.", stars: 97000, language: "Rust", cloneUrl: "https://github.com/rust-lang/rust.git", host: "github" },
  { slug: "tailwindlabs/tailwindcss", description: "A utility-first CSS framework for rapid UI development.", stars: 83000, language: "CSS", cloneUrl: "https://github.com/tailwindlabs/tailwindcss.git", host: "github" },
  { slug: "BurntSushi/ripgrep", description: "Recursively searches directories for a regex pattern while respecting your gitignore.", stars: 49000, language: "Rust", cloneUrl: "https://github.com/BurntSushi/ripgrep.git", host: "github" },
];
