//! Local AI: readiness probing, the Settings AI panel's actions (test, pull,
//! llama.cpp model download, cache clear, index rebuild), the incremental
//! semantic index, and bulk summaries. Every AI affordance is gated on
//! `services.ai_ready`.

use gpui::{Context, SharedString};

use crate::shell::{OrreryApp, SnapKind};

impl OrreryApp {
    /// Re-check AI-backend reachability and list installed models.
    pub fn ai_refresh(&mut self, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if matches!(self.services.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.services.ai_status = AiStatus::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available())
                .await
                .unwrap_or(false);
            let status = if up {
                let models = crate::task::run(orrery_core::ai::installed_models())
                    .await
                    .unwrap_or_default();
                AiStatus::Ready(
                    models
                        .into_iter()
                        .map(|(n, sz)| (n.into(), crate::data::human_bytes(sz).into()))
                        .collect(),
                )
            } else {
                AiStatus::Offline
            };
            let _ = this.update(cx, |this, cx| {
                let ready = up && this.config.ai_enabled;
                this.services.ai_status = status;
                this.services.ai_ready = ready;
                // Reachable now → (re)build the semantic index so the palette can
                // search by meaning.
                if ready {
                    this.index_semantic(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Probe the AI backend with a tiny round-trip and report ok/latency in the
    /// Settings AI note. A quick way to confirm the model actually responds.
    pub fn ai_test(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &mut self.settings {
            s.ai_note = "Testing…".into();
        }
        cx.notify();
        let model = self.config.ai_model.clone();
        cx.spawn(async move |this, cx| {
            let note = crate::task::run(async move {
                let start = std::time::Instant::now();
                match orrery_core::ai::generate(&model, "Reply with the single word: pong.").await {
                    Ok(_) => format!("AI responded in {} ms", start.elapsed().as_millis()),
                    Err(e) => format!("AI test failed: {e}"),
                }
            })
            .await
            .unwrap_or_else(|| "AI test failed unexpectedly".into());
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Clear cached AI summaries + embeddings, reporting the counts in the
    /// Settings AI note. Frees the semantic index and stale summaries.
    pub fn ai_clear_cache(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { orrery_core::cache::clear_ai() })
                .await;
            let note = match result {
                Ok((summaries, embeddings)) => {
                    format!("Cleared {summaries} summaries, {embeddings} embeddings")
                }
                Err(e) => format!("Clear failed: {e}"),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                this.load_index_stats(cx);
                // Reload the rows so the cleared summaries leave the cards and
                // drawer (the reload re-derives summaries from the AI cache).
                this.request_snapshot(SnapKind::Reload, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh the semantic-index size line (chunks/repos/bytes) shown in the
    /// Settings AI section.
    pub fn load_index_stats(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let stats = cx
                .background_executor()
                .spawn(async { orrery_core::semantic::index_stats() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.index_stats = Some(stats);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Settings: drop the whole semantic index (rows + skip signatures) and
    /// re-embed the corpus from scratch, reporting in the AI note. Gated on
    /// `aiReady` like every AI affordance.
    pub fn rebuild_semantic_index(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        if let Some(s) = &mut self.settings {
            s.ai_note = "Rebuilding index…".into();
        }
        cx.notify();
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let cleared = cx
                .background_executor()
                .spawn(async { orrery_core::semantic::clear_index() })
                .await;
            let note = match cleared {
                Ok(_) => {
                    let n =
                        crate::task::run(
                            async move { orrery_core::semantic::index_fleet(repos).await },
                        )
                        .await
                        .unwrap_or(0);
                    format!(
                        "Rebuilt index — embedded {n} repo{}",
                        if n == 1 { "" } else { "s" }
                    )
                }
                Err(e) => format!("Rebuild failed: {e}"),
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(s) = &mut this.settings {
                    s.ai_note = note.into();
                }
                this.load_index_stats(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Download a GGUF model for the llama.cpp backend, streaming progress into
    /// the Settings AI note. The file lands in the app-data models dir; on
    /// success the model list refreshes so it can be selected. A non-empty
    /// `sha256` is verified before the file is installed.
    pub fn llama_download(&mut self, url: String, sha256: String, cx: &mut Context<Self>) {
        let url = url.trim().to_string();
        let sha256 = Some(sha256.trim().to_string()).filter(|s| !s.is_empty());
        let note = |this: &mut Self, msg: SharedString| {
            if let Some(s) = &mut this.settings {
                s.ai_note = msg;
            }
        };
        if url.is_empty() {
            note(self, "Enter a model URL to download.".into());
            cx.notify();
            return;
        }
        note(self, "Starting download…".into());
        cx.notify();

        // Progress messages flow from the (core-runtime) download callback to the
        // UI over a channel — the live-wiring pattern, since the callback can't
        // touch the app entity directly.
        let (tx, rx) = async_channel::unbounded::<SharedString>();
        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if let Some(s) = &mut this.settings {
                            s.ai_note = msg;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let tx_progress = tx.clone();
            let result = crate::task::try_run(async move {
                let mut last_pct = u64::MAX;
                orrery_core::llama::download_model_verified(
                    &url,
                    sha256.as_deref(),
                    move |downloaded, total| {
                        let msg = match (downloaded * 100).checked_div(total) {
                            Some(pct) => {
                                if pct == last_pct {
                                    return; // throttle to whole-percent steps
                                }
                                last_pct = pct;
                                format!(
                                    "Downloading… {pct}% ({} / {})",
                                    crate::data::human_bytes(downloaded),
                                    crate::data::human_bytes(total)
                                )
                            }
                            None => {
                                format!("Downloading… {}", crate::data::human_bytes(downloaded))
                            }
                        };
                        let _ = tx_progress.try_send(msg.into());
                    },
                )
                .await
            })
            .await;
            let final_msg = match result {
                Ok(path) => format!("Downloaded {}", path.rsplit('/').next().unwrap_or(&path)),
                Err(e) => format!("Download failed: {e}"),
            };
            let _ = tx.send(final_msg.into()).await;
            drop(tx); // close the channel so the drain task ends
            // Pick up the new model in the installed list.
            let _ = this.update(cx, |this, cx| this.ai_refresh(cx));
        })
        .detach();
    }

    /// One-shot at launch: if AI is enabled and reachable, mark it ready and
    /// kick off the semantic index (so Ctrl+K works without opening Settings).
    pub fn ai_startup(&mut self, cx: &mut Context<Self>) {
        if !self.config.ai_enabled {
            return;
        }
        cx.spawn(async move |this, cx| {
            let up = crate::task::run(orrery_core::ai::available())
                .await
                .unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.services.ai_ready = up;
                if up {
                    this.index_semantic(cx);
                }
                // AI affordances (summaries, Summarize) appear with readiness.
                cx.notify();
            });
        })
        .detach();
    }

    /// (Re)index the semantic corpus (readme/description/topics/notes/commits
    /// per repo) from the current snapshot, on the shared tokio runtime.
    /// Incremental and self-pacing: unchanged (repo, source) pairs skip via
    /// signatures and core throttles the embed bursts, so post-rescan calls
    /// are cheap. Gated on `aiReady` — no model, no indexing, no errors.
    /// Single-flight: a request mid-pass reruns once afterwards, on the
    /// then-current snapshot.
    pub fn index_semantic(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready || !self.passes.index.begin() {
            return;
        }
        let repos = self.repos.clone();
        cx.spawn(async move |this, cx| {
            let _ = crate::task::run(orrery_core::semantic::index_fleet(repos)).await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.index.finish() {
                    this.index_semantic(cx);
                }
            });
        })
        .detach();
    }

    /// Pull (download) a model on the AI backend, then refresh the status.
    pub fn ai_pull(&mut self, model: String, cx: &mut Context<Self>) {
        use crate::views::settings::AiStatus;
        if model.trim().is_empty() || matches!(self.services.ai_status, AiStatus::Pulling(_)) {
            return;
        }
        self.services.ai_status = AiStatus::Pulling(format!("{model} · starting…").into());
        cx.notify();

        // The pull runs on the tokio runtime and streams (status, done, total)
        // back over a channel; a gpui task drains it to update the live %. When
        // the pull finishes the sender drops, closing the channel — our cue to
        // refresh the model list. (The one-shot `task::run` can't stream, hence
        // the detached spawn + channel.)
        let (tx, rx) = async_channel::unbounded::<(String, u64, u64)>();
        let m = model.clone();
        crate::task::spawn_detached(async move {
            let mut last_pct = u64::MAX;
            let _ = orrery_core::ai::pull(&m, |status, done, total| {
                // Throttle to ~1% steps (and every status-only update, total==0).
                match (done * 100).checked_div(total) {
                    Some(pct) if pct == last_pct => {}
                    pct => {
                        last_pct = pct.unwrap_or(u64::MAX);
                        let _ = tx.try_send((status.to_string(), done, total));
                    }
                }
            })
            .await;
        });

        cx.spawn(async move |this, cx| {
            while let Ok((status, done, total)) = rx.recv().await {
                let label = match (done * 100).checked_div(total) {
                    Some(pct) => format!("{model} · {pct}%"),
                    None => format!("{model} · {status}"),
                };
                if this
                    .update(cx, |this, cx| {
                        this.services.ai_status = AiStatus::Pulling(label.into());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            // Channel closed → pull finished. Drop Pulling so the refresh isn't
            // short-circuited, then re-list models.
            let _ = this.update(cx, |this, cx| {
                this.services.ai_status = AiStatus::Unknown;
                this.ai_refresh(cx);
            });
        })
        .detach();
    }

    /// Generate local-AI one-line summaries for every repo (cached by commit, so
    /// repeats are cheap), then reload the grid so the cards show them. Gated on
    /// `ai_ready` — a no-op when AI is unavailable.
    pub fn summarize_all(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready {
            return;
        }
        cx.spawn(async move |this, cx| {
            let updated = crate::task::run(async { orrery_core::summarize::run_cached().await })
                .await
                .unwrap_or(0);
            if updated > 0 {
                let _ = this.update(cx, |this, cx| this.request_snapshot(SnapKind::Reload, cx));
            }
        })
        .detach();
    }
}
