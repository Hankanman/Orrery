# Configuration

Orrery's configuration lives in a plain TOML file at `~/.config/orrery/config.toml`, editable from **Settings** or by hand. The cache and AI data live separately under `~/.local/share/orrery/`.

## Settings sections

The Settings screen switches sections from the sidebar:

- **Workspace roots** — directories scanned for git repos, scan depth, and ignore globs.
- **Launchers** — IDE and terminal-agent presets (see [Launchers](./launchers)).
- **GitHub** — connect for higher rate limits and private-repo enrichment.
- **AI & search** — Ollama endpoint, models, and the cache control (see [Local AI](./local-ai)).
- **Motion** — a reduce-motion toggle that disables all UI animation.

## Workspace roots

Point Orrery at one or more directories. It walks each up to **scan depth** levels deep looking for git repos, skipping anything matching the **ignore** list.

| Setting | Default | Notes |
|---|---|---|
| Roots | `~/dev` | One or more directories to scan. |
| Scan depth | `3` | How many levels deep to descend (1–8). |
| Ignore | `node_modules, .cache, vendor, target, dist, .git` | Comma-separated directory names to skip. |

## Hosts

Public repos enrich without signing in (and an authenticated `gh` CLI is used automatically if present). Connect an account for higher rate limits and private-repo data.

For **self-hosted GitLab**, only explicitly trusted domains are ever sent a token — a repo's remote domain can't trick Orrery into leaking credentials to an arbitrary host.

## Where things live

| Path | Contents |
|---|---|
| `~/.config/orrery/config.toml` | Your settings. |
| `~/.local/share/orrery/cache.sqlite` | Repo snapshot, host enrichment, favorites, AI summaries & embeddings. |

The repo snapshot and host enrichment are rehydrated on launch, so Mission Control paints instantly and visibility/stars survive restarts without a re-fetch.
