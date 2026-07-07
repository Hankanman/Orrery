//! Agent dispatch naming (#185): deterministic-ish branch/worktree names for
//! "dispatch an agent on a fresh worktree", plus where those worktrees live.
//!
//! Dispatched worktrees are created under the app data dir
//! (`~/.local/share/orrery/worktrees/`), *not* inside or beside the repo:
//! a worktree inside the repo would make the origin permanently dirty (an
//! untracked `.worktrees/` dir) and can confuse the repo's own lint/test
//! tooling, while a sibling dir would be picked up by the root scanner as a
//! separate repo. The pairing back to the origin repo is recorded in the
//! SQLite cache (`cache::record_agent_worktree`), so nothing depends on the
//! path being discoverable.

use std::path::PathBuf;

/// The generated names for one worktree dispatch.
#[derive(Debug, Clone)]
pub struct DispatchNames {
    /// Branch the agent works on: `agent/<slug>-<rand>`.
    pub branch: String,
    /// git worktree name (also the leaf directory name): `agent-<slug>-<rand>`.
    /// Flat (no `/`) because libgit2 uses it as a directory name under
    /// `.git/worktrees/`.
    pub worktree: String,
}

/// Kebab-case slug of a task prompt, capped to `MAX_SLUG` chars. Empty/symbolic
/// prompts fall back to `"task"` so the names stay valid refs.
pub fn slugify(prompt: &str) -> String {
    const MAX_SLUG: usize = 28;
    let mut slug = String::new();
    let mut last_dash = true; // suppress a leading dash
    for c in prompt.chars() {
        if slug.len() >= MAX_SLUG {
            break;
        }
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "task".into()
    } else {
        slug
    }
}

/// A 4-hex-char uniqueness suffix (time + pid hashed) — enough to keep two
/// dispatches of the same prompt apart without pulling in a rand dependency.
fn short_rand() -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    std::process::id().hash(&mut h);
    format!("{:04x}", h.finish() & 0xffff)
}

/// Generate the branch + worktree names for dispatching `prompt`.
pub fn names(prompt: &str) -> DispatchNames {
    let (slug, rand) = (slugify(prompt), short_rand());
    DispatchNames {
        branch: format!("agent/{slug}-{rand}"),
        worktree: format!("agent-{slug}-{rand}"),
    }
}

/// Destination directory for a dispatched worktree:
/// `<data>/orrery/worktrees/<repo-basename>-<worktree-name>`. `None` only when
/// the platform has no data dir.
pub fn worktree_dest(repo_id: &str, worktree_name: &str) -> Option<PathBuf> {
    let base = repo_id.trim_end_matches('/').rsplit('/').next()?;
    Some(
        dirs::data_dir()?
            .join("orrery")
            .join("worktrees")
            .join(format!("{base}-{worktree_name}")),
    )
}

/// What one agents-poll observation means for a dispatched worktree's
/// lifecycle, given the persisted state (see `cache::AgentWorktree`). Pure —
/// the caller applies the matching cache write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTransition {
    /// Nothing to record: never observed alive (dispatch may have failed to
    /// launch, or the app never saw the session), or already marked finished.
    None,
    /// A session is alive — record the sighting.
    SeenAlive,
    /// A session is alive again in a worktree already marked finished — the
    /// outcome is back in flight (e.g. "Resume"); clear the finished state.
    Resumed,
    /// A session that WAS observed alive is now gone — the finish event.
    /// Fires exactly once: after the caller persists `finished_at`, later
    /// polls fall into [`SessionTransition::None`]. Because `last_seen_alive`
    /// is persisted, this also catches sessions that ended while Orrery was
    /// closed (detected on the first poll of the next launch).
    Finished,
}

/// Classify one poll observation: `alive` is whether an agent process is
/// currently running inside the worktree; the other two are the persisted
/// `AgentWorktree` fields.
pub fn session_transition(
    alive: bool,
    last_seen_alive: i64,
    finished_at: i64,
) -> SessionTransition {
    match (alive, last_seen_alive > 0, finished_at > 0) {
        (true, _, true) => SessionTransition::Resumed,
        (true, _, false) => SessionTransition::SeenAlive,
        (false, true, false) => SessionTransition::Finished,
        (false, _, _) => SessionTransition::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_transitions_cover_the_lifecycle() {
        use SessionTransition::*;
        // Never observed alive and not running now → nothing to record (a
        // failed launch must not look like a finished session).
        assert_eq!(session_transition(false, 0, 0), None);
        // Running → record the sighting.
        assert_eq!(session_transition(true, 0, 0), SeenAlive);
        assert_eq!(session_transition(true, 100, 0), SeenAlive);
        // Was alive, now gone, not yet marked → the finish event.
        assert_eq!(session_transition(false, 100, 0), Finished);
        // Already marked finished → no repeat finish event.
        assert_eq!(session_transition(false, 100, 200), None);
        // Alive again after a finish → resumed (clear the finished state).
        assert_eq!(session_transition(true, 100, 200), Resumed);
    }

    #[test]
    fn slugify_kebabs_and_caps() {
        assert_eq!(slugify("Fix the flaky CI tests!"), "fix-the-flaky-ci-tests");
        assert_eq!(slugify("  weird   spacing\n\t"), "weird-spacing");
        assert_eq!(slugify(""), "task");
        assert_eq!(slugify("!!!"), "task");
        let long = slugify("a very long prompt that should be truncated somewhere sensible");
        assert!(long.len() <= 28, "slug too long: {long}");
        assert!(!long.ends_with('-'), "no trailing dash: {long}");
    }

    #[test]
    fn names_shape() {
        let n = names("Fix the tests");
        assert!(n.branch.starts_with("agent/fix-the-tests-"), "{}", n.branch);
        assert!(
            n.worktree.starts_with("agent-fix-the-tests-"),
            "{}",
            n.worktree
        );
        assert!(
            !n.worktree.contains('/'),
            "worktree name must be flat: {}",
            n.worktree
        );
        // 4-hex-char suffix.
        let suffix = n.branch.rsplit('-').next().unwrap();
        assert_eq!(suffix.len(), 4);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn dest_is_under_data_dir_and_keyed_by_repo() {
        let dest = worktree_dest("/home/u/dev/myrepo", "agent-x-abcd").unwrap();
        let s = dest.to_string_lossy();
        assert!(s.contains("orrery"));
        assert!(s.ends_with("worktrees/myrepo-agent-x-abcd"), "{s}");
    }
}
