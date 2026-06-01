//! SQLite-backed cache (`~/.local/share/orrery/cache.sqlite`): persists user
//! favorites and a snapshot of scanned repos so the grid paints instantly on
//! launch and survives offline. Connections are opened per call — these are
//! low-frequency operations and SQLite handles the locking.

use std::collections::HashSet;
use std::path::PathBuf;

use rusqlite::Connection;

use crate::model::Repo;

fn db_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("orrery").join("cache.sqlite"))
}

fn open() -> Result<Connection, String> {
    let path = db_path().ok_or("no data directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(&path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS favorites (id TEXT PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS repos (id TEXT PRIMARY KEY, data TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn favorites_on(conn: &Connection) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT id FROM favorites") else {
        return HashSet::new();
    };
    match stmt.query_map([], |row| row.get::<_, String>(0)) {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => HashSet::new(),
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
    if favorite {
        conn.execute("INSERT OR IGNORE INTO favorites (id) VALUES (?1)", [id])
    } else {
        conn.execute("DELETE FROM favorites WHERE id = ?1", [id])
    }
    .map_err(|e| e.to_string())?;
    Ok(favorite)
}

/// Replace the cached repo snapshot.
pub fn store_repos(repos: &[Repo]) -> Result<(), String> {
    let mut conn = open()?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM repos", []).map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare("INSERT INTO repos (id, data) VALUES (?1, ?2)")
            .map_err(|e| e.to_string())?;
        for repo in repos {
            let json = serde_json::to_string(repo).map_err(|e| e.to_string())?;
            stmt.execute(rusqlite::params![repo.id, json])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Load the cached repo snapshot (for instant paint before a fresh scan).
pub fn load_repos() -> Vec<Repo> {
    let Ok(conn) = open() else {
        return Vec::new();
    };
    let favs = favorites_on(&conn);
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
