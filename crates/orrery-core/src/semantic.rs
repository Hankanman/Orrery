//! Semantic search over a local embedding index (#41).
//!
//! Each repo is embedded (name/slug/language/description) via the configured
//! embedding model and the vector is cached in SQLite. A query is embedded the
//! same way and ranked against the index by cosine similarity. Indexing skips
//! repos whose text signature is unchanged, so a rescan/file-watch refresh costs
//! cheap `meta` lookups rather than N embedding calls.

use crate::{ai, cache, config};

/// Minimum cosine similarity for a query↔repo match to surface.
const MIN_SCORE: f32 = 0.35;
/// Max ranked hits returned for a query.
const MAX_HITS: usize = 8;
/// How many repos to embed concurrently per batch.
const BATCH: usize = 6;

/// Embed each `(id, text)` whose text changed since the last index, caching the
/// vector + a signature so unchanged repos are skipped. Returns how many were
/// (re-)embedded. Embedding failures (AI unreachable) are swallowed — the index
/// just stays as-is.
pub async fn index(items: &[(String, String)]) -> usize {
    let model = config::load().embed_model;
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
                        cache::store_embedding(id, &vec);
                        cache::set_meta(&key, &sig);
                        true
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
    let mut hits: Vec<(String, f32)> = cache::load_embeddings()
        .into_iter()
        .map(|(id, v)| (id, ai::cosine(&q, &v)))
        .filter(|(_, s)| *s > MIN_SCORE)
        .collect();
    hits.sort_by(|a, b| b.1.total_cmp(&a.1));
    hits.truncate(MAX_HITS);
    hits
}

/// Stable hex fingerprint of a repo's embedding text, for skip-if-unchanged.
fn text_signature(text: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    format!("{:x}", h.finish())
}
