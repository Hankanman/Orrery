//! GNOME Shell search provider: exposes repos over
//! `org.gnome.Shell.SearchProvider2`, so typing in the Activities overview
//! finds them. Matching is shared with KRunner ([`crate::repo_search`]).
//!
//! GNOME Shell discovers providers from an `.ini` file under
//! `share/gnome-shell/search-providers/` in a *system* data dir (it doesn't
//! read `~/.local/share`), so the packages ship
//! `packaging/orrery-search-provider.ini`. It sets `AutoStart=false`: there's
//! no headless mode to D-Bus-activate, so results appear while Orrery is
//! running (it lives in the tray), like KRunner's.
//!
//! The interface is served on the KRunner service's connection; see
//! [`crate::krunner::spawn`].

use std::collections::HashMap;

use zbus::zvariant::Value;

use crate::repo_search::{self, RepoAction};
use crate::request::{self, AppRequest};

/// The bus name and object path; they must match the shipped `.ini`.
pub(crate) const BUS_NAME: &str = "com.orrery.SearchProvider";
pub(crate) const OBJECT_PATH: &str = "/com/orrery/SearchProvider";

/// GNOME shows a handful of results per provider; more is wasted work.
const MAX_RESULTS: usize = 10;

pub(crate) struct SearchProvider;

#[zbus::interface(name = "org.gnome.Shell.SearchProvider2")]
impl SearchProvider {
    /// Result ids (repo paths) for a fresh search.
    async fn get_initial_result_set(&self, terms: Vec<String>) -> Vec<String> {
        let terms = repo_search::normalise_terms(&terms);
        repo_search::search(&repo_search::repos(), &terms, MAX_RESULTS)
            .into_iter()
            .map(|(r, _)| r.id.clone())
            .collect()
    }

    /// Refine a previous result set as the user keeps typing.
    async fn get_subsearch_result_set(
        &self,
        previous_results: Vec<String>,
        terms: Vec<String>,
    ) -> Vec<String> {
        let terms = repo_search::normalise_terms(&terms);
        let repos = repo_search::repos();
        previous_results
            .into_iter()
            .filter(|id| {
                repos
                    .iter()
                    .any(|r| &r.id == id && repo_search::matches(r, &terms))
            })
            .collect()
    }

    /// Display data for each result id: `id`, `name`, `description`, `gicon`.
    async fn get_result_metas(
        &self,
        identifiers: Vec<String>,
    ) -> Vec<HashMap<String, Value<'static>>> {
        let repos = repo_search::repos();
        identifiers
            .iter()
            .filter_map(|id| repos.iter().find(|r| &r.id == id))
            .map(|r| {
                HashMap::from([
                    ("id".to_string(), Value::from(r.id.clone())),
                    ("name".to_string(), Value::from(r.display_name.clone())),
                    (
                        "description".to_string(),
                        Value::from(repo_search::subtitle(r)),
                    ),
                    ("gicon".to_string(), Value::from("folder".to_string())),
                ])
            })
            .collect()
    }

    /// A result was clicked: open the repo in the IDE, as KRunner's default.
    async fn activate_result(&self, identifier: String, _terms: Vec<String>, _timestamp: u32) {
        if let Err(e) = repo_search::run(RepoAction::Ide, &identifier).await {
            eprintln!("[orrery search] failed to open {identifier}: {e}");
        }
    }

    /// The provider's icon was clicked: show Orrery with the search.
    async fn launch_search(&self, terms: Vec<String>, _timestamp: u32) {
        let query = terms.join(" ");
        if !request::dispatch(AppRequest::Search {
            query,
            activation_token: None,
        }) {
            eprintln!(
                "[orrery search] LaunchSearch ignored: the app hasn't registered a request handler"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_matches_the_served_names() {
        let ini = include_str!("../../../packaging/orrery-search-provider.ini");
        assert!(ini.contains(&format!("BusName={BUS_NAME}\n")));
        assert!(ini.contains(&format!("ObjectPath={OBJECT_PATH}\n")));
        assert!(ini.contains(&format!("DesktopId={}.desktop\n", crate::app_id::APP_ID)));
        assert!(ini.contains("Version=2\n"));
    }
}
