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

#[cfg(test)]
mod tests {
    use super::*;

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
