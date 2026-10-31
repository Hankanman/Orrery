//! Explore: load the starred-repo browser and clone a starred repo into the
//! first root.

use gpui::{Context, SharedString};

use crate::shell::OrreryApp;

impl OrreryApp {
    /// Load the starred-repo browser over the network.
    pub fn load_starred(&mut self, cx: &mut Context<Self>) {
        use crate::views::explore::{ExploreState, star_row};
        self.explore = ExploreState::Loading;
        self.passes.explore_seq += 1;
        let seq = self.passes.explore_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res =
                crate::task::try_run(async { orrery_core::inbox::github_starred().await }).await;
            let _ = this.update(cx, |this, cx| {
                if this.passes.explore_seq != seq {
                    return; // a refresh superseded this load
                }
                this.explore = match res {
                    Ok(repos) => ExploreState::Ready(repos.into_iter().map(star_row).collect()),
                    Err(e) => ExploreState::Error(e.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Clone a starred repo into the first root, then rescan so it appears.
    pub fn clone_starred(
        &mut self,
        slug: SharedString,
        clone_url: SharedString,
        name: SharedString,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.config.roots.first().cloned() else {
            return;
        };
        self.explore_ui.cloning.insert(slug.clone());
        self.explore_ui.errors.remove(&slug);
        // A keyed Progress toast tracks the clone; the completion below upserts
        // the same key to Success/Error, so the op resolves its own toast.
        let toast_key = SharedString::from(format!("clone:{slug}"));
        self.upsert_toast(
            toast_key.clone(),
            crate::toast::ToastKind::Progress,
            format!("Cloning {slug}…"),
            None,
            cx,
        );
        let dest = orrery_core::scan::expand(&root)
            .join(name.as_ref())
            .to_string_lossy()
            .into_owned();
        let url = clone_url.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if std::path::Path::new(&dest).exists() {
                        Ok(())
                    } else {
                        orrery_core::git_ops::clone(&url, &dest).map(|_| ())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                use crate::toast::ToastKind;
                match result {
                    Ok(()) => {
                        this.upsert_toast(
                            toast_key,
                            ToastKind::Success,
                            format!("Cloned {slug}"),
                            None,
                            cx,
                        );
                        // Keep the card on "cloning" until a rescan has picked
                        // the repo up (then it shows as cloned), and watch the
                        // freshly cloned repo for live changes too.
                        this.passes.snapshot.settle_clones.push(slug.clone());
                        this.rescan(cx);
                    }
                    Err(e) => {
                        this.explore_ui.cloning.remove(&slug);
                        let msg = SharedString::from(format!("Failed: {e}"));
                        // Keep the inline card error AND raise a toast, so the
                        // failure is visible even away from the Explore view.
                        this.explore_ui.errors.insert(slug.clone(), msg.clone());
                        this.upsert_toast(
                            toast_key,
                            ToastKind::Error,
                            format!("Clone failed: {slug}"),
                            Some(msg),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}
