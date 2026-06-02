# Local AI

Orrery's AI features run entirely **on-device** via [Ollama](https://ollama.com) — nothing leaves your machine. When AI is off or unreachable, every AI affordance is hidden rather than broken.

## What it powers

- **Repo summaries** — a synthesized "what is this / what's been happening" blurb on each card, generated on demand or in bulk.
- **Commit messages** — drafted from your staged diff in the repo drawer.
- **Changelogs** — summarised from recent history.
- **Daily briefing** — a one-line "here's where things stand" across your workspace.
- **Semantic search** — embeddings-backed search over your repos.

## Setup

1. [Install Ollama](https://ollama.com/download) and make sure it's running (default endpoint `http://localhost:11434`).
2. Pull a small chat model and an embedding model:

```sh
ollama pull qwen3:0.6b
ollama pull nomic-embed-text
```

3. In **Settings → AI & search**, confirm the endpoint, enable AI, and pick your models. Hit **Test** to verify the connection end-to-end.

![Settings — AI & semantic search](/shots/settings-ai.png)

::: tip Model choice
A small model like `qwen3:0.6b` is plenty for the short summaries Orrery generates and keeps things fast. Point the endpoint at a remote Ollama host if you'd rather run the model elsewhere.
:::

## Where data is stored, and clearing it

Summaries and embeddings are cached in `~/.local/share/orrery/cache.sqlite`. A summary is keyed to the repo's last commit, so it regenerates after new work lands. Use **Clear AI cache** in settings to drop all summaries and embeddings.

## Turning it off

Uncheck **Enable AI features**. The grid, drawer, and toolbar drop every AI control — no empty placeholders, no broken buttons.
