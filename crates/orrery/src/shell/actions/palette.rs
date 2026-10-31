//! Command palette (Ctrl+K): open it, keep its result list, run code and
//! semantic searches as the query changes, and act on the chosen item.

use std::rc::Rc;

use gpui::{AppContext, Context, Focusable, SharedString, Window};
use gpui_component::input::InputEvent;

use crate::shell::{OrreryApp, Overlay, View};

impl OrreryApp {
    /// Open the command palette and focus its query field.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = cx.new(|cx| {
            gpui_component::input::InputState::new(window, cx)
                .placeholder("Search repos, run a command…")
        });
        // On each edit: reset the selection, kick off a (debounced) code
        // search, and re-render. Subscribe to `Change` rather than observing
        // the input — it notifies on every cursor blink too, which would
        // reset the selection and re-run the searches twice a second.
        let sub = cx.subscribe(&query, |this, _q, ev: &InputEvent, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }
            if let Some(Overlay::Palette(d)) = &mut this.overlay {
                d.selected = 0;
                d.rev += 1;
            }
            this.trigger_code_search(cx);
            this.trigger_semantic_search(cx);
            cx.notify();
        });
        let fh = query.read(cx).focus_handle(cx);
        self.overlay = Some(Overlay::Palette(crate::palette::PaletteData {
            query,
            selected: 0,
            code: Vec::new(),
            semantic: Vec::new(),
            embeddings: None,
            query_vecs: std::collections::HashMap::new(),
            generation: 0,
            rev: 0,
            items: Default::default(),
            _sub: sub,
        }));
        window.focus(&fh, cx);
        // Load the embedding index once per palette session (a per-keystroke
        // load would dominate recall latency). Gated on AI: with it off the
        // index stays `None` and the palette is name-matching only.
        if self.services.ai_ready {
            cx.spawn(async move |this, cx| {
                let index = cx
                    .background_executor()
                    .spawn(async { orrery_core::semantic::all_embeddings() })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match &mut this.overlay {
                        Some(Overlay::Palette(d)) => {
                            d.embeddings = Some(std::sync::Arc::new(index))
                        }
                        _ => return, // palette closed while loading
                    }
                    // Rank whatever was typed while the index loaded.
                    this.trigger_semantic_search(cx);
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// The current palette result list (actions + repos + code hits). Cached
    /// on the palette and rebuilt only when its inputs change — the query or
    /// search results (`rev`), the rows, or whether a selection exists — so
    /// render, arrow keys, and Enter share one list.
    pub(crate) fn palette_items(&self, cx: &Context<Self>) -> Rc<Vec<crate::palette::PaletteItem>> {
        let Some(Overlay::Palette(d)) = &self.overlay else {
            return Rc::default();
        };
        let key = (d.rev, self.derived.rows_gen(), !self.selected.is_empty());
        if let Some((k, items)) = &*d.items.borrow()
            && *k == key
        {
            return items.clone();
        }
        let items = Rc::new(crate::palette::items(
            &self.rows,
            &d.code,
            &d.semantic,
            &d.query.read(cx).value(),
            key.2,
        ));
        *d.items.borrow_mut() = Some((key, items.clone()));
        items
    }

    /// Move the palette selection by `delta` (wrapping), if it's open.
    pub(crate) fn move_palette(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.palette_items(cx).len();
        if let Some(Overlay::Palette(d)) = &mut self.overlay
            && len > 0
        {
            let i = d.selected as isize + delta;
            d.selected = i.rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    /// Execute the currently-selected palette item (called on Enter).
    pub(crate) fn confirm_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.palette_items(cx);
        if items.is_empty() {
            return;
        }
        let selected = match &self.overlay {
            Some(Overlay::Palette(d)) => d.selected.min(items.len() - 1),
            _ => return,
        };
        if let Some(item) = items.get(selected).cloned() {
            self.run_palette_item(item, cx);
            window.focus(&self.focus, cx);
        }
    }

    /// Debounced cross-repo code search for the current query.
    fn trigger_code_search(&mut self, cx: &mut Context<Self>) {
        let (query, generation) = match &mut self.overlay {
            Some(Overlay::Palette(d)) => {
                d.generation += 1;
                (d.query.read(cx).value().to_string(), d.generation)
            }
            _ => return,
        };
        let paths: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
        cx.spawn(async move |this, cx| {
            // Debounce keystrokes before doing the (expensive) ripgrep pass.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(220))
                .await;
            // Bail if a newer keystroke superseded this search.
            let current = this
                .update(
                    cx,
                    |this, _| matches!(&this.overlay, Some(Overlay::Palette(d)) if d.generation == generation),
                )
                .unwrap_or(false);
            if !current {
                return;
            }
            let results = if query.trim().len() >= 2 {
                cx.background_executor()
                    .spawn(async move {
                        orrery_core::search::search(&query, &paths, 60).unwrap_or_default()
                    })
                    .await
            } else {
                Vec::new()
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(Overlay::Palette(d)) = &mut this.overlay
                    && d.generation == generation {
                        d.code = results.into_iter().map(crate::palette::code_hit).collect();
                        d.rev += 1;
                        cx.notify();
                    }
            });
        })
        .detach();
    }

    /// Debounced semantic recall for the current palette query: embed the
    /// query, rank it against the session-cached embedding index, and surface
    /// the best repo per match with its snippet. Gated on AI being ready (and
    /// the backend supporting embeddings); reuses the code-search generation
    /// for stale-drop. The query embedding is cached per palette session, so
    /// only genuinely new queries hit the backend — and never before the
    /// debounce window passes.
    fn trigger_semantic_search(&mut self, cx: &mut Context<Self>) {
        if !self.services.ai_ready || !orrery_core::ai::embeddings_supported() {
            return;
        }
        let (query, generation) = match &self.overlay {
            Some(Overlay::Palette(d)) => {
                (d.query.read(cx).value().trim().to_string(), d.generation)
            }
            _ => return,
        };
        if query.len() < 2 {
            if let Some(Overlay::Palette(d)) = &mut self.overlay {
                d.semantic.clear();
                d.rev += 1;
            }
            return;
        }
        // Session-cached query vector → rank right away, no debounce/backend.
        if let Some(Overlay::Palette(d)) = &self.overlay
            && let (Some(vec), Some(index)) =
                (d.query_vecs.get(&query).cloned(), d.embeddings.clone())
        {
            let hits = self.semantic_rank(&vec, &index);
            if let Some(Overlay::Palette(d)) = &mut self.overlay {
                d.semantic = hits;
                d.rev += 1;
            }
            cx.notify();
            return;
        }
        let model = self.config.embed_model.clone();
        cx.spawn(async move |this, cx| {
            // Debounce keystrokes before the (slow) embedding round-trip.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(260))
                .await;
            // Bail if a newer keystroke superseded this search.
            let current = this
                .update(cx, |this, _| {
                    matches!(&this.overlay, Some(Overlay::Palette(d)) if d.generation == generation)
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let q = query.clone();
            let vec =
                crate::task::try_run(async move { orrery_core::ai::embed(&model, &q).await }).await;
            let _ = this.update(cx, |this, cx| {
                let Ok(vec) = vec else {
                    // Backend unreachable — recall just finds nothing.
                    if let Some(Overlay::Palette(d)) = &mut this.overlay
                        && d.generation == generation
                    {
                        d.semantic.clear();
                        d.rev += 1;
                        cx.notify();
                    }
                    return;
                };
                let vec = std::sync::Arc::new(vec);
                let index = match &mut this.overlay {
                    Some(Overlay::Palette(d)) => {
                        // Cache even when stale — a re-typed query reuses it.
                        d.query_vecs.insert(query, vec.clone());
                        d.embeddings.clone()
                    }
                    _ => return,
                };
                // Index still loading → its completion re-triggers ranking.
                let Some(index) = index else { return };
                let hits = this.semantic_rank(&vec, &index);
                if let Some(Overlay::Palette(d)) = &mut this.overlay
                    && d.generation == generation
                {
                    d.semantic = hits;
                    d.rev += 1;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Rank the loaded embedding index against a query vector and resolve the
    /// hits to repo rows: best chunk per repo (core `recall`), then
    /// (host, slug) store keys mapped back to repo ids for the palette.
    fn semantic_rank(
        &self,
        query: &[f32],
        index: &[orrery_core::semantic::EmbeddingRow],
    ) -> Vec<crate::palette::SemanticHit> {
        use orrery_core::semantic;
        let by_key: std::collections::HashMap<(String, String), &str> = self
            .repos
            .iter()
            .map(|r| (semantic::repo_key(r), r.id.as_str()))
            .collect();
        semantic::recall(query, index, semantic::MAX_HITS, semantic::MIN_SCORE)
            .into_iter()
            .filter_map(|h| {
                let id = by_key.get(&(h.host.clone(), h.slug.clone()))?;
                let snippet: String = crate::data::oneline(h.content).chars().take(120).collect();
                Some(crate::palette::SemanticHit {
                    id: SharedString::from(id.to_string()),
                    snippet: snippet.into(),
                })
            })
            .collect()
    }

    /// Close the palette and act on `item`.
    pub fn run_palette_item(&mut self, item: crate::palette::PaletteItem, cx: &mut Context<Self>) {
        use crate::palette::{PaletteAction, PaletteItem};
        // Resolve data living in the (about-to-close) palette first.
        let code_abs = match (&item, &self.overlay) {
            (PaletteItem::Code(i), Some(Overlay::Palette(d))) => {
                d.code.get(*i).map(|h| h.abs.to_string())
            }
            _ => None,
        };
        self.close_overlay();
        match item {
            PaletteItem::Action(PaletteAction::Rescan) => self.rescan(cx),
            PaletteItem::Action(PaletteAction::Settings) => self.view = View::Settings,
            // Fleet verbs (#184) — the same run plumbing as the fleet bar
            // (`fleet.rs`); one bulk run at a time is enforced there.
            PaletteItem::Action(PaletteAction::FetchAll) => {
                let repos: Vec<String> = self.rows.iter().map(|r| r.id.to_string()).collect();
                self.run_fleet_repos(crate::fleet::FleetOp::Fetch, repos, cx);
            }
            PaletteItem::Action(PaletteAction::PullBehind) => {
                let repos: Vec<String> = self
                    .rows
                    .iter()
                    .filter(|r| r.behind > 0)
                    .map(|r| r.id.to_string())
                    .collect();
                if repos.is_empty() {
                    self.push_toast(
                        crate::toast::ToastKind::Info,
                        "Nothing behind",
                        Some("Every repo is up to date with its upstream.".into()),
                        cx,
                    );
                } else {
                    self.run_fleet_repos(crate::fleet::FleetOp::Pull, repos, cx);
                }
            }
            PaletteItem::Action(PaletteAction::FetchSelected) => {
                self.run_fleet(crate::fleet::FleetOp::Fetch, cx)
            }
            PaletteItem::Action(PaletteAction::PullSelected) => {
                self.run_fleet(crate::fleet::FleetOp::Pull, cx)
            }
            PaletteItem::Action(PaletteAction::SelectDirty) => {
                self.select_where(|r| r.dirty > 0, "dirty", cx)
            }
            PaletteItem::Action(PaletteAction::SelectBehind) => {
                self.select_where(|r| r.behind > 0, "behind their upstream", cx)
            }
            PaletteItem::Repo(i) | PaletteItem::Recall { row: i, .. } => {
                if let Some(r) = self.rows.get(i) {
                    let _ = orrery_core::launch::launch(&self.config.ide_command, &r.id);
                }
            }
            PaletteItem::Code(_) => {
                if let Some(abs) = code_abs {
                    let _ = orrery_core::launch::launch(&self.config.ide_command, &abs);
                }
            }
        }
        cx.notify();
    }
}
