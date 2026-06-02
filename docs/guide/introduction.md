# Introduction

**Orrery** points at the directories where you keep your projects, discovers every git repo inside them, and lays them out in a dark, dense "mission control" grid. Each card fuses three sources of truth:

1. **Local git** — branch, ahead/behind, uncommitted changes, last commit, detected language
2. **Your git host** *(GitHub & GitLab, incl. self-hosted)* — stars, topics, releases, issues, visibility
3. **Local AI** — a synthesized "what is this / what's been happening" blurb, generated on-device

…and every card is a launchpad: one click to open the repo in your IDE, or to drop a terminal coding agent (Claude Code, Aider, Codex, …) straight into it.

![Mission Control — the repo grid](/shots/mission-control.png)

## Why?

There's no great *workspace dashboard* for Linux. GitKraken is heavy and git-focused; GitHub Desktop has no Linux build and is single-repo. Orrery is the at-a-glance morning view of everything you're working on — and the fastest way to jump back in.

## Status

::: warning EARLY DEVELOPMENT
Orrery is in active early development and is built from source — there's no packaged release yet. The [Getting started](./getting-started) guide covers building it locally. Expect rough edges.
:::

## Stack

| Layer | Choice |
|---|---|
| Shell | [Tauri 2](https://tauri.app) — Rust core ↔ webview |
| Frontend | Vite + React + TypeScript + Tailwind + [shadcn/ui](https://ui.shadcn.com) |
| Git | `git2` (libgit2, vendored) |
| Persistence | SQLite + TOML config (XDG dirs) |
| Hosts | GitHub + GitLab REST/GraphQL (incl. self-hosted) |
| Local AI | [Ollama](https://ollama.com) over HTTP |

## How it fits together

Orrery is a Tauri 2 app: a Rust core does the heavy lifting (scanning, git, host APIs, caching, AI calls) and exposes it to a React webview over IPC. A SQLite cache persists the repo snapshot and host enrichment so the grid **paints instantly on launch** and keeps working offline. Configuration lives in a plain TOML file under `~/.config/orrery/`.

Read on for [building from source](./getting-started), or jump into the [feature tour](./mission-control).
