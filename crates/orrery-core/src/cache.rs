//! SQLite-backed cache (`~/.local/share/orrery/cache.sqlite`): persists user
//! favorites and a snapshot of scanned repos so the grid paints instantly on
//! launch and survives offline. Connections are opened per call — these are
//! low-frequency operations and SQLite handles the locking.
//!
//! The `*_on(conn)` helpers take a connection so the logic is unit-testable
//! against an in-memory database (see the tests module).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::Connection;

use crate::model::{HostInfo, Repo};

fn db_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery").join("cache.sqlite"))
}

// Bump when a cached payload's shape changes so stale rows are dropped rather
// than silently deserialized with defaulted fields. v2 added HostInfo.private —
// older rows lack the key and would otherwise read back as `private: false`,
// making private repos look public until the 6h TTL lapsed. v3 keys host_cache
// by (host, slug) instead of slug alone, so two repos with the same
// "owner/repo" slug on different hosts (e.g. github.com + a self-hosted
// GitLab) no longer share one cached row. v4 re-shapes `embeddings` from one
// JSON vector per repo id (#41) to chunked f32-blob rows keyed
// (host, slug, source, chunk_ix) for semantic fleet recall (#186).
const CACHE_SCHEMA: i64 = 4;

/// `host_cache` DDL, shared by [`init`] and [`migrate`] (which drops and
/// recreates the table on a schema bump — it's just a cache).
const HOST_CACHE_DDL: &str = "CREATE TABLE IF NOT EXISTS host_cache (
    host TEXT NOT NULL,
    slug TEXT NOT NULL,
    data TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (host, slug));";

/// `embeddings` DDL — the semantic-recall index (#186). A repo contributes
/// several `source` kinds ('readme', 'description', 'notes', 'commits', …),
/// each split into chunks; `vector` is a little-endian f32 blob (see
/// `semantic::encode_vector`). Keyed per (host, slug) like `host_cache`, per
/// the #159 lesson. The DDL lives here so schema versioning stays in one
/// place; the row helpers live in `crate::semantic`.
const EMBEDDINGS_DDL: &str = "CREATE TABLE IF NOT EXISTS embeddings (
    host TEXT NOT NULL,
    slug TEXT NOT NULL,
    source TEXT NOT NULL,
    chunk_ix INTEGER NOT NULL,
    content TEXT NOT NULL,
    vector BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (host, slug, source, chunk_ix));";

/// `ci_cache` DDL — the central CI-status pass's store (#183): the latest
/// default-branch CI state per remote, keyed (host domain, slug) like
/// `host_cache` per the #159 lesson. `url` is the failing/latest run's web
/// page when the API offered one. Adding this table needed no CACHE_SCHEMA
/// bump (nothing existing changed shape — `CREATE TABLE IF NOT EXISTS`
/// covers old databases), but it's in [`migrate`]'s drop list so future
/// shape changes clear it with the rest.
const CI_CACHE_DDL: &str = "CREATE TABLE IF NOT EXISTS ci_cache (
    host TEXT NOT NULL,
    slug TEXT NOT NULL,
    state TEXT NOT NULL,
    url TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (host, slug));";

pub(crate) fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS favorites (id TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS repos (id TEXT PRIMARY KEY, data TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS ai_cache (id TEXT PRIMARY KEY, summary TEXT NOT NULL, last_commit INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS notes (id TEXT PRIMARY KEY, text TEXT NOT NULL DEFAULT '', last_seen_sha TEXT NOT NULL DEFAULT '', last_seen_unix INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS agent_worktrees (
             worktree_path TEXT PRIMARY KEY,
             repo_id TEXT NOT NULL,
             branch TEXT NOT NULL,
             worktree_name TEXT NOT NULL,
             prompt TEXT NOT NULL,
             created_at INTEGER NOT NULL);",
    )?;
    conn.execute_batch(HOST_CACHE_DDL)?;
    conn.execute_batch(EMBEDDINGS_DDL)?;
    conn.execute_batch(CI_CACHE_DDL)?;
    migrate(conn)
}

/// Drop schema-sensitive cached payloads when CACHE_SCHEMA changes — today
/// host enrichment and the embedding index; favorites/repos/AI summaries are
/// untouched. The tables are dropped (not just emptied) because their shape
/// can change between versions — v3 changed host_cache's primary key, v4
/// changed embeddings' entire shape.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: Option<i64> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'cache_schema'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|s| s.parse().ok());
    if current != Some(CACHE_SCHEMA) {
        conn.execute("DROP TABLE IF EXISTS host_cache", [])?;
        conn.execute("DROP TABLE IF EXISTS embeddings", [])?;
        conn.execute("DROP TABLE IF EXISTS ci_cache", [])?;
        conn.execute_batch(HOST_CACHE_DDL)?;
        conn.execute_batch(EMBEDDINGS_DDL)?;
        conn.execute_batch(CI_CACHE_DDL)?;
        // The dropped vectors must re-embed; stale signatures would skip them.
        conn.execute("DELETE FROM meta WHERE key LIKE 'embed_sig:%'", [])?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_schema', ?1)",
            [CACHE_SCHEMA.to_string()],
        )?;
    }
    Ok(())
}

/// Open (and initialize) the on-disk cache. `pub(crate)` so `crate::semantic`'s
/// public wrappers can follow the same open-per-call pattern for the
/// `embeddings` table without cache.rs owning its row logic.
pub(crate) fn open() -> Result<Connection, String> {
    let path = db_path().ok_or("no data directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    init(&conn).map_err(|e| e.to_string())?;
    Ok(conn)
}

fn favorites_on(conn: &Connection) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT id FROM favorites") else {
        return HashSet::new();
    };
    // Bind the query result so its temporary drops before `stmt` (inlining it
    // into the `match` would extend the borrow past `stmt`'s drop — E0597).
    let rows = stmt.query_map([], |row| row.get::<_, String>(0));
    match rows {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => HashSet::new(),
    }
}

fn set_favorite_on(conn: &Connection, id: &str, favorite: bool) -> rusqlite::Result<()> {
    if favorite {
        conn.execute("INSERT OR IGNORE INTO favorites (id) VALUES (?1)", [id])?;
    } else {
        conn.execute("DELETE FROM favorites WHERE id = ?1", [id])?;
    }
    Ok(())
}

fn store_repos_on(conn: &mut Connection, repos: &[Repo]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM repos", [])?;
    {
        let mut stmt = tx.prepare("INSERT INTO repos (id, data) VALUES (?1, ?2)")?;
        for repo in repos {
            // A Repo is plain serde-derivable data, so this should never fail;
            // if it ever does, skip that row rather than persisting an empty
            // string that would silently fail to deserialize on load.
            match serde_json::to_string(repo) {
                Ok(json) => {
                    stmt.execute(rusqlite::params![repo.id, json])?;
                }
                Err(e) => eprintln!("[cache] skipping unserializable repo {}: {e}", repo.id),
            }
        }
    }
    tx.commit()
}

fn load_repos_on(conn: &Connection) -> Vec<Repo> {
    let favs = favorites_on(conn);
    let Ok(mut stmt) = conn.prepare("SELECT data FROM repos") else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| row.get::<_, String>(0));
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter_map(|json| serde_json::from_str::<Repo>(&json).ok())
            .map(|mut r| {
                r.favorite = favs.contains(&r.id);
                r
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// The set of repo ids the user has favorited.
pub fn favorites() -> HashSet<String> {
    match open() {
        Ok(conn) => favorites_on(&conn),
        Err(_) => HashSet::new(),
    }
}

/// Toggle a repo's favorite flag, returning the new state.
pub fn set_favorite(id: &str, favorite: bool) -> Result<bool, String> {
    let conn = open()?;
    set_favorite_on(&conn, id, favorite).map_err(|e| e.to_string())?;
    Ok(favorite)
}

/// Replace the cached repo snapshot.
pub fn store_repos(repos: &[Repo]) -> Result<(), String> {
    let mut conn = open()?;
    store_repos_on(&mut conn, repos).map_err(|e| e.to_string())
}

/// Load the cached repo snapshot (for instant paint before a fresh scan).
pub fn load_repos() -> Vec<Repo> {
    match open() {
        Ok(conn) => load_repos_on(&conn),
        Err(_) => Vec::new(),
    }
}

/// Cached host enrichment for a (host domain, slug) pair, if newer than
/// `max_age_secs`.
pub fn cached_host_info(host: &str, slug: &str, max_age_secs: i64, now: i64) -> Option<HostInfo> {
    let conn = open().ok()?;
    let mut stmt = conn
        .prepare("SELECT data, fetched_at FROM host_cache WHERE host = ?1 AND slug = ?2")
        .ok()?;
    let (json, fetched_at): (String, i64) = stmt
        .query_row([host, slug], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?;
    if now.saturating_sub(fetched_at) > max_age_secs {
        return None;
    }
    serde_json::from_str(&json).ok()
}

fn fresh_host_keys_on(conn: &Connection, max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    let Ok(mut stmt) = conn.prepare("SELECT host, slug, fetched_at FROM host_cache") else {
        return HashSet::new();
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter(|(_, _, fetched_at)| now.saturating_sub(*fetched_at) <= max_age_secs)
            .map(|(host, slug, _)| (host, slug))
            .collect(),
        Err(_) => HashSet::new(),
    }
}

/// (host domain, slug) pairs whose cached host enrichment is still within
/// `max_age_secs` — the enrich pass skips these so a rescan within the TTL
/// costs no host-API calls. One query, unlike calling [`cached_host_info`]
/// per repo. Keyed per host so the same slug on two hosts is tracked
/// independently.
pub fn fresh_host_keys(max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    match open() {
        Ok(conn) => fresh_host_keys_on(&conn, max_age_secs, now),
        Err(_) => HashSet::new(),
    }
}

fn all_host_info_on(conn: &Connection) -> HashMap<(String, String), HostInfo> {
    let mut map = HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT host, slug, data FROM host_cache") else {
        return map;
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    });
    if let Ok(rows) = rows {
        for (host, slug, json) in rows.flatten() {
            if let Ok(info) = serde_json::from_str::<HostInfo>(&json) {
                map.insert((host, slug), info);
            }
        }
    }
    map
}

/// Overlay persisted host enrichment onto repos (by remote host domain + slug,
/// matching how the enrich pass stores it). Freshly-scanned repos start with
/// empty host fields, so this restores cached visibility/stars/etc. on launch —
/// no network re-fetch required.
fn apply_host_info_on(conn: &Connection, repos: &mut [Repo]) {
    let cache = all_host_info_on(conn);
    if cache.is_empty() {
        return;
    }
    for r in repos.iter_mut() {
        let Some(slug) = r.slug.as_deref() else {
            continue;
        };
        let host = r.remote_host.as_deref().unwrap_or_default();
        if let Some(info) = cache.get(&(host.to_string(), slug.to_string())) {
            r.stars = info.stars;
            r.topics = info.topics.clone();
            r.open_issues = info.open_issues;
            r.latest_release = info.latest_release.clone();
            r.private = info.private;
        }
    }
}

/// Rehydrate `private`/`stars`/etc. on a repo snapshot from the host cache.
pub fn apply_host_info(repos: &mut [Repo]) {
    if let Ok(conn) = open() {
        apply_host_info_on(&conn, repos);
    }
}

fn store_host_info_on(conn: &Connection, host: &str, slug: &str, info: &HostInfo, now: i64) {
    if let Ok(json) = serde_json::to_string(info) {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO host_cache (host, slug, data, fetched_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![host, slug, json, now],
        );
    }
}

/// Persist host enrichment for a (host domain, slug) pair.
pub fn store_host_info(host: &str, slug: &str, info: &HostInfo, now: i64) {
    if let Ok(conn) = open() {
        store_host_info_on(&conn, host, slug, info, now);
    }
}

/// One cached CI result: the shared four-state vocabulary from `inbox`
/// ("success" | "failure" | "pending" | "none"), plus the run's web URL when
/// the API offered one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiEntry {
    pub state: String,
    pub url: Option<String>,
    pub updated_at: i64,
}

pub(crate) fn store_ci_state_on(
    conn: &Connection,
    host: &str,
    slug: &str,
    state: &str,
    url: Option<&str>,
    now: i64,
) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO ci_cache (host, slug, state, url, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![host, slug, state, url, now],
    );
}

pub(crate) fn fresh_ci_keys_on(
    conn: &Connection,
    max_age_secs: i64,
    now: i64,
) -> HashSet<(String, String)> {
    let Ok(mut stmt) = conn.prepare("SELECT host, slug, updated_at FROM ci_cache") else {
        return HashSet::new();
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter(|(_, _, at)| now.saturating_sub(*at) <= max_age_secs)
            .map(|(host, slug, _)| (host, slug))
            .collect(),
        Err(_) => HashSet::new(),
    }
}

/// (host domain, slug) pairs whose cached CI state is still within
/// `max_age_secs` — the CI pass skips these (the `fresh_host_keys` pattern).
pub fn fresh_ci_keys(max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    match open() {
        Ok(conn) => fresh_ci_keys_on(&conn, max_age_secs, now),
        Err(_) => HashSet::new(),
    }
}

pub(crate) fn all_ci_states_on(conn: &Connection) -> HashMap<(String, String), CiEntry> {
    let mut map = HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT host, slug, state, url, updated_at FROM ci_cache")
    else {
        return map;
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
            CiEntry {
                state: row.get(2)?,
                url: row.get(3)?,
                updated_at: row.get(4)?,
            },
        ))
    });
    if let Ok(rows) = rows {
        for (key, entry) in rows.flatten() {
            map.insert(key, entry);
        }
    }
    map
}

/// Every cached CI state, keyed (host domain, slug) — the app's `CiFact`
/// source and the drawer's Overview CI line. One query; stale entries are
/// included (last known state beats no state — the pass refreshes them).
pub fn all_ci_states() -> HashMap<(String, String), CiEntry> {
    match open() {
        Ok(conn) => all_ci_states_on(&conn),
        Err(_) => HashMap::new(),
    }
}

fn apply_summaries_on(conn: &Connection, repos: &mut [Repo]) {
    let Ok(mut stmt) = conn.prepare("SELECT id, summary, last_commit FROM ai_cache") else {
        return;
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    });
    let Ok(rows) = rows else {
        return;
    };
    let map: HashMap<String, (String, i64)> = rows
        .flatten()
        .map(|(id, summary, last)| (id, (summary, last)))
        .collect();
    if map.is_empty() {
        return;
    }
    for r in repos.iter_mut() {
        // Only apply a summary that matches the repo's current commit — a stale
        // one (repo committed since) is dropped so it regenerates.
        if let Some((summary, last)) = map.get(&r.id) {
            if *last == r.last_commit_unix {
                r.ai_summary = Some(summary.clone());
            }
        }
    }
}

/// Overlay cached AI summaries onto a repo snapshot (by id, current commit only).
pub fn apply_summaries(repos: &mut [Repo]) {
    if let Ok(conn) = open() {
        apply_summaries_on(&conn, repos);
    }
}

/// Cached AI summary for a repo, valid only while the last commit is unchanged
/// (so it regenerates after new work lands).
pub fn cached_summary(id: &str, last_commit: i64) -> Option<String> {
    let conn = open().ok()?;
    let mut stmt = conn
        .prepare("SELECT summary, last_commit FROM ai_cache WHERE id = ?1")
        .ok()?;
    let (summary, cached_commit): (String, i64) = stmt
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?;
    (cached_commit == last_commit).then_some(summary)
}

/// Persist an AI summary keyed to the repo's current last commit.
pub fn store_summary(id: &str, summary: &str, last_commit: i64) {
    if let Ok(conn) = open() {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO ai_cache (id, summary, last_commit) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, summary, last_commit],
        );
    }
}

pub fn get_meta(key: &str) -> Option<String> {
    let conn = open().ok()?;
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1").ok()?;
    stmt.query_row([key], |row| row.get::<_, String>(0)).ok()
}

pub fn set_meta(key: &str, value: &str) {
    if let Ok(conn) = open() {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        );
    }
}

// ── Per-repo notes + last-seen cursor (#69) ────────────────────────────────
// The `notes` row holds both a markdown scratchpad and the commit the user had
// seen last; upserts touch only their own column(s) so the two features don't
// clobber each other.

fn note_on(conn: &Connection, id: &str) -> Option<String> {
    let mut stmt = conn.prepare("SELECT text FROM notes WHERE id = ?1").ok()?;
    stmt.query_row([id], |row| row.get::<_, String>(0)).ok()
}

fn set_note_on(conn: &Connection, id: &str, text: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO notes (id, text) VALUES (?1, ?2)
         ON CONFLICT(id) DO UPDATE SET text = excluded.text",
        rusqlite::params![id, text],
    )?;
    Ok(())
}

fn seen_sha_on(conn: &Connection, id: &str) -> Option<String> {
    let mut stmt = conn
        .prepare("SELECT last_seen_sha FROM notes WHERE id = ?1")
        .ok()?;
    let sha: String = stmt.query_row([id], |row| row.get(0)).ok()?;
    (!sha.is_empty()).then_some(sha)
}

fn set_seen_on(conn: &Connection, id: &str, sha: &str, unix: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO notes (id, last_seen_sha, last_seen_unix) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET last_seen_sha = excluded.last_seen_sha, last_seen_unix = excluded.last_seen_unix",
        rusqlite::params![id, sha, unix],
    )?;
    Ok(())
}

/// The markdown note pinned to a repo (empty string if none).
pub fn note(id: &str) -> String {
    open()
        .ok()
        .and_then(|c| note_on(&c, id))
        .unwrap_or_default()
}

/// Persist a repo's markdown note.
pub fn set_note(id: &str, text: &str) -> Result<(), String> {
    let conn = open()?;
    set_note_on(&conn, id, text).map_err(|e| e.to_string())
}

/// The full SHA of the commit the user had seen last for this repo, if any.
pub fn seen_sha(id: &str) -> Option<String> {
    open().ok().and_then(|c| seen_sha_on(&c, id))
}

/// Record the commit the user has now caught up to.
pub fn set_seen(id: &str, sha: &str, unix: i64) -> Result<(), String> {
    let conn = open()?;
    set_seen_on(&conn, id, sha, unix).map_err(|e| e.to_string())
}

// ── Dispatched agent worktrees (#185) ──────────────────────────────────────
// The worktree-path ↔ origin-repo pairing for agents dispatched onto a fresh
// worktree, so the Agents view (and later outcome handling) can find them —
// the worktrees live under the app data dir, outside any scanned root.

/// One recorded agent-worktree dispatch.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentWorktree {
    /// Absolute path of the worktree's working directory (primary key).
    pub worktree_path: String,
    /// The origin repo's id (absolute path).
    pub repo_id: String,
    /// The branch the agent works on (`agent/<slug>-<rand>`).
    pub branch: String,
    /// The git worktree name in the origin repo (needed to prune it).
    pub worktree_name: String,
    /// The task prompt the agent was dispatched with.
    pub prompt: String,
    /// Unix time of the dispatch.
    pub created_at: i64,
}

fn record_agent_worktree_on(conn: &Connection, wt: &AgentWorktree) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO agent_worktrees
         (worktree_path, repo_id, branch, worktree_name, prompt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            wt.worktree_path,
            wt.repo_id,
            wt.branch,
            wt.worktree_name,
            wt.prompt,
            wt.created_at
        ],
    )?;
    Ok(())
}

fn agent_worktrees_on(conn: &Connection) -> Vec<AgentWorktree> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT worktree_path, repo_id, branch, worktree_name, prompt, created_at
         FROM agent_worktrees ORDER BY created_at DESC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok(AgentWorktree {
            worktree_path: r.get(0)?,
            repo_id: r.get(1)?,
            branch: r.get(2)?,
            worktree_name: r.get(3)?,
            prompt: r.get(4)?,
            created_at: r.get(5)?,
        })
    });
    match rows {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

fn remove_agent_worktree_on(conn: &Connection, worktree_path: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM agent_worktrees WHERE worktree_path = ?1",
        [worktree_path],
    )?;
    Ok(())
}

/// Record a dispatched agent worktree (upserts on the worktree path).
pub fn record_agent_worktree(wt: &AgentWorktree) -> Result<(), String> {
    let conn = open()?;
    record_agent_worktree_on(&conn, wt).map_err(|e| e.to_string())
}

/// All recorded agent-worktree dispatches, newest first.
pub fn agent_worktrees() -> Vec<AgentWorktree> {
    match open() {
        Ok(conn) => agent_worktrees_on(&conn),
        Err(_) => Vec::new(),
    }
}

/// Forget a dispatched worktree (after it's removed on disk).
pub fn remove_agent_worktree(worktree_path: &str) -> Result<(), String> {
    let conn = open()?;
    remove_agent_worktree_on(&conn, worktree_path).map_err(|e| e.to_string())
}

fn clear_ai_on(conn: &Connection) -> rusqlite::Result<(usize, usize)> {
    let summaries = conn.execute("DELETE FROM ai_cache", [])?;
    let embeddings = conn.execute("DELETE FROM embeddings", [])?;
    // The per-repo embedding signatures that drive index-skip (see semantic::index).
    conn.execute("DELETE FROM meta WHERE key LIKE 'embed_sig:%'", [])?;
    Ok((summaries, embeddings))
}

/// Clear cached AI summaries and embeddings (and their index-skip signatures).
/// Returns the number of summaries and embeddings removed.
pub fn clear_ai() -> Result<(usize, usize), String> {
    let conn = open()?;
    clear_ai_on(&conn).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Activity, GitStatus};

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    fn sample(id: &str) -> Repo {
        Repo {
            id: id.to_string(),
            display_name: "Test".into(),
            slug: Some("o/test".into()),
            path: "~/dev/test".into(),
            description: None,
            language: Some("Rust".into()),
            git: GitStatus::default(),
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: None,
            remote_host: None,
            stars: 0,
            topics: Vec::new(),
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
        }
    }

    #[test]
    fn notes_and_seen_share_a_row_without_clobbering() {
        let conn = mem();
        assert_eq!(note_on(&conn, "/a"), None);
        assert_eq!(seen_sha_on(&conn, "/a"), None);

        // Setting a note must not wipe the (absent) seen cursor, and vice versa.
        set_note_on(&conn, "/a", "todo: ship it").unwrap();
        set_seen_on(&conn, "/a", "deadbeef", 1_000).unwrap();
        assert_eq!(note_on(&conn, "/a").as_deref(), Some("todo: ship it"));
        assert_eq!(seen_sha_on(&conn, "/a").as_deref(), Some("deadbeef"));

        // Updating one column leaves the other intact.
        set_note_on(&conn, "/a", "done").unwrap();
        assert_eq!(seen_sha_on(&conn, "/a").as_deref(), Some("deadbeef"));
        set_seen_on(&conn, "/a", "cafef00d", 2_000).unwrap();
        assert_eq!(note_on(&conn, "/a").as_deref(), Some("done"));
        assert_eq!(seen_sha_on(&conn, "/a").as_deref(), Some("cafef00d"));

        // An empty seen sha reads back as None (no cursor yet).
        set_seen_on(&conn, "/b", "", 0).unwrap();
        assert_eq!(seen_sha_on(&conn, "/b"), None);
    }

    #[test]
    fn agent_worktrees_record_list_remove() {
        let conn = mem();
        assert!(agent_worktrees_on(&conn).is_empty());

        let a = AgentWorktree {
            worktree_path: "/data/worktrees/repo-agent-x-1111".into(),
            repo_id: "/dev/repo".into(),
            branch: "agent/x-1111".into(),
            worktree_name: "agent-x-1111".into(),
            prompt: "fix x".into(),
            created_at: 100,
        };
        let b = AgentWorktree {
            worktree_path: "/data/worktrees/repo-agent-y-2222".into(),
            repo_id: "/dev/repo".into(),
            branch: "agent/y-2222".into(),
            worktree_name: "agent-y-2222".into(),
            prompt: "do y".into(),
            created_at: 200,
        };
        record_agent_worktree_on(&conn, &a).unwrap();
        record_agent_worktree_on(&conn, &b).unwrap();

        // Newest first, full pairing round-trips.
        let listed = agent_worktrees_on(&conn);
        assert_eq!(listed, vec![b.clone(), a.clone()]);

        // Re-recording the same path replaces rather than duplicates.
        let a2 = AgentWorktree {
            prompt: "fix x, again".into(),
            ..a.clone()
        };
        record_agent_worktree_on(&conn, &a2).unwrap();
        let listed = agent_worktrees_on(&conn);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[1].prompt, "fix x, again");

        remove_agent_worktree_on(&conn, &a.worktree_path).unwrap();
        assert_eq!(agent_worktrees_on(&conn), vec![b]);
        // Removing an unknown path is a no-op, not an error.
        remove_agent_worktree_on(&conn, "/nope").unwrap();
    }

    #[test]
    fn favorites_roundtrip() {
        let conn = mem();
        assert!(favorites_on(&conn).is_empty());
        set_favorite_on(&conn, "/a", true).unwrap();
        set_favorite_on(&conn, "/b", true).unwrap();
        let favs = favorites_on(&conn);
        assert!(favs.contains("/a") && favs.contains("/b"));
        set_favorite_on(&conn, "/a", false).unwrap();
        assert!(!favorites_on(&conn).contains("/a"));
    }

    #[test]
    fn schema_bump_clears_versioned_caches() {
        let conn = mem(); // init() sets cache_schema to the current version
        store_host_info_on(&conn, "github.com", "o/test", &HostInfo::default(), 1_000);
        conn.execute(
            "INSERT INTO embeddings (host, slug, source, chunk_ix, content, vector, updated_at)
             VALUES ('github.com', 'o/test', 'readme', 0, 't', X'0000803f', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embed_sig:/a', 'x')",
            [],
        )
        .unwrap();
        store_ci_state_on(&conn, "github.com", "o/test", "failure", None, 1_000);
        // Simulate an older schema, then migrate.
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_schema', '1')",
            [],
        )
        .unwrap();
        migrate(&conn).unwrap();
        for table in ["host_cache", "embeddings", "ci_cache"] {
            let rows: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "stale {table} should be cleared on schema bump");
        }
        // Embedding signatures go with the vectors, or nothing would re-embed.
        let sigs: i64 = conn
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'embed_sig:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sigs, 0, "embed signatures must be cleared with the vectors");
        // Re-running is a no-op now that the version matches.
        store_host_info_on(&conn, "github.com", "o/test", &HostInfo::default(), 1_000);
        migrate(&conn).unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM host_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "matching schema must not clear the cache");
    }

    #[test]
    fn migrate_recreates_embeddings_from_pre_v4_shape() {
        // A v3 database has embeddings keyed by repo id with a JSON vector;
        // migrate must drop that shape and recreate the chunked v4 table.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE embeddings (id TEXT PRIMARY KEY, vec TEXT NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('cache_schema', '3');
             INSERT INTO embeddings (id, vec) VALUES ('/a', '[0.1]');",
        )
        .unwrap();
        init(&conn).unwrap();
        // The new shape accepts (host, slug, source, chunk_ix) rows and starts empty.
        conn.execute(
            "INSERT INTO embeddings (host, slug, source, chunk_ix, content, vector, updated_at)
             VALUES ('github.com', 'o/test', 'readme', 0, 't', X'0000803f', 1)",
            [],
        )
        .unwrap();
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM embeddings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn migrate_recreates_host_cache_from_pre_v3_shape() {
        // A v2 database has host_cache keyed by slug alone; migrate must be able
        // to drop that shape and recreate the (host, slug)-keyed table.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE host_cache (slug TEXT PRIMARY KEY, data TEXT NOT NULL, fetched_at INTEGER NOT NULL);
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO meta (key, value) VALUES ('cache_schema', '2');
             INSERT INTO host_cache (slug, data, fetched_at) VALUES ('o/test', '{}', 1);",
        )
        .unwrap();
        init(&conn).unwrap();
        // The new shape accepts (host, slug) rows and starts empty.
        store_host_info_on(&conn, "github.com", "o/test", &HostInfo::default(), 1_000);
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM host_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn ci_state_roundtrips_and_overwrites() {
        let conn = mem();
        assert!(all_ci_states_on(&conn).is_empty());
        store_ci_state_on(
            &conn,
            "github.com",
            "o/test",
            "failure",
            Some("https://github.com/o/test/actions/runs/1"),
            1_000,
        );
        store_ci_state_on(&conn, "github.com", "o/other", "success", None, 1_000);
        let states = all_ci_states_on(&conn);
        assert_eq!(states.len(), 2);
        let entry = &states[&("github.com".to_string(), "o/test".to_string())];
        assert_eq!(entry.state, "failure");
        assert_eq!(
            entry.url.as_deref(),
            Some("https://github.com/o/test/actions/runs/1")
        );
        assert_eq!(entry.updated_at, 1_000);

        // A later write replaces (one row per key), including clearing the URL.
        store_ci_state_on(&conn, "github.com", "o/test", "success", None, 2_000);
        let states = all_ci_states_on(&conn);
        assert_eq!(states.len(), 2);
        let entry = &states[&("github.com".to_string(), "o/test".to_string())];
        assert_eq!(entry.state, "success");
        assert_eq!(entry.url, None);
        assert_eq!(entry.updated_at, 2_000);
    }

    #[test]
    fn fresh_ci_keys_filters_by_ttl() {
        let conn = mem();
        store_ci_state_on(&conn, "github.com", "o/fresh", "success", None, 1_000);
        store_ci_state_on(&conn, "github.com", "o/stale", "success", None, 100);
        let fresh = fresh_ci_keys_on(&conn, 300, 1_200);
        assert!(fresh.contains(&("github.com".to_string(), "o/fresh".to_string())));
        assert!(!fresh.contains(&("github.com".to_string(), "o/stale".to_string())));
    }

    #[test]
    fn apply_host_info_rehydrates_repo_from_cache() {
        let conn = mem();
        let info = HostInfo {
            stars: 42,
            topics: vec!["cli".into()],
            open_issues: 3,
            latest_release: Some("v1.2.3".into()),
            private: true,
        };
        store_host_info_on(&conn, "github.com", "o/test", &info, 1_000);

        // slug "o/test" on github.com, host fields empty
        let mut repos = vec![Repo {
            remote_host: Some("github.com".into()),
            ..sample("/a")
        }];
        apply_host_info_on(&conn, &mut repos);
        assert!(repos[0].private);
        assert_eq!(repos[0].stars, 42);
        assert_eq!(repos[0].latest_release.as_deref(), Some("v1.2.3"));

        // A repo with no cached slug is left untouched.
        let mut other = vec![Repo {
            slug: Some("o/none".into()),
            remote_host: Some("github.com".into()),
            ..sample("/b")
        }];
        apply_host_info_on(&conn, &mut other);
        assert!(!other[0].private);
        assert_eq!(other[0].stars, 0);
    }

    #[test]
    fn host_cache_keys_by_host_and_slug() {
        // Regression for #159: the same "owner/repo" slug on two hosts (public
        // GitHub + private self-hosted GitLab) must not share one cached row.
        let conn = mem();
        let public = HostInfo {
            stars: 42,
            private: false,
            ..HostInfo::default()
        };
        let private = HostInfo {
            stars: 0,
            private: true,
            ..HostInfo::default()
        };
        store_host_info_on(&conn, "github.com", "o/test", &public, 1_000);
        store_host_info_on(&conn, "gitlab.acme.io", "o/test", &private, 1_000);

        let mut repos = vec![
            Repo {
                remote_host: Some("github.com".into()),
                ..sample("/a")
            },
            Repo {
                remote_host: Some("gitlab.acme.io".into()),
                ..sample("/b")
            },
        ];
        apply_host_info_on(&conn, &mut repos);
        assert!(!repos[0].private, "github repo must stay public");
        assert_eq!(repos[0].stars, 42);
        assert!(repos[1].private, "self-hosted repo must stay private");
        assert_eq!(repos[1].stars, 0);
    }

    #[test]
    fn apply_summaries_only_for_current_commit() {
        let conn = mem();
        conn.execute(
            "INSERT INTO ai_cache (id, summary, last_commit) VALUES ('/a', 'fresh summary', 5)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_cache (id, summary, last_commit) VALUES ('/b', 'stale summary', 1)",
            [],
        )
        .unwrap();

        let mut repos = vec![
            Repo {
                last_commit_unix: 5,
                ..sample("/a")
            },
            Repo {
                last_commit_unix: 9, // repo committed since the summary → stale
                ..sample("/b")
            },
        ];
        apply_summaries_on(&conn, &mut repos);
        assert_eq!(repos[0].ai_summary.as_deref(), Some("fresh summary"));
        assert_eq!(repos[1].ai_summary, None, "stale summary must not apply");
    }

    #[test]
    fn fresh_host_keys_filters_by_ttl() {
        let conn = mem();
        store_host_info_on(&conn, "github.com", "o/fresh", &HostInfo::default(), 1_000);
        store_host_info_on(&conn, "github.com", "o/stale", &HostInfo::default(), 100);
        // Same slug as "o/fresh" but on another host — freshness is per host.
        store_host_info_on(
            &conn,
            "gitlab.acme.io",
            "o/fresh",
            &HostInfo::default(),
            100,
        );
        // now=1_500, ttl=600 → "fresh" (age 500) kept, "stale" (age 1_400) dropped.
        let fresh = fresh_host_keys_on(&conn, 600, 1_500);
        assert!(fresh.contains(&("github.com".into(), "o/fresh".into())));
        assert!(!fresh.contains(&("github.com".into(), "o/stale".into())));
        assert!(
            !fresh.contains(&("gitlab.acme.io".into(), "o/fresh".into())),
            "a fresh slug on one host must not mark the other host fresh"
        );
    }

    #[test]
    fn clear_ai_removes_summaries_embeddings_and_sigs() {
        let conn = mem();
        conn.execute(
            "INSERT INTO ai_cache (id, summary, last_commit) VALUES ('/a', 's', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO embeddings (host, slug, source, chunk_ix, content, vector, updated_at)
             VALUES ('github.com', 'o/test', 'readme', 0, 'text', X'0000803f', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embed_sig:/a', 'x')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO meta (key, value) VALUES ('keep', 'me')", [])
            .unwrap();

        let (summaries, embeddings) = clear_ai_on(&conn).unwrap();
        assert_eq!((summaries, embeddings), (1, 1));
        assert_eq!(
            conn.query_row("SELECT count(*) FROM ai_cache", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM embeddings", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        // unrelated meta is preserved; embed_sig is removed
        let keep: i64 = conn
            .query_row("SELECT count(*) FROM meta WHERE key = 'keep'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let sig: i64 = conn
            .query_row(
                "SELECT count(*) FROM meta WHERE key = 'embed_sig:/a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((keep, sig), (1, 0));
    }

    #[test]
    fn favorite_insert_is_idempotent() {
        let conn = mem();
        set_favorite_on(&conn, "/a", true).unwrap();
        set_favorite_on(&conn, "/a", true).unwrap();
        assert_eq!(favorites_on(&conn).len(), 1);
    }

    #[test]
    fn store_then_load_repos_roundtrips_and_replaces() {
        let mut conn = mem();
        store_repos_on(&mut conn, &[sample("/a"), sample("/b")]).unwrap();
        assert_eq!(load_repos_on(&conn).len(), 2);
        // store replaces the snapshot rather than appending
        store_repos_on(&mut conn, &[sample("/c")]).unwrap();
        let loaded = load_repos_on(&conn);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "/c");
    }

    #[test]
    fn load_marks_favorites() {
        let mut conn = mem();
        store_repos_on(&mut conn, &[sample("/a"), sample("/b")]).unwrap();
        set_favorite_on(&conn, "/b", true).unwrap();
        let loaded = load_repos_on(&conn);
        let fav_b = loaded.iter().find(|r| r.id == "/b").unwrap();
        let fav_a = loaded.iter().find(|r| r.id == "/a").unwrap();
        assert!(fav_b.favorite);
        assert!(!fav_a.favorite);
    }
}
