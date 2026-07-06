//! Semantic fleet recall — the embedding store + similarity layer (#186).
//!
//! The `embeddings` cache table holds chunked f32 vectors keyed by
//! (host, slug, source, chunk_ix): one repo can contribute several `source`
//! kinds ('readme', 'description', 'notes', 'commits', …), each split into
//! chunks. Keying by (host, slug) — never slug alone — follows the #159
//! lesson: the same "owner/repo" slug on two hosts must not share rows.
//!
//! Search is brute-force cosine over every stored row ([`top_k`]). At fleet
//! scale (a few thousand vectors) that is sub-millisecond, so there is
//! deliberately no vector DB and no ANN index.
//!
//! Layer split: the table DDL lives in `cache.rs` next to the rest of the
//! schema so versioning/migration stays in one place; the row helpers live
//! here, following the cache convention — public fns open the connection,
//! the logic sits in `*_on(conn)` variants unit-tested against in-memory
//! SQLite.
//!
//! The corpus indexing pipeline and palette recall mode are separate #186
//! workstreams. Until they land, the [`index`]/[`search`] pair at the bottom
//! keeps the #41-era whole-repo pipeline working on top of this store.

use rusqlite::Connection;

use crate::{ai, cache, config};

/// Minimum cosine similarity for a query↔repo match to surface.
const MIN_SCORE: f32 = 0.35;
/// Max ranked hits returned for a query.
const MAX_HITS: usize = 8;
/// How many repos to embed concurrently per batch.
const BATCH: usize = 6;

/// One stored chunk of the embedding index, decoded and ready to rank.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingRow {
    pub host: String,
    pub slug: String,
    pub source: String,
    pub chunk_ix: i64,
    pub content: String,
    pub vector: Vec<f32>,
}

/// A ranked search result: which repo/source matched and the chunk text that
/// matched, for showing as context in the palette.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredHit {
    pub host: String,
    pub slug: String,
    pub source: String,
    pub content: String,
    pub score: f32,
}

/// Size of the embedding index, for the Settings display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Stored chunks (rows).
    pub chunks: usize,
    /// Distinct (host, slug) pairs with at least one chunk.
    pub repos: usize,
    /// Total bytes of stored vectors + chunk text.
    pub bytes: u64,
}

// ── Vector encoding ─────────────────────────────────────────────────────────

/// Encode a vector as a little-endian f32 blob (4 bytes per component).
pub fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode a little-endian f32 blob. `None` when the length isn't a multiple
/// of 4 — a truncated/foreign blob must not silently decode to garbage.
pub fn decode_vector(b: &[u8]) -> Option<Vec<f32>> {
    if !b.len().is_multiple_of(4) {
        return None;
    }
    Some(
        b.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

// ── Row helpers (`_on(conn)` per the cache convention) ──────────────────────

/// Replace all stored chunks for a (host, slug, source) with `chunks`
/// (content + vector, indexed in order), atomically. A repo whose readme
/// shrank from 5 chunks to 2 must not keep 3 stale rows.
pub fn store_embeddings_on(
    conn: &mut Connection,
    host: &str,
    slug: &str,
    source: &str,
    chunks: &[(String, Vec<f32>)],
    now: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM embeddings WHERE host = ?1 AND slug = ?2 AND source = ?3",
        rusqlite::params![host, slug, source],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO embeddings (host, slug, source, chunk_ix, content, vector, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for (ix, (content, vector)) in chunks.iter().enumerate() {
            stmt.execute(rusqlite::params![
                host,
                slug,
                source,
                ix as i64,
                content,
                encode_vector(vector),
                now
            ])?;
        }
    }
    tx.commit()
}

/// Delete a repo's stored chunks — all of them, or only one `source` kind
/// (e.g. just 'notes' when a note is cleared). Returns the rows removed.
pub fn delete_embeddings_on(
    conn: &Connection,
    host: &str,
    slug: &str,
    source: Option<&str>,
) -> rusqlite::Result<usize> {
    match source {
        Some(source) => conn.execute(
            "DELETE FROM embeddings WHERE host = ?1 AND slug = ?2 AND source = ?3",
            rusqlite::params![host, slug, source],
        ),
        None => conn.execute(
            "DELETE FROM embeddings WHERE host = ?1 AND slug = ?2",
            rusqlite::params![host, slug],
        ),
    }
}

/// Load the whole index for a search pass. Rows whose blob fails to decode
/// are skipped (they can only appear via corruption; a rebuild restores them).
pub fn all_embeddings_on(conn: &Connection) -> Vec<EmbeddingRow> {
    let Ok(mut stmt) =
        conn.prepare("SELECT host, slug, source, chunk_ix, content, vector FROM embeddings")
    else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Vec<u8>>(5)?,
        ))
    });
    match rows {
        Ok(iter) => iter
            .flatten()
            .filter_map(|(host, slug, source, chunk_ix, content, blob)| {
                decode_vector(&blob).map(|vector| EmbeddingRow {
                    host,
                    slug,
                    source,
                    chunk_ix,
                    content,
                    vector,
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Count/size of the index (zeros on error). char(31) keeps the DISTINCT
/// pair count safe against host/slug concatenation collisions.
pub fn index_stats_on(conn: &Connection) -> IndexStats {
    conn.query_row(
        "SELECT count(*),
                count(DISTINCT host || char(31) || slug),
                coalesce(sum(length(vector) + length(content)), 0)
         FROM embeddings",
        [],
        |r| {
            Ok(IndexStats {
                chunks: r.get::<_, i64>(0)? as usize,
                repos: r.get::<_, i64>(1)? as usize,
                bytes: r.get::<_, i64>(2)? as u64,
            })
        },
    )
    .unwrap_or_default()
}

// ── Public wrappers (open the on-disk cache) ────────────────────────────────

/// Replace the stored chunks for a (host, slug, source). See
/// [`store_embeddings_on`].
pub fn store_embeddings(
    host: &str,
    slug: &str,
    source: &str,
    chunks: &[(String, Vec<f32>)],
    now: i64,
) -> Result<(), String> {
    let mut conn = cache::open()?;
    store_embeddings_on(&mut conn, host, slug, source, chunks, now).map_err(|e| e.to_string())
}

/// Delete a repo's stored chunks (all sources, or one). Returns rows removed.
pub fn delete_embeddings(host: &str, slug: &str, source: Option<&str>) -> Result<usize, String> {
    let conn = cache::open()?;
    delete_embeddings_on(&conn, host, slug, source).map_err(|e| e.to_string())
}

/// Load the whole index (empty on error — search just finds nothing).
pub fn all_embeddings() -> Vec<EmbeddingRow> {
    match cache::open() {
        Ok(conn) => all_embeddings_on(&conn),
        Err(_) => Vec::new(),
    }
}

/// Count/size of the index, for the Settings display.
pub fn index_stats() -> IndexStats {
    match cache::open() {
        Ok(conn) => index_stats_on(&conn),
        Err(_) => IndexStats::default(),
    }
}

// ── Similarity + ranking ────────────────────────────────────────────────────

/// Cosine similarity in [-1, 1]. Mismatched lengths, empty, and zero-norm
/// vectors all return 0.0 rather than `None`: for ranking, "can't compare"
/// and "no similarity" are the same outcome, and a plain f32 keeps the
/// [`top_k`] sort total. (A length mismatch means the row was embedded with
/// a different model than the query — such rows simply never rank.)
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Brute-force rank `rows` against `query`, best `k` first. This *is* the
/// search engine: no vector DB, no ANN — a linear scan is sub-millisecond at
/// fleet scale. Equal scores tie-break on (host, slug, source, chunk_ix) so
/// results are deterministic run to run.
pub fn top_k(query: &[f32], rows: Vec<EmbeddingRow>, k: usize) -> Vec<ScoredHit> {
    let mut scored: Vec<(f32, EmbeddingRow)> = rows
        .into_iter()
        .map(|r| (cosine_similarity(query, &r.vector), r))
        .collect();
    scored.sort_by(|(sa, ra), (sb, rb)| {
        sb.total_cmp(sa).then_with(|| {
            (&ra.host, &ra.slug, &ra.source, ra.chunk_ix).cmp(&(
                &rb.host,
                &rb.slug,
                &rb.source,
                rb.chunk_ix,
            ))
        })
    });
    scored.truncate(k);
    scored
        .into_iter()
        .map(|(score, r)| ScoredHit {
            host: r.host,
            slug: r.slug,
            source: r.source,
            content: r.content,
            score,
        })
        .collect()
}

// ── Legacy whole-repo pipeline (#41) ────────────────────────────────────────
// One vector per repo (name/slug/language/description), keyed by repo id.
// Ported onto the chunked store so the palette keeps working until the #186
// indexing + recall workstreams replace it. Rows are stored under an empty
// host with the repo id (a path) standing in for the slug, tagged with a
// dedicated source so real (host, slug) corpus rows never mix with them.

/// Source tag for legacy whole-repo rows.
const LEGACY_SOURCE: &str = "repo";

/// Embed each `(id, text)` whose text changed since the last index, caching the
/// vector + a signature so unchanged repos are skipped. Returns how many were
/// (re-)embedded. Embedding failures (AI unreachable) are swallowed — the index
/// just stays as-is.
pub async fn index(items: &[(String, String)]) -> usize {
    let model = config::load().embed_model;
    let now = unix_now();
    let mut count = 0usize;
    for chunk in items.chunks(BATCH) {
        let done = futures_util::future::join_all(chunk.iter().map(|(id, text)| {
            let model = model.clone();
            async move {
                let key = format!("embed_sig:{id}");
                let sig = text_signature(text);
                if cache::get_meta(&key).as_deref() == Some(sig.as_str()) {
                    return false; // unchanged — skip the embed call
                }
                match ai::embed(&model, text).await {
                    Ok(vec) => {
                        let stored =
                            store_embeddings("", id, LEGACY_SOURCE, &[(text.clone(), vec)], now);
                        if stored.is_ok() {
                            cache::set_meta(&key, &sig);
                        }
                        stored.is_ok()
                    }
                    Err(_) => false,
                }
            }
        }))
        .await;
        count += done.into_iter().filter(|x| *x).count();
    }
    count
}

/// Rank the embedding index against `query`, returning `(repo id, score)` for the
/// top matches above the similarity floor. Empty when the query is blank or the
/// embedding backend is unreachable.
pub async fn search(query: &str) -> Vec<(String, f32)> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let model = config::load().embed_model;
    let Ok(q) = ai::embed(&model, query).await else {
        return Vec::new();
    };
    let rows: Vec<EmbeddingRow> = all_embeddings()
        .into_iter()
        .filter(|r| r.host.is_empty() && r.source == LEGACY_SOURCE)
        .collect();
    top_k(&q, rows, MAX_HITS)
        .into_iter()
        .filter(|h| h.score > MIN_SCORE)
        .map(|h| (h.slug, h.score))
        .collect()
}

/// Stable hex fingerprint of a repo's embedding text, for skip-if-unchanged.
fn text_signature(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:x}", h.finish())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        cache::init(&conn).unwrap();
        conn
    }

    fn chunks(vecs: &[&[f32]]) -> Vec<(String, Vec<f32>)> {
        vecs.iter()
            .enumerate()
            .map(|(i, v)| (format!("chunk {i}"), v.to_vec()))
            .collect()
    }

    fn row(host: &str, slug: &str, source: &str, chunk_ix: i64, vector: &[f32]) -> EmbeddingRow {
        EmbeddingRow {
            host: host.into(),
            slug: slug.into(),
            source: source.into(),
            chunk_ix,
            content: format!("{slug} {source} {chunk_ix}"),
            vector: vector.to_vec(),
        }
    }

    #[test]
    fn vector_blob_roundtrips() {
        let v = vec![0.0f32, 1.0, -1.0, f32::MIN_POSITIVE, 12345.678, -0.25];
        let blob = encode_vector(&v);
        assert_eq!(blob.len(), v.len() * 4);
        assert_eq!(decode_vector(&blob), Some(v));
        // Empty is a valid (empty) vector; a truncated blob is not.
        assert_eq!(decode_vector(&[]), Some(Vec::new()));
        assert_eq!(decode_vector(&[0, 0, 0]), None);
    }

    #[test]
    fn store_replaces_per_source_and_deletes_per_repo_or_source() {
        let mut conn = mem();
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[1.0, 0.0], &[0.0, 1.0]]),
            100,
        )
        .unwrap();
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "notes",
            &chunks(&[&[0.5, 0.5]]),
            100,
        )
        .unwrap();
        assert_eq!(all_embeddings_on(&conn).len(), 3);

        // Re-storing a source replaces its rows: 2 readme chunks become 1,
        // with no stale chunk_ix=1 left behind.
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[0.9, 0.1]]),
            200,
        )
        .unwrap();
        let rows = all_embeddings_on(&conn);
        assert_eq!(rows.len(), 2);
        let readme: Vec<_> = rows.iter().filter(|r| r.source == "readme").collect();
        assert_eq!(readme.len(), 1);
        assert_eq!(readme[0].chunk_ix, 0);
        assert_eq!(readme[0].vector, vec![0.9, 0.1]);

        // Delete one source, then the whole repo.
        assert_eq!(
            delete_embeddings_on(&conn, "github.com", "o/test", Some("notes")).unwrap(),
            1
        );
        assert_eq!(all_embeddings_on(&conn).len(), 1);
        assert_eq!(
            delete_embeddings_on(&conn, "github.com", "o/test", None).unwrap(),
            1
        );
        assert!(all_embeddings_on(&conn).is_empty());
    }

    #[test]
    fn same_slug_on_two_hosts_does_not_collide() {
        // The #159 lesson: "owner/repo" on github.com and on a self-hosted
        // GitLab are different repos and must keep independent rows.
        let mut conn = mem();
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[1.0, 0.0]]),
            100,
        )
        .unwrap();
        store_embeddings_on(
            &mut conn,
            "gitlab.acme.io",
            "o/test",
            "readme",
            &chunks(&[&[0.0, 1.0]]),
            100,
        )
        .unwrap();
        assert_eq!(all_embeddings_on(&conn).len(), 2);

        // Deleting the github repo must not touch the self-hosted one.
        delete_embeddings_on(&conn, "github.com", "o/test", None).unwrap();
        let rows = all_embeddings_on(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].host, "gitlab.acme.io");
        assert_eq!(rows[0].vector, vec![0.0, 1.0]);
    }

    #[test]
    fn cosine_edge_cases() {
        let a = [1.0f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6); // identical
        assert!(cosine_similarity(&a, &[0.0, 1.0, 0.0]).abs() < 1e-6); // orthogonal
        assert!((cosine_similarity(&a, &[-1.0, 0.0, 0.0]) + 1.0).abs() < 1e-6); // opposite
        assert_eq!(cosine_similarity(&a, &[1.0, 0.0]), 0.0); // mismatched length
        assert_eq!(cosine_similarity(&a, &[0.0, 0.0, 0.0]), 0.0); // zero vector
        assert_eq!(cosine_similarity(&[], &[]), 0.0); // empty
    }

    #[test]
    fn top_k_orders_and_truncates() {
        let rows = vec![
            row("github.com", "o/far", "readme", 0, &[0.0, 1.0]), // score 0
            row("github.com", "o/near", "readme", 0, &[1.0, 0.0]), // score 1
            row("github.com", "o/mid", "notes", 0, &[1.0, 1.0]),  // score ≈0.707
            row("github.com", "o/alien", "readme", 0, &[1.0]),    // wrong dim → 0
        ];
        let hits = top_k(&[1.0, 0.0], rows.clone(), 2);
        assert_eq!(hits.len(), 2, "k must truncate");
        assert_eq!(hits[0].slug, "o/near");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert_eq!(hits[1].slug, "o/mid");
        assert_eq!(hits[1].source, "notes");
        assert_eq!(hits[1].content, "o/mid notes 0");

        // k larger than the index returns everything, still best-first; the
        // wrong-dimension row scores 0.0 and sinks rather than erroring.
        let all = top_k(&[1.0, 0.0], rows.clone(), 10);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].slug, "o/near");
        assert!(top_k(&[1.0, 0.0], rows, 0).is_empty());
    }

    #[test]
    fn top_k_ties_break_deterministically() {
        // Identical scores order by (host, slug, source, chunk_ix).
        let v: &[f32] = &[1.0, 0.0];
        let rows = vec![
            row("gitlab.acme.io", "o/a", "readme", 0, v),
            row("github.com", "o/b", "readme", 1, v),
            row("github.com", "o/b", "readme", 0, v),
            row("github.com", "o/a", "notes", 0, v),
        ];
        let hits = top_k(v, rows, 4);
        let order: Vec<(&str, &str, &str)> = hits
            .iter()
            .map(|h| (h.host.as_str(), h.slug.as_str(), h.source.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![
                ("github.com", "o/a", "notes"),
                ("github.com", "o/b", "readme"), // chunk 0 before chunk 1
                ("github.com", "o/b", "readme"),
                ("gitlab.acme.io", "o/a", "readme"),
            ]
        );
    }

    #[test]
    fn index_stats_counts_chunks_repos_and_bytes() {
        let mut conn = mem();
        assert_eq!(index_stats_on(&conn), IndexStats::default());

        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[1.0, 0.0], &[0.0, 1.0]]),
            100,
        )
        .unwrap();
        // Same slug, different host — a second repo (the #159 lesson again).
        store_embeddings_on(
            &mut conn,
            "gitlab.acme.io",
            "o/test",
            "readme",
            &chunks(&[&[1.0, 0.0]]),
            100,
        )
        .unwrap();
        let stats = index_stats_on(&conn);
        assert_eq!(stats.chunks, 3);
        assert_eq!(stats.repos, 2);
        // 3 vectors × 2 f32 × 4 bytes, plus the "chunk N" content text.
        assert_eq!(stats.bytes, 3 * 8 + 3 * "chunk 0".len() as u64);
    }
}
