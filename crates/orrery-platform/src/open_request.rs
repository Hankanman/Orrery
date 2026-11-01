//! The `orrery open <query>` command line and the `orrery://` URI scheme:
//! parse what the user (or a web page) asked for, then resolve it against the
//! cached repo list into an [`AppRequest`] for the app.
//!
//! A URI is untrusted input: any web page can link to `orrery://…` and the
//! browser hands it to us. So the URI grammar is deliberately tiny and strict:
//!
//! - `orrery://open/<owner>/<repo>` (GitLab subgroups: more `/segments`), and
//! - `orrery://open?path=<absolute path>`.
//!
//! Anything else is refused. Even a well-formed URI only ever *focuses a repo
//! that's already in the scanned list*: it never runs a command, never opens a
//! path that isn't a known repo, and never touches the filesystem while
//! parsing.

use std::path::Path;

use orrery_core::model::Repo;

use crate::repo_search;
use crate::request::AppRequest;

/// The URI scheme the desktop entry registers (`x-scheme-handler/orrery`).
pub const SCHEME: &str = "orrery";

/// Longest URI accepted. Real ones are a slug or a path; anything longer is
/// junk or an attack.
const MAX_URI_LEN: usize = 2048;

/// Most `/`-separated segments in a slug (GitLab nests groups; a few levels
/// is plenty).
const MAX_SLUG_SEGMENTS: usize = 8;

/// Longest search query passed on to the palette, in characters.
pub const MAX_QUERY_CHARS: usize = 200;

/// What the user asked to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    /// Free text (`orrery open <query>`): the best-matching repo.
    Query(String),
    /// A host slug (`owner/repo`, or `group/sub/repo`), matched exactly.
    Slug(String),
    /// An absolute path, matched exactly against the scanned repo ids.
    Path(String),
}

/// Parse the process arguments (without the program name). `Ok(None)` for a
/// plain launch, including arguments this parser doesn't own (so other flags
/// pass through untouched); `Err` for a malformed `open` request.
pub fn parse_args(args: &[String]) -> Result<Option<OpenTarget>, String> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    // The desktop entry runs `orrery %u`, so a clicked link arrives alone.
    if has_scheme(first) {
        return parse_uri(first).map(Some);
    }
    if first != "open" {
        return Ok(None);
    }
    let rest = &args[1..];
    if let [only] = rest {
        if has_scheme(only) {
            return parse_uri(only).map(Some);
        }
    }
    let query = clean_query(&rest.join(" ")).ok_or("usage: orrery open <query>")?;
    Ok(Some(OpenTarget::Query(query)))
}

/// Whether `s` starts with the `orrery:` scheme (any case).
fn has_scheme(s: &str) -> bool {
    s.get(..SCHEME.len() + 1)
        .is_some_and(|p| p.eq_ignore_ascii_case("orrery:"))
}

/// Parse an `orrery://` URI strictly (see the module docs for the grammar).
pub fn parse_uri(uri: &str) -> Result<OpenTarget, String> {
    if uri.len() > MAX_URI_LEN {
        return Err("URI too long".into());
    }
    if uri.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("URI contains whitespace or control characters".into());
    }
    if !has_scheme(uri) {
        return Err("not an orrery: URI".into());
    }
    let rest = &uri[SCHEME.len() + 1..];
    // Only the `open` authority, written with `//`. No user info, port or
    // other hosts (`orrery://evil@open`, `orrery://open:80` both fail here).
    let Some(rest) = rest.strip_prefix("//open") else {
        return Err("only orrery://open/… is supported".into());
    };
    if rest.contains('#') {
        return Err("fragments are not allowed".into());
    }
    match rest.split_once('?') {
        // orrery://open/<owner>/<repo>
        None => parse_slug(rest.strip_prefix('/').ok_or("expected /<owner>/<repo>")?),
        // orrery://open?path=<path> (also tolerate orrery://open/?path=…)
        Some(("" | "/", query)) => parse_path_query(query),
        Some(_) => Err("give either a /<owner>/<repo> slug or ?path=, not both".into()),
    }
}

/// `<owner>/<repo>[/…]`, each segment a plain host name segment.
fn parse_slug(raw: &str) -> Result<OpenTarget, String> {
    let raw = raw.strip_suffix('/').unwrap_or(raw);
    let segments: Vec<String> = raw
        .split('/')
        .map(percent_decode)
        .collect::<Result<_, _>>()?;
    if segments.len() < 2 {
        return Err("expected /<owner>/<repo>".into());
    }
    if segments.len() > MAX_SLUG_SEGMENTS {
        return Err("too many slug segments".into());
    }
    for s in &segments {
        let ok = !s.is_empty()
            && s != "."
            && s != ".."
            && s.len() <= 100
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        if !ok {
            return Err(format!("invalid slug segment {s:?}"));
        }
    }
    Ok(OpenTarget::Slug(segments.join("/")))
}

/// `path=<absolute path>`: exactly one key, no extras.
fn parse_path_query(query: &str) -> Result<OpenTarget, String> {
    let mut path = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or("expected path=<path>")?;
        if key != "path" {
            return Err(format!("unknown parameter {key:?}"));
        }
        if path.is_some() {
            return Err("path given twice".into());
        }
        path = Some(percent_decode(value)?);
    }
    let path = path.ok_or("expected path=<path>")?;
    validate_path(&path)?;
    Ok(OpenTarget::Path(path))
}

/// An absolute, already-normal path with no `.`/`..` steps and no control
/// characters. Nothing is read from disk; the path only ever selects a repo
/// that's already known.
fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > 4096 {
        return Err("bad path length".into());
    }
    if path.chars().any(char::is_control) {
        return Err("path contains control characters".into());
    }
    if !Path::new(path).is_absolute() {
        return Err("path must be absolute".into());
    }
    // On the raw text: `Path::components` quietly drops inner `.` steps.
    if path.split('/').any(|seg| seg == "." || seg == "..") {
        return Err("path must not contain . or .. steps".into());
    }
    Ok(())
}

/// Decode `%XX` escapes (UTF-8). `+` is left as is: these are path/slug
/// components, not form data. Rejects malformed escapes, invalid UTF-8 and
/// decoded NULs / control characters.
fn percent_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or("truncated %-escape")?;
            // Two hex digits exactly (`from_str_radix` alone would take "+1").
            if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("bad %-escape".into());
            }
            let b = u8::from_str_radix(hex, 16).map_err(|_| "bad %-escape")?;
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).map_err(|_| "escape is not UTF-8")?;
    if decoded.chars().any(char::is_control) {
        return Err("escape decodes to a control character".into());
    }
    Ok(decoded)
}

/// Tidy a free-text query for matching / the palette: control characters
/// dropped, whitespace collapsed, capped at [`MAX_QUERY_CHARS`]. `None` when
/// nothing is left.
pub fn clean_query(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let joined: String = words
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_CHARS)
        .collect();
    let joined = joined.trim().to_string();
    (!joined.is_empty()).then_some(joined)
}

/// Resolve `target` against the scanned repos into the request the app
/// should act on. A query that matches no repo still opens the palette with
/// it; a slug or path that isn't a known repo is refused.
pub fn resolve(
    target: &OpenTarget,
    repos: &[Repo],
    activation_token: Option<String>,
) -> Result<AppRequest, String> {
    let repo = match target {
        OpenTarget::Query(q) => {
            let terms = repo_search::query_terms(q);
            match repo_search::search(repos, &terms, 1).first() {
                Some((r, _)) => *r,
                None => {
                    return Ok(AppRequest::Search {
                        query: q.clone(),
                        activation_token,
                    });
                }
            }
        }
        OpenTarget::Slug(slug) => {
            let mut hits: Vec<&Repo> = repos
                .iter()
                .filter(|r| {
                    r.slug
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(slug))
                })
                .collect();
            // A slug cloned twice (or a linked worktree): prefer the main
            // checkout, then a stable order.
            hits.sort_by(|a, b| {
                a.worktree_of
                    .is_some()
                    .cmp(&b.worktree_of.is_some())
                    .then_with(|| a.id.cmp(&b.id))
            });
            hits.into_iter()
                .next()
                .ok_or_else(|| format!("no scanned repo has the slug {slug}"))?
        }
        OpenTarget::Path(path) => {
            let path = path
                .strip_suffix('/')
                .filter(|p| !p.is_empty())
                .unwrap_or(path);
            repos
                .iter()
                .find(|r| r.id == path)
                .ok_or_else(|| format!("no scanned repo at {path}"))?
        }
    };
    Ok(AppRequest::ShowRepo {
        id: repo.id.clone(),
        activation_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orrery_core::model::{Activity, GitStatus};

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    fn repo(id: &str, name: &str, slug: Option<&str>) -> Repo {
        Repo {
            id: id.into(),
            display_name: name.into(),
            slug: slug.map(Into::into),
            path: id.into(),
            description: None,
            language: None,
            git: GitStatus::default(),
            last_commit_unix: 0,
            activity: Activity::Active,
            root: "/home/u/dev".into(),
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
    fn desktop_entry_registers_the_scheme_and_passes_the_link() {
        let desktop = include_str!("../../../packaging/orrery.desktop");
        assert!(desktop.contains(&format!("MimeType=x-scheme-handler/{SCHEME};\n")));
        assert!(desktop.contains("Exec=orrery %u\n"));
    }

    #[test]
    fn plain_launches_and_foreign_flags_pass_through() {
        assert_eq!(parse_args(&[]), Ok(None));
        assert_eq!(parse_args(&args(&["--shot", "settings"])), Ok(None));
        assert_eq!(parse_args(&args(&["opener"])), Ok(None));
    }

    #[test]
    fn open_takes_a_query_or_a_uri() {
        assert_eq!(
            parse_args(&args(&["open", "orr", "ery"])),
            Ok(Some(OpenTarget::Query("orr ery".into())))
        );
        assert_eq!(
            parse_args(&args(&["open", "orrery://open/Hankanman/Orrery"])),
            Ok(Some(OpenTarget::Slug("Hankanman/Orrery".into())))
        );
        // The desktop entry's `%u` passes the URI on its own.
        assert_eq!(
            parse_args(&args(&["ORRERY://open?path=/home/u/dev/x"])),
            Ok(Some(OpenTarget::Path("/home/u/dev/x".into())))
        );
        assert!(parse_args(&args(&["open"])).is_err());
        assert!(parse_args(&args(&["open", "  ", "\u{7}"])).is_err());
    }

    #[test]
    fn well_formed_uris() {
        assert_eq!(
            parse_uri("orrery://open/owner/repo"),
            Ok(OpenTarget::Slug("owner/repo".into()))
        );
        assert_eq!(
            parse_uri("orrery://open/group/sub/repo.rs/"),
            Ok(OpenTarget::Slug("group/sub/repo.rs".into()))
        );
        assert_eq!(
            parse_uri("orrery://open?path=%2Fhome%2Fu%2Fmy%20repo"),
            Ok(OpenTarget::Path("/home/u/my repo".into()))
        );
        assert_eq!(
            parse_uri("orrery://open/?path=/srv/code/x"),
            Ok(OpenTarget::Path("/srv/code/x".into()))
        );
    }

    #[test]
    fn hostile_uris_are_refused() {
        let bad = [
            // Wrong scheme / authority / verb.
            "https://open/owner/repo",
            "orrery:open/owner/repo",
            "orrery://run/owner/repo",
            "orrery://opener/owner/repo",
            "orrery://evil@open/owner/repo",
            "orrery://open:80/owner/repo",
            "orrery://open",
            "orrery://open/",
            "orrery://open/owner",
            // Traversal and odd segments.
            "orrery://open/../etc/passwd",
            "orrery://open/owner/..",
            "orrery://open/owner/%2e%2e",
            "orrery://open/owner/%2Fetc",
            "orrery://open/owner//repo",
            "orrery://open/own;er/repo",
            "orrery://open/owner/repo$(rm%20-rf)",
            "orrery://open/owner/re%60po",
            "orrery://open/a/b/c/d/e/f/g/h/i",
            // Paths: relative, traversal, tilde, NUL, controls, extra keys.
            "orrery://open?path=relative/x",
            "orrery://open?path=~/dev/x",
            "orrery://open?path=/home/u/../../etc",
            "orrery://open?path=/home/./u",
            "orrery://open?path=/home/u%00/x",
            "orrery://open?path=/home/u%0a/x",
            "orrery://open?path=",
            "orrery://open?path=/a&path=/b",
            "orrery://open?path=/a&cmd=rm",
            "orrery://open?cmd=/bin/sh",
            "orrery://open?path",
            "orrery://open/owner/repo?path=/a",
            "orrery://open/owner/repo#frag",
            // Bad escapes, whitespace, controls.
            "orrery://open/owner/re%zzpo",
            "orrery://open/owner/repo%",
            "orrery://open/owner/re%ffpo",
            "orrery://open/owner/re po",
            "orrery://open/owner/re\npo",
        ];
        for uri in bad {
            assert!(parse_uri(uri).is_err(), "accepted hostile URI {uri:?}");
        }
        let long = format!("orrery://open/owner/{}", "a".repeat(MAX_URI_LEN));
        assert!(parse_uri(&long).is_err());
    }

    #[test]
    fn queries_are_cleaned_and_capped() {
        assert_eq!(clean_query("  orr \t ery  "), Some("orr ery".into()));
        assert_eq!(clean_query("a\u{1b}[31mb"), Some("a[31mb".into()));
        assert_eq!(clean_query(" \n "), None);
        assert_eq!(
            clean_query(&"x".repeat(500)).unwrap().len(),
            MAX_QUERY_CHARS
        );
    }

    #[test]
    fn resolve_only_selects_known_repos() {
        let repos = [
            repo("/home/u/dev/orrery", "Orrery", Some("Hankanman/Orrery")),
            repo("/home/u/dev/zed", "zed", Some("zed-industries/zed")),
        ];
        let show = |id: &str| {
            Ok(AppRequest::ShowRepo {
                id: id.into(),
                activation_token: None,
            })
        };
        let r = |t: OpenTarget| resolve(&t, &repos, None);
        assert_eq!(
            r(OpenTarget::Query("orr".into())),
            show("/home/u/dev/orrery")
        );
        assert_eq!(
            r(OpenTarget::Query("nothing like it".into())),
            Ok(AppRequest::Search {
                query: "nothing like it".into(),
                activation_token: None
            })
        );
        assert_eq!(
            r(OpenTarget::Slug("hankanman/orrery".into())),
            show("/home/u/dev/orrery")
        );
        assert!(r(OpenTarget::Slug("someone/else".into())).is_err());
        assert_eq!(
            r(OpenTarget::Path("/home/u/dev/zed/".into())),
            show("/home/u/dev/zed")
        );
        // Not a repo root, even though it's inside one.
        assert!(r(OpenTarget::Path("/home/u/dev/zed/src".into())).is_err());
        assert!(r(OpenTarget::Path("/etc".into())).is_err());
    }

    #[test]
    fn a_slug_prefers_the_main_checkout() {
        let mut wt = repo("/a/orrery-wt", "Orrery", Some("o/orrery"));
        wt.worktree_of = Some("/z/orrery".into());
        let repos = [wt, repo("/z/orrery", "Orrery", Some("o/orrery"))];
        assert_eq!(
            resolve(&OpenTarget::Slug("o/orrery".into()), &repos, None),
            Ok(AppRequest::ShowRepo {
                id: "/z/orrery".into(),
                activation_token: None
            })
        );
    }
}
