//! SQLite-backed cache (`~/.local/share/orrery/cache.sqlite`): persists user
//! favorites and a snapshot of scanned repos so the grid paints instantly on
//! launch and survives offline. Connections are opened per call (cheap: the
//! schema init/migrate runs once per process, see [`open`]); the database is
//! in WAL mode with a busy timeout so concurrent background writers wait
//! rather than fail with "database is locked".
//!
//! The `*_on(conn)` helpers take a connection so the logic is unit-testable
//! against an in-memory database (see the tests module).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use rusqlite::{Connection, Row, Transaction, TransactionBehavior};

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

/// `disk_usage` DDL: the janitor's per-repo artefact measurement
/// (`janitor::DiskUsage` as JSON), TTL-paced by `measured_at`.
const DISK_USAGE_DDL: &str = "CREATE TABLE IF NOT EXISTS disk_usage (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    measured_at INTEGER NOT NULL);";

/// `advisory_cache` DDL: security-advisory counts per source
/// (`advisories::AdvisoryReport` as JSON), keyed (host, key) like `ci_cache`.
/// Dependabot rows use (domain, slug); local audits use ("local", repo id).
const ADVISORY_CACHE_DDL: &str = "CREATE TABLE IF NOT EXISTS advisory_cache (
    host TEXT NOT NULL,
    slug TEXT NOT NULL,
    data TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (host, slug));";

/// Create/upgrade the schema. Runs in one `BEGIN IMMEDIATE` transaction so two
/// first-launch connections (two processes, or two threads before the
/// per-process [`open`] guard) can't interleave the `PRAGMA table_info` probe
/// and the `ALTER TABLE` and fail with "duplicate column": the second waits
/// for the write lock, then sees the column already there.
pub(crate) fn init(conn: &Connection) -> rusqlite::Result<()> {
    // `new_unchecked` because we only hold `&Connection`; nothing else is in a
    // transaction on this connection at this point. Drop rolls back on error.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    init_schema(&tx)?;
    tx.commit()
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
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
             created_at INTEGER NOT NULL,
             last_seen_alive INTEGER NOT NULL DEFAULT 0,
             finished_at INTEGER NOT NULL DEFAULT 0,
             commits_ahead INTEGER NOT NULL DEFAULT 0,
             pr_url TEXT NOT NULL DEFAULT '');",
    )?;
    // Outcome columns added for #185 session-finish detection. Unlike
    // host_cache/embeddings (disposable caches, dropped on a CACHE_SCHEMA
    // bump), agent_worktrees holds durable worktree↔repo pairings that must
    // survive schema evolution — so it grows via additive, idempotent ALTERs
    // instead. Detection state is persisted (not kept on the app) so a session
    // that finishes while Orrery is closed is still detected on the next
    // launch: `last_seen_alive > 0` + no live process = finished.
    for (col, ddl) in [
        ("last_seen_alive", "INTEGER NOT NULL DEFAULT 0"),
        ("finished_at", "INTEGER NOT NULL DEFAULT 0"),
        ("commits_ahead", "INTEGER NOT NULL DEFAULT 0"),
        ("pr_url", "TEXT NOT NULL DEFAULT ''"),
    ] {
        add_column_if_missing(conn, "agent_worktrees", col, ddl)?;
    }
    conn.execute_batch(HOST_CACHE_DDL)?;
    conn.execute_batch(EMBEDDINGS_DDL)?;
    conn.execute_batch(CI_CACHE_DDL)?;
    conn.execute_batch(DISK_USAGE_DDL)?;
    conn.execute_batch(ADVISORY_CACHE_DDL)?;
    // Durable (not a cache): when each repo was first seen dirty.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS dirty_since (id TEXT PRIMARY KEY, since INTEGER NOT NULL);",
    )?;
    migrate(conn)
}

/// Idempotent `ALTER TABLE … ADD COLUMN` — the migration path for tables whose
/// rows are durable state (can't be dropped and recreated on a schema bump).
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    ddl: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let existing: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .flatten()
        .collect();
    if !existing.iter().any(|c| c == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
            [],
        )?;
    }
    Ok(())
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
        conn.execute("DROP TABLE IF EXISTS disk_usage", [])?;
        conn.execute("DROP TABLE IF EXISTS advisory_cache", [])?;
        conn.execute_batch(HOST_CACHE_DDL)?;
        conn.execute_batch(EMBEDDINGS_DDL)?;
        conn.execute_batch(CI_CACHE_DDL)?;
        conn.execute_batch(DISK_USAGE_DDL)?;
        conn.execute_batch(ADVISORY_CACHE_DDL)?;
        // The dropped vectors must re-embed; stale signatures would skip them.
        conn.execute("DELETE FROM meta WHERE key LIKE 'embed_sig:%'", [])?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('cache_schema', ?1)",
            [CACHE_SCHEMA.to_string()],
        )?;
    }
    Ok(())
}

/// Database paths whose schema this process has already initialized, so
/// [`open`] runs [`init`] once per process rather than on every call.
static INITIALIZED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// How long a connection waits on another writer's lock before erroring.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Open the on-disk cache, initializing/migrating the schema on the first open
/// of `path` in this process. `pub(crate)` so `crate::semantic`'s public
/// wrappers can follow the same open-per-call pattern for the `embeddings`
/// table without cache.rs owning its row logic.
pub(crate) fn open() -> Result<Connection, String> {
    let path = db_path().ok_or("no data directory")?;
    open_at(&path).map_err(|e| e.to_string())
}

fn open_at(path: &std::path::Path) -> Result<Connection, Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // Held across init so concurrent first opens in this process serialize;
    // a failed init isn't recorded, so the next open retries it.
    let mut done = INITIALIZED.lock().unwrap_or_else(|e| e.into_inner());
    if !done.contains(path) {
        // WAL is persistent in the file and can't be switched inside a
        // transaction, so set it here, before init's transaction.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        init(&conn)?;
        done.insert(path.to_path_buf());
    }
    Ok(conn)
}

/// Run `f` on a fresh cache connection, or return `default` if the cache can't
/// be opened — the shape of every infallible public cache wrapper.
pub(crate) fn with_conn<T>(default: T, f: impl FnOnce(&Connection) -> T) -> T {
    match open() {
        Ok(conn) => f(&conn),
        Err(_) => default,
    }
}

/// Run a fallible `f` on a fresh cache connection, stringifying any error —
/// the shape of every `Result`-returning public cache wrapper.
pub(crate) fn try_with_conn<T>(
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let conn = open()?;
    f(&conn).map_err(|e| e.to_string())
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
    with_conn(HashSet::new(), favorites_on)
}

/// Toggle a repo's favorite flag, returning the new state.
pub fn set_favorite(id: &str, favorite: bool) -> Result<bool, String> {
    try_with_conn(|c| set_favorite_on(c, id, favorite))?;
    Ok(favorite)
}

/// Replace the cached repo snapshot, and update the first-seen-dirty
/// timestamps the at-risk report reads (see [`dirty_since_all`]).
pub fn store_repos(repos: &[Repo]) -> Result<(), String> {
    let mut conn = open()?;
    store_repos_on(&mut conn, repos).map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    record_dirty_on(&mut conn, repos, now).map_err(|e| e.to_string())
}

/// Load the cached repo snapshot (for instant paint before a fresh scan).
pub fn load_repos() -> Vec<Repo> {
    with_conn(Vec::new(), load_repos_on)
}

/// Run `sql` — whose first two columns are (host, slug) — and pair each row's
/// key with the value `value` reads from the remaining columns. Unreadable
/// rows are skipped: it's a cache. Shared by the host- and CI-cache readers,
/// which are both keyed (host domain, slug) per the #159 lesson.
fn keyed_rows_on<T>(
    conn: &Connection,
    sql: &str,
    value: impl Fn(&Row) -> rusqlite::Result<T>,
) -> Vec<((String, String), T)> {
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
            value(row)?,
        ))
    });
    match rows {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Keys from `sql` (selecting host, slug, timestamp) no older than `max_age_secs`.
fn fresh_keys_on(
    conn: &Connection,
    sql: &str,
    max_age_secs: i64,
    now: i64,
) -> HashSet<(String, String)> {
    keyed_rows_on(conn, sql, |row| row.get::<_, i64>(2))
        .into_iter()
        .filter(|(_, at)| now.saturating_sub(*at) <= max_age_secs)
        .map(|(key, _)| key)
        .collect()
}

fn fresh_host_keys_on(conn: &Connection, max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    fresh_keys_on(
        conn,
        "SELECT host, slug, fetched_at FROM host_cache",
        max_age_secs,
        now,
    )
}

/// (host domain, slug) pairs whose cached host enrichment is still within
/// `max_age_secs` — the enrich pass skips these so a rescan within the TTL
/// costs no host-API calls. One query for the whole fleet. Keyed per host so
/// the same slug on two hosts is tracked independently.
pub fn fresh_host_keys(max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    with_conn(HashSet::new(), |c| fresh_host_keys_on(c, max_age_secs, now))
}

fn all_host_info_on(conn: &Connection) -> HashMap<(String, String), HostInfo> {
    keyed_rows_on(conn, "SELECT host, slug, data FROM host_cache", |row| {
        row.get::<_, String>(2)
    })
    .into_iter()
    .filter_map(|(key, json)| Some((key, serde_json::from_str::<HostInfo>(&json).ok()?)))
    .collect()
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
    with_conn((), |c| apply_host_info_on(c, repos));
}

fn store_host_info_on(conn: &Connection, host: &str, slug: &str, info: &HostInfo, now: i64) {
    if let Ok(json) = serde_json::to_string(info) {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO host_cache (host, slug, data, fetched_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![host, slug, json, now],
        );
    }
}

/// Persist host enrichment for one (host domain, slug) pair. For a batch (the
/// enrich pass) prefer [`store_host_infos`]: one connection, one transaction.
pub fn store_host_info(host: &str, slug: &str, info: &HostInfo, now: i64) {
    with_conn((), |c| store_host_info_on(c, host, slug, info, now));
}

/// Persist a batch of (host domain, slug, info) enrichment results in a single
/// transaction, all stamped `now`. Rows that fail to serialize are skipped.
fn store_host_infos_on(
    conn: &Connection,
    entries: &[(String, String, HostInfo)],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR REPLACE INTO host_cache (host, slug, data, fetched_at) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (host, slug, info) in entries {
            if let Ok(json) = serde_json::to_string(info) {
                stmt.execute(rusqlite::params![host, slug, json, now])?;
            }
        }
    }
    tx.commit()
}

/// Persist a batch of host enrichment results — one connection and one
/// transaction instead of one per repo.
pub fn store_host_infos(entries: &[(String, String, HostInfo)], now: i64) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    try_with_conn(|c| store_host_infos_on(c, entries, now))
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
    fresh_keys_on(
        conn,
        "SELECT host, slug, updated_at FROM ci_cache",
        max_age_secs,
        now,
    )
}

/// (host domain, slug) pairs whose cached CI state is still within
/// `max_age_secs` — the CI pass skips these (the `fresh_host_keys` pattern).
pub fn fresh_ci_keys(max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    with_conn(HashSet::new(), |c| fresh_ci_keys_on(c, max_age_secs, now))
}

pub(crate) fn all_ci_states_on(conn: &Connection) -> HashMap<(String, String), CiEntry> {
    keyed_rows_on(
        conn,
        "SELECT host, slug, state, url, updated_at FROM ci_cache",
        |row| {
            Ok(CiEntry {
                state: row.get(2)?,
                url: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .into_iter()
    .collect()
}

/// Every cached CI state, keyed (host domain, slug) — the app's `CiFact`
/// source and the drawer's Overview CI line. One query; stale entries are
/// included (last known state beats no state — the pass refreshes them).
pub fn all_ci_states() -> HashMap<(String, String), CiEntry> {
    with_conn(HashMap::new(), all_ci_states_on)
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
    with_conn((), |c| apply_summaries_on(c, repos));
}

/// Cached AI summary for a repo, valid only while the last commit is unchanged
/// (so it regenerates after new work lands).
pub fn cached_summary(id: &str, last_commit: i64) -> Option<String> {
    with_conn(None, |conn| {
        let mut stmt = conn
            .prepare("SELECT summary, last_commit FROM ai_cache WHERE id = ?1")
            .ok()?;
        let (summary, cached_commit): (String, i64) = stmt
            .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
            .ok()?;
        (cached_commit == last_commit).then_some(summary)
    })
}

/// Persist an AI summary keyed to the repo's current last commit.
pub fn store_summary(id: &str, summary: &str, last_commit: i64) {
    with_conn((), |conn| {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO ai_cache (id, summary, last_commit) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, summary, last_commit],
        );
    });
}

pub fn get_meta(key: &str) -> Option<String> {
    with_conn(None, |conn| {
        let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?1").ok()?;
        stmt.query_row([key], |row| row.get::<_, String>(0)).ok()
    })
}

pub fn set_meta(key: &str, value: &str) {
    with_conn((), |conn| {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        );
    });
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
    with_conn(None, |c| note_on(c, id)).unwrap_or_default()
}

/// Persist a repo's markdown note.
pub fn set_note(id: &str, text: &str) -> Result<(), String> {
    try_with_conn(|c| set_note_on(c, id, text))
}

/// The full SHA of the commit the user had seen last for this repo, if any.
pub fn seen_sha(id: &str) -> Option<String> {
    with_conn(None, |c| seen_sha_on(c, id))
}

/// Record the commit the user has now caught up to.
pub fn set_seen(id: &str, sha: &str, unix: i64) -> Result<(), String> {
    try_with_conn(|c| set_seen_on(c, id, sha, unix))
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
    /// Unix time an agent process was last observed alive inside the worktree
    /// (0 = never observed). Persisted so a session that ends while Orrery is
    /// closed is still recognized as finished on the next launch.
    pub last_seen_alive: i64,
    /// Unix time the session was observed to have finished (0 = not finished).
    /// Cleared when a session resumes in the worktree.
    pub finished_at: i64,
    /// Commits ahead of the origin repo's default branch, measured when the
    /// session finished — the "has work to review" signal.
    pub commits_ahead: u32,
    /// URL of the PR opened from this worktree's branch ("" = none). Once set,
    /// the finished state stops raising attention — the work has been handed
    /// off to review.
    pub pr_url: String,
}

fn record_agent_worktree_on(conn: &Connection, wt: &AgentWorktree) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO agent_worktrees
         (worktree_path, repo_id, branch, worktree_name, prompt, created_at,
          last_seen_alive, finished_at, commits_ahead, pr_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            wt.worktree_path,
            wt.repo_id,
            wt.branch,
            wt.worktree_name,
            wt.prompt,
            wt.created_at,
            wt.last_seen_alive,
            wt.finished_at,
            wt.commits_ahead,
            wt.pr_url
        ],
    )?;
    Ok(())
}

fn agent_worktrees_on(conn: &Connection) -> Vec<AgentWorktree> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT worktree_path, repo_id, branch, worktree_name, prompt, created_at,
                last_seen_alive, finished_at, commits_ahead, pr_url
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
            last_seen_alive: r.get(6)?,
            finished_at: r.get(7)?,
            commits_ahead: r.get(8)?,
            pr_url: r.get(9)?,
        })
    });
    match rows {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

/// Record a live-session sighting: bump `last_seen_alive` and clear any
/// finished state (a session running again means the outcome is back in
/// flight — e.g. the user hit "Resume").
fn mark_agent_worktree_alive_on(
    conn: &Connection,
    worktree_path: &str,
    now: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE agent_worktrees
         SET last_seen_alive = ?2, finished_at = 0, commits_ahead = 0
         WHERE worktree_path = ?1",
        rusqlite::params![worktree_path, now],
    )?;
    Ok(())
}

/// Mark a session finished, with the commits-ahead count measured at finish.
fn mark_agent_worktree_finished_on(
    conn: &Connection,
    worktree_path: &str,
    finished_at: i64,
    commits_ahead: u32,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE agent_worktrees SET finished_at = ?2, commits_ahead = ?3
         WHERE worktree_path = ?1",
        rusqlite::params![worktree_path, finished_at, commits_ahead],
    )?;
    Ok(())
}

/// Record the PR opened from this worktree's branch (clears its attention).
fn set_agent_worktree_pr_on(
    conn: &Connection,
    worktree_path: &str,
    pr_url: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE agent_worktrees SET pr_url = ?2 WHERE worktree_path = ?1",
        rusqlite::params![worktree_path, pr_url],
    )?;
    Ok(())
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
    try_with_conn(|c| record_agent_worktree_on(c, wt))
}

/// All recorded agent-worktree dispatches, newest first.
pub fn agent_worktrees() -> Vec<AgentWorktree> {
    with_conn(Vec::new(), agent_worktrees_on)
}

/// Forget a dispatched worktree (after it's removed on disk).
pub fn remove_agent_worktree(worktree_path: &str) -> Result<(), String> {
    try_with_conn(|c| remove_agent_worktree_on(c, worktree_path))
}

/// Record a live-session sighting (bumps `last_seen_alive`, clears finished).
pub fn mark_agent_worktree_alive(worktree_path: &str, now: i64) -> Result<(), String> {
    try_with_conn(|c| mark_agent_worktree_alive_on(c, worktree_path, now))
}

/// Mark a dispatched session finished with its commits-ahead count.
pub fn mark_agent_worktree_finished(
    worktree_path: &str,
    finished_at: i64,
    commits_ahead: u32,
) -> Result<(), String> {
    try_with_conn(|c| mark_agent_worktree_finished_on(c, worktree_path, finished_at, commits_ahead))
}

/// Record the PR opened from a dispatched worktree's branch.
pub fn set_agent_worktree_pr(worktree_path: &str, pr_url: &str) -> Result<(), String> {
    try_with_conn(|c| set_agent_worktree_pr_on(c, worktree_path, pr_url))
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
    try_with_conn(clear_ai_on)
}

// ── Dirty-since tracking (at-risk work) ─────────────────────────────────────
// The first time each repo was seen with a dirty working tree, so "dirty for
// N days" can be judged. Updated on every snapshot write (`store_repos`): a
// newly dirty repo gets `now`, an already-dirty one keeps its timestamp, and a
// clean (or vanished) repo is cleared. Durable state, not a cache: never
// dropped on a CACHE_SCHEMA bump.

pub(crate) fn record_dirty_on(
    conn: &mut Connection,
    repos: &[Repo],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut mark =
            tx.prepare("INSERT OR IGNORE INTO dirty_since (id, since) VALUES (?1, ?2)")?;
        let mut clear = tx.prepare("DELETE FROM dirty_since WHERE id = ?1")?;
        for repo in repos {
            if repo.git.dirty > 0 {
                mark.execute(rusqlite::params![repo.id, now])?;
            } else {
                clear.execute([&repo.id])?;
            }
        }
    }
    // Repos no longer in the snapshot.
    let ids: HashSet<&str> = repos.iter().map(|r| r.id.as_str()).collect();
    let stale: Vec<String> = dirty_since_all_on(&tx)
        .into_keys()
        .filter(|id| !ids.contains(id.as_str()))
        .collect();
    for id in stale {
        tx.execute("DELETE FROM dirty_since WHERE id = ?1", [id])?;
    }
    tx.commit()
}

pub(crate) fn dirty_since_all_on(conn: &Connection) -> HashMap<String, i64> {
    let Ok(mut stmt) = conn.prepare("SELECT id, since FROM dirty_since") else {
        return HashMap::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
    match rows {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => HashMap::new(),
    }
}

/// When each currently dirty repo was first seen dirty (unix seconds), keyed
/// by `Repo::id`. Clean repos are absent.
pub fn dirty_since_all() -> HashMap<String, i64> {
    with_conn(HashMap::new(), dirty_since_all_on)
}

// ── Disk usage (janitor) ────────────────────────────────────────────────────

pub(crate) fn store_disk_usage_on(
    conn: &Connection,
    usage: &crate::janitor::DiskUsage,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(usage)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT OR REPLACE INTO disk_usage (id, data, measured_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![usage.repo_id, json, usage.measured_at],
    )?;
    Ok(())
}

pub(crate) fn all_disk_usage_on(conn: &Connection) -> HashMap<String, crate::janitor::DiskUsage> {
    let Ok(mut stmt) = conn.prepare("SELECT id, data FROM disk_usage") else {
        return HashMap::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter_map(|(id, json)| Some((id, serde_json::from_str(&json).ok()?)))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

pub(crate) fn fresh_disk_usage_ids_on(
    conn: &Connection,
    max_age_secs: i64,
    now: i64,
) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT id, measured_at FROM disk_usage") else {
        return HashSet::new();
    };
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter(|(_, at)| now.saturating_sub(*at) <= max_age_secs)
            .map(|(id, _)| id)
            .collect(),
        Err(_) => HashSet::new(),
    }
}

pub(crate) fn clear_disk_usage_on(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM disk_usage WHERE id = ?1", [id])?;
    Ok(())
}

/// Persist a repo's artefact measurement (replacing any earlier one).
pub fn store_disk_usage(usage: &crate::janitor::DiskUsage) -> Result<(), String> {
    try_with_conn(|c| store_disk_usage_on(c, usage))
}

/// Every cached measurement, keyed by repo id. Stale entries are included
/// (last known beats nothing); check `measured_at` or use
/// [`fresh_disk_usage_ids`] to decide what to re-measure.
pub fn all_disk_usage() -> HashMap<String, crate::janitor::DiskUsage> {
    with_conn(HashMap::new(), all_disk_usage_on)
}

/// Repo ids measured within `max_age_secs` (see `janitor::TTL_SECS`).
pub fn fresh_disk_usage_ids(max_age_secs: i64, now: i64) -> HashSet<String> {
    with_conn(HashSet::new(), |c| {
        fresh_disk_usage_ids_on(c, max_age_secs, now)
    })
}

/// Forget a repo's measurement (after a purge, so it's re-measured).
pub fn clear_disk_usage(id: &str) {
    with_conn((), |c| {
        let _ = clear_disk_usage_on(c, id);
    });
}

// ── Security advisories ─────────────────────────────────────────────────────
// Keyed (source host, key) like ci_cache: ("github.com", "owner/repo") for
// Dependabot, ("local", repo id) for a local `cargo audit` / `npm audit`.

pub(crate) fn store_advisories_on(
    conn: &Connection,
    host: &str,
    key: &str,
    report: &crate::advisories::AdvisoryReport,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(report)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT OR REPLACE INTO advisory_cache (host, slug, data, updated_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![host, key, json, report.checked_at],
    )?;
    Ok(())
}

pub(crate) fn all_advisories_on(
    conn: &Connection,
) -> HashMap<(String, String), crate::advisories::AdvisoryReport> {
    keyed_rows_on(conn, "SELECT host, slug, data FROM advisory_cache", |row| {
        row.get::<_, String>(2)
    })
    .into_iter()
    .filter_map(|(key, json)| Some((key, serde_json::from_str(&json).ok()?)))
    .collect()
}

pub(crate) fn fresh_advisory_keys_on(
    conn: &Connection,
    max_age_secs: i64,
    now: i64,
) -> HashSet<(String, String)> {
    fresh_keys_on(
        conn,
        "SELECT host, slug, updated_at FROM advisory_cache",
        max_age_secs,
        now,
    )
}

/// Every cached advisory report, keyed (source host, key).
pub fn all_advisories() -> HashMap<(String, String), crate::advisories::AdvisoryReport> {
    with_conn(HashMap::new(), all_advisories_on)
}

/// Keys whose cached advisory report is within `max_age_secs`.
pub fn fresh_advisory_keys(max_age_secs: i64, now: i64) -> HashSet<(String, String)> {
    with_conn(HashSet::new(), |c| {
        fresh_advisory_keys_on(c, max_age_secs, now)
    })
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
            worktree_of: None,
            worktree_branch: None,
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
            last_seen_alive: 0,
            finished_at: 0,
            commits_ahead: 0,
            pr_url: String::new(),
        };
        let b = AgentWorktree {
            worktree_path: "/data/worktrees/repo-agent-y-2222".into(),
            repo_id: "/dev/repo".into(),
            branch: "agent/y-2222".into(),
            worktree_name: "agent-y-2222".into(),
            prompt: "do y".into(),
            created_at: 200,
            last_seen_alive: 0,
            finished_at: 0,
            commits_ahead: 0,
            pr_url: String::new(),
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
    fn agent_worktree_outcome_lifecycle() {
        let conn = mem();
        let wt = AgentWorktree {
            worktree_path: "/data/worktrees/repo-agent-x-1111".into(),
            repo_id: "/dev/repo".into(),
            branch: "agent/x-1111".into(),
            worktree_name: "agent-x-1111".into(),
            prompt: "fix x".into(),
            created_at: 100,
            last_seen_alive: 0,
            finished_at: 0,
            commits_ahead: 0,
            pr_url: String::new(),
        };
        record_agent_worktree_on(&conn, &wt).unwrap();

        // Sighting → last_seen_alive set, not finished.
        mark_agent_worktree_alive_on(&conn, &wt.worktree_path, 150).unwrap();
        let row = &agent_worktrees_on(&conn)[0];
        assert_eq!((row.last_seen_alive, row.finished_at), (150, 0));

        // Finish → finished_at + commits_ahead recorded.
        mark_agent_worktree_finished_on(&conn, &wt.worktree_path, 300, 4).unwrap();
        let row = &agent_worktrees_on(&conn)[0];
        assert_eq!((row.finished_at, row.commits_ahead), (300, 4));

        // Resume → finished state clears, sighting updates.
        mark_agent_worktree_alive_on(&conn, &wt.worktree_path, 400).unwrap();
        let row = &agent_worktrees_on(&conn)[0];
        assert_eq!(
            (row.last_seen_alive, row.finished_at, row.commits_ahead),
            (400, 0, 0)
        );

        // Opened PR persists (and survives an alive-mark: hand-off is done).
        set_agent_worktree_pr_on(&conn, &wt.worktree_path, "https://github.com/o/r/pull/1")
            .unwrap();
        mark_agent_worktree_alive_on(&conn, &wt.worktree_path, 500).unwrap();
        let row = &agent_worktrees_on(&conn)[0];
        assert_eq!(row.pr_url, "https://github.com/o/r/pull/1");

        // Updates to an unknown path are no-ops, not errors.
        mark_agent_worktree_alive_on(&conn, "/nope", 1).unwrap();
        mark_agent_worktree_finished_on(&conn, "/nope", 1, 1).unwrap();
        set_agent_worktree_pr_on(&conn, "/nope", "u").unwrap();
    }

    #[test]
    fn agent_worktrees_migrates_pre_outcome_schema() {
        // A database created before the outcome columns existed (#197's shape)
        // must gain them via the additive ALTERs without losing rows.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_worktrees (
                 worktree_path TEXT PRIMARY KEY,
                 repo_id TEXT NOT NULL,
                 branch TEXT NOT NULL,
                 worktree_name TEXT NOT NULL,
                 prompt TEXT NOT NULL,
                 created_at INTEGER NOT NULL);
             INSERT INTO agent_worktrees VALUES ('/wt', '/repo', 'agent/x', 'agent-x', 'p', 9);",
        )
        .unwrap();
        init(&conn).unwrap();
        let rows = agent_worktrees_on(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].worktree_path, "/wt");
        assert_eq!(rows[0].created_at, 9);
        assert_eq!(rows[0].last_seen_alive, 0);
        assert_eq!(rows[0].finished_at, 0);
        assert_eq!(rows[0].commits_ahead, 0);
        assert_eq!(rows[0].pr_url, "");
        // Idempotent: a second init leaves the shape intact.
        init(&conn).unwrap();
        assert_eq!(agent_worktrees_on(&conn).len(), 1);
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
    fn store_host_infos_batches_in_one_transaction() {
        let conn = mem();
        let entries = vec![
            (
                "github.com".to_string(),
                "o/a".to_string(),
                HostInfo {
                    stars: 1,
                    ..HostInfo::default()
                },
            ),
            (
                "gitlab.acme.io".to_string(),
                "o/a".to_string(),
                HostInfo {
                    private: true,
                    ..HostInfo::default()
                },
            ),
        ];
        store_host_infos_on(&conn, &entries, 1_000).unwrap();
        let all = all_host_info_on(&conn);
        assert_eq!(all.len(), 2);
        assert_eq!(all[&("github.com".into(), "o/a".into())].stars, 1);
        assert!(all[&("gitlab.acme.io".into(), "o/a".into())].private);
        // Stamped `now`, so both read as fresh; a re-store replaces, not appends.
        assert_eq!(fresh_host_keys_on(&conn, 10, 1_005).len(), 2);
        store_host_infos_on(&conn, &entries[..1], 2_000).unwrap();
        assert_eq!(all_host_info_on(&conn).len(), 2);
        assert_eq!(fresh_host_keys_on(&conn, 10, 2_005).len(), 1);
        // The connection is usable (no transaction left open) afterwards.
        assert!(conn.is_autocommit());
    }

    #[test]
    fn all_host_info_skips_unparseable_rows() {
        let conn = mem();
        store_host_info_on(&conn, "github.com", "o/ok", &HostInfo::default(), 1);
        conn.execute(
            "INSERT INTO host_cache (host, slug, data, fetched_at) VALUES ('github.com', 'o/bad', 'nope', 1)",
            [],
        )
        .unwrap();
        let all = all_host_info_on(&conn);
        assert_eq!(all.len(), 1);
        assert!(all.contains_key(&("github.com".into(), "o/ok".into())));
    }

    #[test]
    fn open_at_enables_wal_and_inits_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("cache.sqlite");
        let conn = open_at(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        set_favorite_on(&conn, "/a", true).unwrap();
        assert!(INITIALIZED.lock().unwrap().contains(&path));
        // A second open skips init but sees the same, initialized schema.
        let again = open_at(&path).unwrap();
        assert!(favorites_on(&again).contains("/a"));
    }

    #[test]
    fn concurrent_first_init_does_not_race_the_alter() {
        // Two connections initializing a pre-#185 database at once must not
        // both see the column missing and both ALTER ("duplicate column").
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE agent_worktrees (
                     worktree_path TEXT PRIMARY KEY, repo_id TEXT NOT NULL,
                     branch TEXT NOT NULL, worktree_name TEXT NOT NULL,
                     prompt TEXT NOT NULL, created_at INTEGER NOT NULL);",
            )
            .unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let conn = Connection::open(&path).unwrap();
                    conn.busy_timeout(BUSY_TIMEOUT).unwrap();
                    init(&conn)
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let conn = Connection::open(&path).unwrap();
        assert!(agent_worktrees_on(&conn).is_empty());
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

    #[test]
    fn dirty_since_keeps_first_seen_and_clears_on_clean_or_gone() {
        let mut conn = mem();
        let mut a = sample("/a");
        a.git.dirty = 2;
        let mut b = sample("/b");
        b.git.dirty = 1;
        record_dirty_on(&mut conn, &[a.clone(), b.clone()], 100).unwrap();
        // Still dirty later: the first-seen time sticks.
        record_dirty_on(&mut conn, &[a.clone(), b.clone()], 500).unwrap();
        let since = dirty_since_all_on(&conn);
        assert_eq!(since.get("/a"), Some(&100));
        assert_eq!(since.get("/b"), Some(&100));

        // /a is committed (clean); /b vanishes from the snapshot.
        a.git.dirty = 0;
        record_dirty_on(&mut conn, &[a.clone()], 600).unwrap();
        assert!(dirty_since_all_on(&conn).is_empty());

        // Dirty again: a fresh first-seen time.
        a.git.dirty = 1;
        record_dirty_on(&mut conn, &[a], 700).unwrap();
        assert_eq!(dirty_since_all_on(&conn).get("/a"), Some(&700));
    }

    #[test]
    fn disk_usage_round_trips_with_ttl_and_clear() {
        use crate::janitor::{ArtifactDir, ArtifactKind, DiskUsage};
        let conn = mem();
        let usage = DiskUsage {
            repo_id: "/a".into(),
            dirs: vec![ArtifactDir {
                kind: ArtifactKind::NodeModules,
                path: "/a/node_modules".into(),
                rel: "node_modules".into(),
                bytes: 1234,
                files: 3,
                truncated: false,
            }],
            total_bytes: 1234,
            truncated: false,
            measured_at: 1_000,
        };
        store_disk_usage_on(&conn, &usage).unwrap();
        assert_eq!(all_disk_usage_on(&conn).get("/a"), Some(&usage));
        assert!(fresh_disk_usage_ids_on(&conn, 100, 1_050).contains("/a"));
        assert!(fresh_disk_usage_ids_on(&conn, 100, 1_200).is_empty());
        clear_disk_usage_on(&conn, "/a").unwrap();
        assert!(all_disk_usage_on(&conn).is_empty());
    }
}
