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
// making private repos look public until the 6h TTL lapsed.
const CACHE_SCHEMA: i64 = 2;

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS favorites (id TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS repos (id TEXT PRIMARY KEY, data TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS host_cache (slug TEXT PRIMARY KEY, data TEXT NOT NULL, fetched_at INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS ai_cache (id TEXT PRIMARY KEY, summary TEXT NOT NULL, last_commit INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS embeddings (id TEXT PRIMARY KEY, vec TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    migrate(conn)
}

/// Drop schema-sensitive cached payloads when CACHE_SCHEMA changes. Only
/// host enrichment is version-sensitive today; favorites/AI cache are untouched.
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = 'cache_schema'", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|s| s.parse().ok());
    if current != Some(CACHE_SCHEMA) {
        conn.execute("DELETE FROM host_cache", [])?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_schema', ?1)",
            [CACHE_SCHEMA.to_string()],
        )?;
    }
    Ok(())
}

fn open() -> Result<Connection, String> {
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
            let json = serde_json::to_string(repo).unwrap_or_default();
            stmt.execute(rusqlite::params![repo.id, json])?;
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

/// Cached host enrichment for a slug, if newer than `max_age_secs`.
pub fn cached_host_info(slug: &str, max_age_secs: i64, now: i64) -> Option<HostInfo> {
    let conn = open().ok()?;
    let mut stmt = conn
        .prepare("SELECT data, fetched_at FROM host_cache WHERE slug = ?1")
        .ok()?;
    let (json, fetched_at): (String, i64) = stmt
        .query_row([slug], |row| Ok((row.get(0)?, row.get(1)?)))
        .ok()?;
    if now.saturating_sub(fetched_at) > max_age_secs {
        return None;
    }
    serde_json::from_str(&json).ok()
}

fn all_host_info_on(conn: &Connection) -> HashMap<String, HostInfo> {
    let mut map = HashMap::new();
    let Ok(mut stmt) = conn.prepare("SELECT slug, data FROM host_cache") else {
        return map;
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    });
    if let Ok(rows) = rows {
        for (slug, json) in rows.flatten() {
            if let Ok(info) = serde_json::from_str::<HostInfo>(&json) {
                map.insert(slug, info);
            }
        }
    }
    map
}

/// Overlay persisted host enrichment onto repos (by slug). Freshly-scanned
/// repos start with empty host fields, so this restores cached
/// visibility/stars/etc. on launch — no network re-fetch required.
fn apply_host_info_on(conn: &Connection, repos: &mut [Repo]) {
    let cache = all_host_info_on(conn);
    if cache.is_empty() {
        return;
    }
    for r in repos.iter_mut() {
        let Some(slug) = r.slug.as_deref() else { continue };
        if let Some(info) = cache.get(slug) {
            r.stars = info.stars;
            r.topics = info.topics.clone();
            r.open_issues = info.open_issues;
            r.latest_release = info.latest_release.clone();
            r.private = info.private;
        }
    }
}

/// All persisted host enrichment, keyed by slug (any age).
pub fn all_host_info() -> HashMap<String, HostInfo> {
    open().map(|c| all_host_info_on(&c)).unwrap_or_default()
}

/// Rehydrate `private`/`stars`/etc. on a repo snapshot from the host cache.
pub fn apply_host_info(repos: &mut [Repo]) {
    if let Ok(conn) = open() {
        apply_host_info_on(&conn, repos);
    }
}

fn store_host_info_on(conn: &Connection, slug: &str, info: &HostInfo, now: i64) {
    if let Ok(json) = serde_json::to_string(info) {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO host_cache (slug, data, fetched_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![slug, json, now],
        );
    }
}

/// Persist host enrichment for a slug.
pub fn store_host_info(slug: &str, info: &HostInfo, now: i64) {
    if let Ok(conn) = open() {
        store_host_info_on(&conn, slug, info, now);
    }
}

/// Cached AI summary for a repo, valid only while the last commit is unchanged
/// (so it regenerates after new work lands).
pub fn cached_summary(id: &str, last_commit: i64) -> Option<String> {
    let conn = open().ok()?;
    let mut stmt = conn
        .prepare("SELECT summary, last_commit FROM ai_cache WHERE id = ?1")
        .ok()?;
    let (summary, cached_commit): (String, i64) =
        stmt.query_row([id], |row| Ok((row.get(0)?, row.get(1)?))).ok()?;
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

/// Store a repo's embedding vector (as JSON) for semantic search.
pub fn store_embedding(id: &str, vec: &[f32]) {
    if let Ok(conn) = open() {
        if let Ok(json) = serde_json::to_string(vec) {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO embeddings (id, vec) VALUES (?1, ?2)",
                rusqlite::params![id, json],
            );
        }
    }
}

/// Load all repo embeddings as (id, vector).
pub fn load_embeddings() -> Vec<(String, Vec<f32>)> {
    let Ok(conn) = open() else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT id, vec FROM embeddings") else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)));
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter_map(|(id, json)| serde_json::from_str::<Vec<f32>>(&json).ok().map(|v| (id, v)))
            .collect(),
        Err(_) => Vec::new(),
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

fn clear_ai_on(conn: &Connection) -> rusqlite::Result<(usize, usize)> {
    let summaries = conn.execute("DELETE FROM ai_cache", [])?;
    let embeddings = conn.execute("DELETE FROM embeddings", [])?;
    // The per-repo embedding signatures that drive index-skip (see index_repos).
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
    fn schema_bump_clears_host_cache() {
        let conn = mem(); // init() sets cache_schema to the current version
        store_host_info_on(&conn, "o/test", &HostInfo::default(), 1_000);
        // Simulate an older schema, then migrate.
        conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_schema', '1')", []).unwrap();
        migrate(&conn).unwrap();
        let rows: i64 = conn.query_row("SELECT count(*) FROM host_cache", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0, "stale host_cache should be cleared on schema bump");
        // Re-running is a no-op now that the version matches.
        store_host_info_on(&conn, "o/test", &HostInfo::default(), 1_000);
        migrate(&conn).unwrap();
        let rows: i64 = conn.query_row("SELECT count(*) FROM host_cache", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 1, "matching schema must not clear the cache");
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
        store_host_info_on(&conn, "o/test", &info, 1_000);

        let mut repos = vec![sample("/a")]; // slug "o/test", host fields empty
        apply_host_info_on(&conn, &mut repos);
        assert!(repos[0].private);
        assert_eq!(repos[0].stars, 42);
        assert_eq!(repos[0].latest_release.as_deref(), Some("v1.2.3"));

        // A repo with no cached slug is left untouched.
        let mut other = vec![Repo { slug: Some("o/none".into()), ..sample("/b") }];
        apply_host_info_on(&conn, &mut other);
        assert!(!other[0].private);
        assert_eq!(other[0].stars, 0);
    }

    #[test]
    fn clear_ai_removes_summaries_embeddings_and_sigs() {
        let conn = mem();
        conn.execute("INSERT INTO ai_cache (id, summary, last_commit) VALUES ('/a', 's', 1)", []).unwrap();
        conn.execute("INSERT INTO embeddings (id, vec) VALUES ('/a', '[0.1]')", []).unwrap();
        conn.execute("INSERT INTO meta (key, value) VALUES ('embed_sig:/a', 'x')", []).unwrap();
        conn.execute("INSERT INTO meta (key, value) VALUES ('keep', 'me')", []).unwrap();

        let (summaries, embeddings) = clear_ai_on(&conn).unwrap();
        assert_eq!((summaries, embeddings), (1, 1));
        assert_eq!(conn.query_row("SELECT count(*) FROM ai_cache", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert_eq!(conn.query_row("SELECT count(*) FROM embeddings", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        // unrelated meta is preserved; embed_sig is removed
        let keep: i64 = conn.query_row("SELECT count(*) FROM meta WHERE key = 'keep'", [], |r| r.get(0)).unwrap();
        let sig: i64 = conn.query_row("SELECT count(*) FROM meta WHERE key = 'embed_sig:/a'", [], |r| r.get(0)).unwrap();
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
