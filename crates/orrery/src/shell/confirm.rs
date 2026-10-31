//! The arm-then-confirm pattern behind the app's irreversible buttons
//! (Cleanup "Prune", Agents "Remove worktree" / "Discard"): the first click
//! arms the button for one key, a second click while armed confirms, and the
//! arm reverts on its own after a few seconds. Arming another key replaces
//! the current one.

use std::time::Duration;

use gpui::{Context, SharedString};

use super::OrreryApp;

/// How long an armed button waits for its confirming click.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(4);

/// One armed-confirm slot: the armed key, if any, plus a generation bumped on
/// every arm, so a stale revert timer from an earlier arm can't clear a newer
/// one.
#[derive(Default)]
pub(crate) struct ArmedConfirm {
    key: Option<SharedString>,
    generation: u64,
}

impl ArmedConfirm {
    /// The currently armed key, if any (what the view compares against).
    pub(crate) fn armed(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Drop the armed key (the action ran, or the view reloaded).
    pub(crate) fn disarm(&mut self) {
        self.key = None;
    }

    /// Arm `key`, returning this arm's generation for its revert timer.
    fn arm(&mut self, key: SharedString) -> u64 {
        self.key = Some(key);
        self.generation += 1;
        self.generation
    }

    /// The revert timer for `generation` fired: clear the key if that arm is
    /// still the active one. `true` when something was cleared (repaint).
    fn expire(&mut self, generation: u64) -> bool {
        self.generation == generation && self.key.take().is_some()
    }
}

impl OrreryApp {
    /// Arm the confirm slot `slot` picks out for `key`, and start its revert
    /// timer.
    pub(crate) fn arm_confirm(
        &mut self,
        slot: fn(&mut OrreryApp) -> &mut ArmedConfirm,
        key: SharedString,
        cx: &mut Context<Self>,
    ) {
        let generation = slot(self).arm(key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CONFIRM_TIMEOUT).await;
            // Revert only if this timer's arm is still the active one.
            let _ = this.update(cx, |this, cx| {
                if slot(this).expire(generation) {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_replaces_and_disarm_clears() {
        let mut c = ArmedConfirm::default();
        assert_eq!(c.armed(), None);
        c.arm("a".into());
        assert_eq!(c.armed(), Some("a"));
        c.arm("b".into());
        assert_eq!(c.armed(), Some("b"), "arming another key replaces it");
        c.disarm();
        assert_eq!(c.armed(), None);
    }

    #[test]
    fn only_the_latest_arms_timer_reverts_it() {
        let mut c = ArmedConfirm::default();
        let first = c.arm("a".into());
        let second = c.arm("b".into());
        assert!(!c.expire(first), "a stale timer leaves the newer arm alone");
        assert_eq!(c.armed(), Some("b"));
        assert!(c.expire(second));
        assert_eq!(c.armed(), None);
        assert!(!c.expire(second), "nothing left to clear");
    }
}
