//! Cross-repo content search, powered by ripgrep (`rg`). We shell out to rg
//! rather than reimplement it: it's fast, respects .gitignore, and is almost
//! always present on a dev machine. Results are capped so the palette stays
//! responsive across hundreds of repos.

use std::process::Command;

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// Repo path that contains the match (one of the input `paths`).
    pub repo: String,
    /// File path relative to `repo` (for display).
    pub file: String,
    /// Absolute file path (for launching in an editor).
    pub abs: String,
    pub line: u32,
    /// The matching line's text (trimmed).
    pub text: String,
}

/// Search `query` across `paths` (repo roots), returning up to `limit` hits.
pub fn search(query: &str, paths: &[String], limit: usize) -> Result<Vec<SearchHit>, String> {
    let query = query.trim();
    if query.is_empty() || paths.is_empty() {
        return Ok(Vec::new());
    }
    let rg = which::which("rg").map_err(|_| "ripgrep (rg) not found on PATH".to_string())?;

    let mut cmd = Command::new(rg);
    cmd.arg("--json")
        .arg("--smart-case")
        .arg("--max-count")
        .arg("20") // per-file cap
        .arg("--max-columns")
        .arg("300")
        .arg("-e")
        .arg(query);
    for p in paths {
        cmd.arg(p);
    }

    let out = cmd.output().map_err(|e| e.to_string())?;
    // rg exits 1 when there are simply no matches — not an error for us.
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut hits = Vec::new();
    for line in stdout.lines() {
        if hits.len() >= limit {
            break;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("match") {
            continue;
        }
        let data = &v["data"];
        // rg encodes non-UTF8 paths/lines as { "bytes": ... }; we only handle text.
        let Some(abs) = data["path"]["text"].as_str() else { continue };
        let text = data["lines"]["text"].as_str().unwrap_or("").trim_end().to_string();
        let line_no = data["line_number"].as_u64().unwrap_or(0) as u32;

        // Attribute the hit to the longest matching repo prefix.
        let repo = paths
            .iter()
            .filter(|p| abs.starts_with(p.as_str()))
            .max_by_key(|p| p.len())
            .cloned()
            .unwrap_or_default();
        let file = abs.strip_prefix(&repo).map(|s| s.trim_start_matches('/').to_string()).unwrap_or_else(|| abs.to_string());

        hits.push(SearchHit { repo, file, abs: abs.to_string(), line: line_no, text });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_a_match_and_attributes_it_to_the_repo() {
        if which::which("rg").is_err() {
            return; // rg not installed in this environment — skip
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("foo.txt"), "alpha\nthe needle is here\nbeta\n").unwrap();
        let root = dir.path().to_string_lossy().into_owned();

        let hits = search("needle", &[root.clone()], 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file, "foo.txt");
        assert_eq!(hits[0].repo, root);
        assert_eq!(hits[0].line, 2);
        assert!(hits[0].text.contains("needle"));
    }

    #[test]
    fn empty_query_or_no_paths_returns_empty() {
        assert!(search("", &["/tmp".into()], 10).unwrap().is_empty());
        assert!(search("x", &[], 10).unwrap().is_empty());
    }
}
