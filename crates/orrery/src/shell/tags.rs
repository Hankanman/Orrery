//! Project tags: user labels ("work", "client-x") that group repos across
//! roots. Edited from the drawer Overview, shown as chips on cards and list
//! rows, and listed in the sidebar's TAGS section, which filters the grid.
//!
//! The tags live in the SQLite cache (`cache::all_tags` / `add_tag` /
//! `remove_tag`); this module keeps an in-memory copy keyed by repo id so the
//! render path never touches the database. Edits update the copy at once and
//! persist off the UI thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Context, SharedString};

use super::OrreryApp;

/// The sidebar facet: `(tag, repo count)`, most repos first.
type TagFacets = Rc<Vec<(SharedString, usize)>>;

/// Every repo's tags, plus a change counter for the render-path caches.
#[derive(Default)]
pub struct Tags {
    by_repo: HashMap<SharedString, Vec<SharedString>>,
    /// Bumped on every edit; keys the visible-rows and facet caches.
    generation: u64,
    /// The sidebar facet, cached per (rows, tags) generation.
    facets: RefCell<Option<((u64, u64), TagFacets)>>,
}

impl Tags {
    /// Load every repo's tags from the cache (a quick indexed read, done once
    /// at launch like the saved views).
    pub fn load() -> Self {
        let by_repo = orrery_core::cache::all_tags()
            .into_iter()
            .map(|(id, tags)| (id.into(), tags.into_iter().map(Into::into).collect()))
            .collect();
        Tags {
            by_repo,
            ..Default::default()
        }
    }

    /// This repo's tags, sorted.
    pub fn of(&self, repo: &SharedString) -> &[SharedString] {
        self.by_repo.get(repo).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn insert(&mut self, repo: SharedString, tag: SharedString) -> bool {
        let list = self.by_repo.entry(repo).or_default();
        match list.binary_search(&tag) {
            Ok(_) => false,
            Err(at) => {
                list.insert(at, tag);
                self.generation += 1;
                true
            }
        }
    }

    fn remove(&mut self, repo: &SharedString, tag: &SharedString) -> bool {
        let Some(list) = self.by_repo.get_mut(repo) else {
            return false;
        };
        let before = list.len();
        list.retain(|t| t != tag);
        let changed = list.len() != before;
        if list.is_empty() {
            self.by_repo.remove(repo);
        }
        if changed {
            self.generation += 1;
        }
        changed
    }
}

/// Repos per tag over the repos in `ids` (tags of repos no longer scanned
/// don't count), most repos first, then by name.
fn compute_facets<'a>(
    tags: &Tags,
    ids: impl Iterator<Item = &'a SharedString>,
) -> Vec<(SharedString, usize)> {
    let mut counts: HashMap<&SharedString, usize> = HashMap::new();
    for id in ids {
        for tag in tags.of(id) {
            *counts.entry(tag).or_default() += 1;
        }
    }
    let mut v: Vec<(SharedString, usize)> =
        counts.into_iter().map(|(t, n)| (t.clone(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

impl OrreryApp {
    /// Tag a repo (drawer Overview). The tag is normalised first (trimmed,
    /// lowercased, spaces to `-`); a blank one is ignored.
    pub fn add_tag(&mut self, repo: SharedString, raw: &str, cx: &mut Context<Self>) {
        let Some(tag) = orrery_core::cache::normalise_tag(raw) else {
            return;
        };
        if !self.grid.tags.insert(repo.clone(), tag.clone().into()) {
            return;
        }
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = orrery_core::cache::add_tag(&repo, &tag) {
                    eprintln!("[orrery tags] couldn't save tag {tag:?}: {e}");
                }
            })
            .detach();
        cx.notify();
    }

    /// Remove a tag from a repo (the chip's ×). Clears the sidebar tag filter
    /// if that was the last repo carrying it.
    pub fn remove_tag(&mut self, repo: SharedString, tag: SharedString, cx: &mut Context<Self>) {
        if !self.grid.tags.remove(&repo, &tag) {
            return;
        }
        if self.grid.tag.as_ref() == Some(&tag)
            && !self.grid.tags.by_repo.values().any(|v| v.contains(&tag))
        {
            self.grid.tag = None;
        }
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = orrery_core::cache::remove_tag(&repo, &tag) {
                    eprintln!("[orrery tags] couldn't remove tag {tag:?}: {e}");
                }
            })
            .detach();
        cx.notify();
    }

    /// Toggle the sidebar TAGS filter (clicking the active tag clears it).
    pub fn toggle_tag_filter(&mut self, tag: SharedString, cx: &mut Context<Self>) {
        self.grid.tag = if self.grid.tag.as_ref() == Some(&tag) {
            None
        } else {
            Some(tag)
        };
        cx.notify();
    }

    /// The sidebar TAGS facet, cached until the rows or the tags change.
    pub(super) fn tag_facets(&self) -> TagFacets {
        let key = (self.derived.rows_gen(), self.grid.tags.generation);
        if let Some((k, f)) = &*self.grid.tags.facets.borrow()
            && *k == key
        {
            return f.clone();
        }
        let f = Rc::new(compute_facets(
            &self.grid.tags,
            self.rows.iter().map(|r| &r.id),
        ));
        *self.grid.tags.facets.borrow_mut() = Some((key, f.clone()));
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> SharedString {
        x.to_string().into()
    }

    #[test]
    fn insert_keeps_sorted_unique_and_counts_changes() {
        let mut tags = Tags::default();
        assert!(tags.insert(s("/a"), s("work")));
        assert!(tags.insert(s("/a"), s("client-x")));
        assert!(!tags.insert(s("/a"), s("work")), "duplicate is a no-op");
        assert_eq!(tags.of(&s("/a")), &[s("client-x"), s("work")]);
        assert_eq!(tags.generation(), 2);
        assert!(tags.remove(&s("/a"), &s("work")));
        assert!(!tags.remove(&s("/a"), &s("work")));
        assert!(tags.remove(&s("/a"), &s("client-x")));
        assert!(tags.of(&s("/a")).is_empty());
        assert_eq!(tags.generation(), 4);
    }

    #[test]
    fn facets_count_only_scanned_repos() {
        let mut tags = Tags::default();
        tags.insert(s("/a"), s("work"));
        tags.insert(s("/b"), s("work"));
        tags.insert(s("/b"), s("oss"));
        tags.insert(s("/gone"), s("oss"));
        tags.insert(s("/gone"), s("old"));
        let ids = [s("/a"), s("/b")];
        assert_eq!(
            compute_facets(&tags, ids.iter()),
            vec![(s("work"), 2), (s("oss"), 1)]
        );
    }
}
