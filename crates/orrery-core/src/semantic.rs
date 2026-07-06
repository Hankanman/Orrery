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
//! Indexing ([`index_fleet`]) builds a per-repo corpus — readme (chunked),
//! description, topics, notes, recent commit subjects — and embeds only the
//! (repo, source) pairs whose text signature changed since the last pass
//! (`embed_sig:*` meta keys), pacing itself so a cold fleet doesn't hammer the
//! backend. Recall ([`recall`]) ranks a query vector against the whole index,
//! best chunk per repo. Both are dormant unless the backend can embed.

use rusqlite::Connection;

use crate::model::Repo;
use crate::{ai, cache, config, git_ops};

/// Minimum cosine similarity for a query↔repo match to surface.
pub const MIN_SCORE: f32 = 0.35;
/// Max ranked repos returned for a query.
pub const MAX_HITS: usize = 8;
/// How many chunks to embed concurrently.
const BATCH: usize = 4;

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

/// Rank the whole index against a query vector: the best-scoring chunk per
/// repo, best repos first, floored at `min_score` and capped at `k`. Pure —
/// this is the palette recall engine; the UI resolves (host, slug) back to a
/// repo row and shows `content` as the matching snippet.
pub fn recall(query: &[f32], rows: &[EmbeddingRow], k: usize, min_score: f32) -> Vec<ScoredHit> {
    let mut best: std::collections::HashMap<(&str, &str), (f32, &EmbeddingRow)> =
        std::collections::HashMap::new();
    for r in rows {
        let score = cosine_similarity(query, &r.vector);
        if score < min_score {
            continue;
        }
        let entry = best.entry((r.host.as_str(), r.slug.as_str()));
        match entry {
            std::collections::hash_map::Entry::Occupied(mut e) if score > e.get().0 => {
                e.insert((score, r));
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert((score, r));
            }
            _ => {}
        }
    }
    let mut hits: Vec<ScoredHit> = best
        .into_values()
        .map(|(score, r)| ScoredHit {
            host: r.host.clone(),
            slug: r.slug.clone(),
            source: r.source.clone(),
            content: r.content.clone(),
            score,
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| (&a.host, &a.slug).cmp(&(&b.host, &b.slug)))
    });
    hits.truncate(k);
    hits
}

// ── Corpus building (per-repo sources) ──────────────────────────────────────

/// Source kinds a repo contributes to the index. Each is stored, signed, and
/// re-embedded independently, so e.g. a new commit only re-embeds 'commits'.
pub const SOURCE_README: &str = "readme";
pub const SOURCE_DESCRIPTION: &str = "description";
pub const SOURCE_TOPICS: &str = "topics";
pub const SOURCE_NOTES: &str = "notes";
pub const SOURCE_COMMITS: &str = "commits";

/// Chunk packing target/hard-cap, in chars (~200–400 tokens per chunk).
const CHUNK_TARGET_CHARS: usize = 1200;
const CHUNK_MAX_CHARS: usize = 1800;
/// Cap chunks per source so a huge readme can't monopolize an indexing pass.
const MAX_CHUNKS_PER_SOURCE: usize = 12;
/// Cap on readme text read from disk.
const README_MAX_CHARS: usize = 24_000;
/// How many recent commit subjects feed the 'commits' source.
const COMMIT_SUBJECTS: usize = 30;

/// A repo's identity in the embedding store plus its corpus texts, one entry
/// per source kind (present even when empty, so a cleared source is detected
/// and its stale chunks dropped).
#[derive(Debug, Clone, PartialEq)]
pub struct RepoDoc {
    pub host: String,
    pub slug: String,
    /// `(source kind, text)` — the full pre-chunk text per source.
    pub sources: Vec<(String, String)>,
}

/// A repo's key in the embedding store: (remote domain, slug) when it has a
/// remote — the #159 host-keying lesson — else ("", absolute path), which can
/// never collide with a real slug pair.
pub fn repo_key(repo: &Repo) -> (String, String) {
    match &repo.slug {
        Some(slug) => (repo.remote_host.clone().unwrap_or_default(), slug.clone()),
        None => (String::new(), repo.id.clone()),
    }
}

/// Meta key holding the text signature for one (repo, source). Unit-separator
/// joints keep host/slug/source concatenations collision-free, and distinguish
/// these keys from the legacy `embed_sig:{id}` shape.
pub fn sig_key(host: &str, slug: &str, source: &str) -> String {
    format!("embed_sig:{host}\u{1f}{slug}\u{1f}{source}")
}

/// Stable hex fingerprint of a source's text, for skip-if-unchanged.
pub fn text_signature(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:x}", h.finish())
}

/// Split `text` into embedding-sized chunks: markdown headings and blank lines
/// delimit blocks, blocks pack greedily to ~[`CHUNK_TARGET_CHARS`], and a
/// single oversized block hard-splits at whitespace. Empty text → no chunks.
pub fn chunk_text(text: &str) -> Vec<String> {
    // Pass 1: blocks. A heading starts a new block (so a section title stays
    // with its first paragraph); a blank line ends one.
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !cur.trim().is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        if line.starts_with('#') && !cur.trim().is_empty() {
            blocks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.trim().is_empty() {
        blocks.push(cur);
    }

    // Pass 2: pack blocks into chunks.
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for block in blocks {
        let block_len = block.chars().count();
        if !cur.is_empty() && cur.chars().count() + block_len > CHUNK_TARGET_CHARS {
            chunks.push(std::mem::take(&mut cur));
        }
        if block_len > CHUNK_MAX_CHARS {
            if !cur.is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
            chunks.extend(hard_split(&block));
        } else {
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(&block);
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks.truncate(MAX_CHUNKS_PER_SOURCE);
    chunks
}

/// Split an oversized block at whitespace near the cap (mid-word only when a
/// "word" exceeds half a chunk, e.g. minified text).
fn hard_split(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = block;
    while rest.chars().count() > CHUNK_MAX_CHARS {
        let cap = rest
            .char_indices()
            .nth(CHUNK_MAX_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        let head = &rest[..cap];
        let cut = head
            .rfind(char::is_whitespace)
            .filter(|&i| i > cap / 2)
            .unwrap_or(cap);
        let piece = rest[..cut].trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        rest = rest[cut..].trim_start();
    }
    if !rest.trim().is_empty() {
        out.push(rest.trim().to_string());
    }
    out
}

/// Which of a doc's sources need (re)embedding, given the stored signature per
/// source: `(source, text, new signature)` for each changed one. Pure — the
/// signature lookup is injected. A source that is empty *and* was never stored
/// is skipped outright (no rows to write, no signature worth recording); an
/// emptied source that *was* stored is returned so its stale chunks drop.
pub fn changed_sources(
    sources: &[(String, String)],
    stored_sig: impl Fn(&str) -> Option<String>,
) -> Vec<(&str, &str, String)> {
    sources
        .iter()
        .filter_map(|(source, text)| {
            let stored = stored_sig(source);
            if text.trim().is_empty() && stored.is_none() {
                return None;
            }
            let sig = text_signature(text);
            (stored.as_deref() != Some(sig.as_str())).then_some((
                source.as_str(),
                text.as_str(),
                sig,
            ))
        })
        .collect()
}

/// Build a repo's corpus doc: readme from disk, description/topics from the
/// (enriched) snapshot, the drawer note from the cache, and recent commit
/// subjects via git. Sync fs/git/SQLite I/O — call off the UI thread.
pub fn build_doc(repo: &Repo) -> RepoDoc {
    let (host, slug) = repo_key(repo);
    // Name/slug/language ride with the description so "that rust dashboard
    // thing" style queries land even when a repo has no readme.
    let description = format!(
        "{} {} {} {}",
        repo.display_name,
        repo.slug.as_deref().unwrap_or(""),
        repo.language.as_deref().unwrap_or(""),
        repo.description.as_deref().unwrap_or(""),
    );
    let commits = git_ops::recent_log(&repo.id, COMMIT_SUBJECTS)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.summary)
        .collect::<Vec<_>>()
        .join("\n");
    RepoDoc {
        host,
        slug,
        sources: vec![
            (SOURCE_README.into(), read_readme_text(&repo.id)),
            (SOURCE_DESCRIPTION.into(), description),
            (SOURCE_TOPICS.into(), repo.topics.join(", ")),
            (SOURCE_NOTES.into(), cache::note(&repo.id)),
            (SOURCE_COMMITS.into(), commits),
        ],
    }
}

/// The repo's readme text (same candidates as the scanner), capped so a
/// pathological readme can't blow up chunking. Empty when there isn't one.
fn read_readme_text(repo_path: &str) -> String {
    let candidates = [
        "README.md",
        "Readme.md",
        "readme.md",
        "README.markdown",
        "README",
    ];
    let path = std::path::Path::new(repo_path);
    let text = candidates
        .iter()
        .find_map(|name| std::fs::read_to_string(path.join(name)).ok())
        .unwrap_or_default();
    if text.chars().count() > README_MAX_CHARS {
        text.chars().take(README_MAX_CHARS).collect()
    } else {
        text
    }
}

// ── The indexing pass ───────────────────────────────────────────────────────

/// How many repos to embed before pausing, and for how long — gentle pacing so
/// a cold full-fleet index doesn't monopolize the local backend (the enrich
/// precedent: bounded concurrency + skip-fresh).
const REPOS_PER_BURST: usize = 12;
const BURST_PAUSE_MS: u64 = 1500;

/// Index the fleet's corpus incrementally: build each repo's doc, skip sources
/// whose signature is unchanged, embed + store the rest. Returns how many repos
/// had something (re)embedded. Fully graceful: returns 0 without side effects
/// when the backend can't embed, stops early (to retry next pass) when it goes
/// away mid-run, and at most one pass runs at a time process-wide.
pub async fn index_fleet(repos: Vec<Repo>) -> usize {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INDEXING: AtomicBool = AtomicBool::new(false);
    if !ai::embeddings_supported() || INDEXING.swap(true, Ordering::SeqCst) {
        return 0;
    }
    let count = index_fleet_inner(&repos).await;
    INDEXING.store(false, Ordering::SeqCst);
    count
}

/// One source's pending work: (source kind, full text, new signature).
type SourceWork = (String, String, String);

async fn index_fleet_inner(repos: &[Repo]) -> usize {
    let _ = purge_legacy();
    let model = config::load().embed_model;
    let now = unix_now();

    // Plan first (cheap, local): docs + changed sources, deduped by store key
    // (two checkouts of one remote index once, like enrich jobs).
    let mut seen = std::collections::HashSet::new();
    let mut work: Vec<(RepoDoc, Vec<SourceWork>)> = Vec::new();
    for repo in repos {
        let key = repo_key(repo);
        if !seen.insert(key) {
            continue;
        }
        let doc = build_doc(repo);
        let changed: Vec<SourceWork> = changed_sources(&doc.sources, |source| {
            cache::get_meta(&sig_key(&doc.host, &doc.slug, source))
        })
        .into_iter()
        .map(|(s, t, sig)| (s.to_string(), t.to_string(), sig))
        .collect();
        if !changed.is_empty() {
            work.push((doc, changed));
        }
    }

    let mut embedded = 0usize;
    for (i, (doc, sources)) in work.into_iter().enumerate() {
        if i > 0 && i.is_multiple_of(REPOS_PER_BURST) {
            tokio::time::sleep(std::time::Duration::from_millis(BURST_PAUSE_MS)).await;
        }
        for (source, text, sig) in sources {
            let chunks = chunk_text(&text);
            let Ok(vectors) = embed_chunks(&model, &chunks).await else {
                // Backend went away mid-run — unfinished sources keep their old
                // signatures, so the next pass picks up exactly here.
                return embedded;
            };
            if store_embeddings(&doc.host, &doc.slug, &source, &vectors, now).is_ok() {
                cache::set_meta(&sig_key(&doc.host, &doc.slug, &source), &sig);
            }
        }
        embedded += 1;
    }
    embedded
}

/// Embed a source's chunks, [`BATCH`] at a time. Any failure fails the source
/// (partial vectors must not be stored — ranking needs the full source).
async fn embed_chunks(model: &str, chunks: &[String]) -> Result<Vec<(String, Vec<f32>)>, String> {
    let mut out = Vec::with_capacity(chunks.len());
    for group in chunks.chunks(BATCH) {
        let vecs = futures_util::future::join_all(group.iter().map(|c| ai::embed(model, c))).await;
        for (chunk, vec) in group.iter().zip(vecs) {
            out.push((chunk.clone(), vec?));
        }
    }
    Ok(out)
}

// ── Maintenance ─────────────────────────────────────────────────────────────

/// Drop rows + signatures left by the pre-#186 single-vector-per-repo pipeline
/// (source 'repo' under an empty host; sig keys without unit separators), so
/// they never pollute corpus recall. Idempotent, cheap.
pub fn purge_legacy_on(conn: &Connection) -> rusqlite::Result<usize> {
    let rows = conn.execute(
        "DELETE FROM embeddings WHERE host = '' AND source = 'repo'",
        [],
    )?;
    conn.execute(
        "DELETE FROM meta WHERE key LIKE 'embed_sig:%' AND key NOT LIKE '%' || char(31) || '%'",
        [],
    )?;
    Ok(rows)
}

/// See [`purge_legacy_on`].
pub fn purge_legacy() -> Result<usize, String> {
    let conn = cache::open()?;
    purge_legacy_on(&conn).map_err(|e| e.to_string())
}

/// Empty the whole index and its skip signatures, so the next pass re-embeds
/// everything (Settings "Rebuild index"). Returns the chunks removed.
pub fn clear_index_on(conn: &Connection) -> rusqlite::Result<usize> {
    let rows = conn.execute("DELETE FROM embeddings", [])?;
    conn.execute("DELETE FROM meta WHERE key LIKE 'embed_sig:%'", [])?;
    Ok(rows)
}

/// See [`clear_index_on`].
pub fn clear_index() -> Result<usize, String> {
    let conn = cache::open()?;
    clear_index_on(&conn).map_err(|e| e.to_string())
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
    fn chunking_packs_paragraphs_and_respects_headings() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("  \n\n \n").is_empty());

        // Short paragraphs pack into one chunk.
        let chunks = chunk_text("first para\nstill first\n\nsecond para");
        assert_eq!(chunks, vec!["first para\nstill first\n\nsecond para"]);

        // A heading stays glued to its following paragraph...
        let chunks = chunk_text("# Title\nintro text\n\n## Usage\nrun it");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("# Title\nintro text"));
        assert!(chunks[0].contains("## Usage\nrun it"));

        // ...and once past the target, a new heading starts a new chunk.
        let para = "x".repeat(CHUNK_TARGET_CHARS - 10);
        let text = format!("# One\n{para}\n\n# Two\nsecond section");
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].starts_with("# One"));
        assert_eq!(chunks[1], "# Two\nsecond section");
    }

    #[test]
    fn chunking_hard_splits_oversized_blocks_and_caps_count() {
        // One giant unbroken paragraph must split under the hard cap.
        let words = "word ".repeat(1000); // ~5000 chars, no blank lines
        let chunks = chunk_text(&words);
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_MAX_CHARS);
            assert!(!c.is_empty());
        }
        // Splits land at whitespace, so no word is torn apart.
        assert!(chunks.iter().all(|c| c.starts_with("word")));

        // A pathological readme is capped, not embedded forever.
        let huge = "para\n\n".repeat(10_000);
        assert_eq!(chunk_text(&huge).len(), MAX_CHUNKS_PER_SOURCE);
    }

    #[test]
    fn changed_sources_skips_unchanged_and_absent() {
        let sources = vec![
            ("readme".to_string(), "# Hello".to_string()),
            ("notes".to_string(), "remember the milk".to_string()),
            ("topics".to_string(), "".to_string()), // never stored, empty
        ];
        // Nothing stored yet → both non-empty sources change; empty 'topics'
        // is skipped (no rows to write).
        let changed = changed_sources(&sources, |_| None);
        assert_eq!(
            changed.iter().map(|(s, ..)| *s).collect::<Vec<_>>(),
            vec!["readme", "notes"]
        );

        // readme signature matches → only notes changes.
        let readme_sig = text_signature("# Hello");
        let changed = changed_sources(&sources, |s| (s == "readme").then(|| readme_sig.clone()));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, "notes");
        assert_eq!(changed[0].2, text_signature("remember the milk"));

        // A cleared source that *was* stored is returned, so its rows drop.
        let cleared = vec![("notes".to_string(), "".to_string())];
        let changed = changed_sources(&cleared, |_| Some("oldsig".into()));
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1, "");
        // ...and once the empty text's signature is stored, it stops changing.
        let empty_sig = text_signature("");
        assert!(changed_sources(&cleared, |_| Some(empty_sig.clone())).is_empty());
    }

    #[test]
    fn recall_groups_best_chunk_per_repo_and_ranks() {
        let rows = vec![
            row("github.com", "o/a", "readme", 0, &[0.6, 0.8]), // a: 0.6
            row("github.com", "o/a", "notes", 0, &[1.0, 0.0]),  // a: 1.0 ← best
            row("github.com", "o/b", "readme", 0, &[1.0, 1.0]), // b: ≈0.707
            row("github.com", "o/c", "readme", 0, &[0.0, 1.0]), // c: 0 — floored
        ];
        let hits = recall(&[1.0, 0.0], &rows, 8, 0.35);
        assert_eq!(hits.len(), 2, "one hit per repo, floor applied");
        assert_eq!(hits[0].slug, "o/a");
        assert_eq!(hits[0].source, "notes", "best chunk wins the repo");
        assert_eq!(hits[0].content, "o/a notes 0");
        assert_eq!(hits[1].slug, "o/b");
        assert!(hits[0].score > hits[1].score);

        // k truncates repos, not chunks.
        assert_eq!(recall(&[1.0, 0.0], &rows, 1, 0.35).len(), 1);
        assert!(recall(&[1.0, 0.0], &[], 8, 0.35).is_empty());
    }

    #[test]
    fn repo_key_prefers_host_slug_and_falls_back_to_path() {
        use crate::model::{Activity, GitStatus};
        let mut repo = Repo {
            id: "/home/u/dev/orrery".into(),
            display_name: "Orrery".into(),
            slug: Some("o/orrery".into()),
            path: "~/dev/orrery".into(),
            description: None,
            language: None,
            git: GitStatus::default(),
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "~/dev".into(),
            host: None,
            remote_host: Some("github.com".into()),
            stars: 0,
            topics: vec![],
            open_issues: 0,
            latest_release: None,
            private: false,
            favorite: false,
            ai_summary: None,
        };
        assert_eq!(
            repo_key(&repo),
            ("github.com".to_string(), "o/orrery".to_string())
        );
        repo.slug = None;
        // No remote → keyed by path under the empty host (can't collide with
        // a real slug pair).
        assert_eq!(
            repo_key(&repo),
            (String::new(), "/home/u/dev/orrery".to_string())
        );
    }

    #[test]
    fn purge_legacy_drops_only_legacy_rows_and_sigs() {
        let mut conn = mem();
        // A legacy row (empty host, source 'repo') and a real corpus row.
        store_embeddings_on(&mut conn, "", "/x/repo", "repo", &chunks(&[&[1.0]]), 1).unwrap();
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[1.0]]),
            1,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('embed_sig:/x/repo', 'old'),
             (?1, 'new'), ('other_key', 'kept')",
            [sig_key("github.com", "o/test", "readme")],
        )
        .unwrap();

        assert_eq!(purge_legacy_on(&conn).unwrap(), 1);
        let rows = all_embeddings_on(&conn);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "readme");
        let keys: Vec<String> = {
            let mut stmt = conn.prepare("SELECT key FROM meta ORDER BY key").unwrap();
            let iter = stmt.query_map([], |r| r.get(0)).unwrap();
            iter.flatten().collect()
        };
        assert!(keys.contains(&"other_key".to_string()));
        assert!(keys.contains(&sig_key("github.com", "o/test", "readme")));
        assert!(!keys.contains(&"embed_sig:/x/repo".to_string()));
    }

    #[test]
    fn clear_index_empties_rows_and_signatures() {
        let mut conn = mem();
        store_embeddings_on(
            &mut conn,
            "github.com",
            "o/test",
            "readme",
            &chunks(&[&[1.0], &[0.5]]),
            1,
        )
        .unwrap();
        cache_sig(&conn, &sig_key("github.com", "o/test", "readme"));
        assert_eq!(clear_index_on(&conn).unwrap(), 2);
        assert!(all_embeddings_on(&conn).is_empty());
        let sigs: i64 = conn
            .query_row(
                "SELECT count(*) FROM meta WHERE key LIKE 'embed_sig:%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sigs, 0);
    }

    fn cache_sig(conn: &Connection, key: &str) {
        conn.execute("INSERT INTO meta (key, value) VALUES (?1, 'sig')", [key])
            .unwrap();
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
