//! Bridge from `orrery-core` (`cache` / `model`) to a flat, render-ready `Row`.
//! The card reads `Row` and never touches the core's serde types directly.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::SharedString;
use orrery_core::{cache, config, model, scan};

/// Everything the grid card renders, flattened from `model::Repo`.
#[derive(Clone)]
pub struct Row {
    pub id: SharedString,  // absolute path — launch cwd + favorite/cache key
    pub url: SharedString, // host web URL, or "" (open-on-host button)
    pub name: SharedString,
    pub slug: SharedString, // "owner/repo" or "no remote"
    pub root: SharedString, // the scanned root this repo was found under
    pub path: SharedString,
    pub description: SharedString,
    pub language: SharedString, // "" when unknown
    pub branch: SharedString,
    pub age: SharedString, // e.g. "3d ago"
    pub release: SharedString,
    pub ai_summary: SharedString,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub stars: SharedString, // pre-formatted (e.g. "1.2k")
    pub host: SharedString,  // "github" / "gitlab" / "" (brand-icon name)
    pub private: bool,
    pub favorite: bool,
    /// Activity bucket (active/idle/stale) — drives the "Stale" filter.
    pub activity: model::Activity,
    /// Last-commit time (Unix secs) — sort key for the "Activity" sort.
    pub last_commit_unix: i64,
}

pub(crate) fn rel_age(last_commit_unix: i64, now: i64) -> String {
    if last_commit_unix <= 0 {
        return "—".into();
    }
    let secs = (now - last_commit_unix).max(0);
    let days = secs / 86_400;
    if days >= 365 {
        format!("{}y ago", days / 365)
    } else if days >= 1 {
        format!("{days}d ago")
    } else {
        let hours = secs / 3_600;
        if hours >= 1 {
            format!("{hours}h ago")
        } else {
            let mins = (secs / 60).max(1);
            format!("{mins}m ago")
        }
    }
}

/// Human-readable byte size ("3.8 GB", "512 MB") for model listings.
pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Mirror of `formatStars` in the frontend: 1234 → "1.2k".
fn fmt_stars(stars: u32) -> String {
    if stars >= 1000 {
        format!("{:.1}k", stars as f64 / 1000.0)
    } else {
        stars.to_string()
    }
}

/// Flatten any newlines/tabs to single spaces. GPUI's single-line text elements
/// panic on embedded newlines, and our card/drawer render these as one line.
pub(crate) fn oneline(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        let c = if ch == '\n' || ch == '\r' || ch == '\t' {
            ' '
        } else {
            ch
        };
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Reflow soft-wrapped Markdown before handing it to gpui-component's renderer.
///
/// That renderer keeps a paragraph's source line breaks as literal `\n` inline
/// runs; its intrinsic-width layout pass then shapes a whole run as one line and
/// gpui panics ("text argument should not contain newlines"). So any hard-wrapped
/// paragraph in a README crashes the app. We join soft-wrapped continuation lines
/// of paragraphs / list items / block quotes onto a single line (the `\n` becomes
/// a space), while leaving block structure intact: blank lines, fenced code,
/// headings, list markers, block quotes, tables, and thematic breaks all stay on
/// their own lines.
pub(crate) fn unwrap_soft_breaks(src: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Heading,
        Hr,
        Quote,
        Table,
        List,
        Code,
        Para,
    }

    /// A run of ≥3 of the same `-`/`*`/`_` (a thematic break).
    fn is_hr(t: &str) -> bool {
        let s: String = t.chars().filter(|c| !c.is_whitespace()).collect();
        s.len() >= 3
            && (s.bytes().all(|b| b == b'-')
                || s.bytes().all(|b| b == b'*')
                || s.bytes().all(|b| b == b'_'))
    }

    /// A list item marker (`- `, `* `, `+ `, or `1.` / `1)`).
    fn is_list(t: &str) -> bool {
        if matches!(t.get(..2), Some("- " | "* " | "+ ")) || matches!(t, "-" | "*" | "+") {
            return true;
        }
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && matches!(t[digits..].get(..2), Some(". " | ") "))
    }

    /// Does this line begin a new block construct (so it can't be folded into an
    /// open paragraph)?
    fn starts_block(t: &str) -> bool {
        t.starts_with('#')
            || t.starts_with('>')
            || t.starts_with("```")
            || t.starts_with("~~~")
            || t.contains('|')
            || is_hr(t)
            || is_list(t)
    }

    fn classify(raw: &str, t: &str) -> Kind {
        if is_hr(t) {
            Kind::Hr
        } else if t.starts_with('#') {
            Kind::Heading
        } else if t.starts_with('>') {
            Kind::Quote
        } else if t.contains('|') {
            Kind::Table
        } else if is_list(t) {
            Kind::List
        } else if raw.starts_with("    ") || raw.starts_with('\t') {
            Kind::Code
        } else {
            Kind::Para
        }
    }

    fn push_line(out: &mut String, line: &str) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }

    fn append_word(out: &mut String, text: &str) {
        if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
        out.push_str(text);
    }

    let mut out = String::with_capacity(src.len());
    let mut fence: Option<String> = None;
    // Kind of the logical line currently open for continuation (None after a
    // blank line / fence / at the start).
    let mut open: Option<Kind> = None;

    for raw in src.split('\n') {
        let t = raw.trim_start();

        // Inside a fenced code block: copy verbatim until the closing fence.
        if let Some(f) = &fence {
            push_line(&mut out, raw);
            if t.starts_with(f.as_str()) {
                fence = None;
            }
            open = None;
            continue;
        }
        if t.starts_with("```") || t.starts_with("~~~") {
            push_line(&mut out, raw);
            fence = Some(t[..3].to_string());
            open = None;
            continue;
        }
        if t.is_empty() {
            push_line(&mut out, "");
            open = None;
            continue;
        }

        // Fold a lazy continuation onto the open paragraph / list item.
        if matches!(open, Some(Kind::Para | Kind::List)) && !starts_block(t) {
            append_word(&mut out, t);
            continue;
        }
        // Fold continued / lazily-continued block-quote lines (strip the marker).
        if open == Some(Kind::Quote) && (t.starts_with('>') || !starts_block(t)) {
            append_word(&mut out, t.trim_start_matches('>').trim_start());
            continue;
        }

        push_line(&mut out, raw);
        open = Some(classify(raw, t));
    }

    out
}

pub fn to_rows(repos: Vec<model::Repo>, now: i64) -> Vec<Row> {
    repos
        .into_iter()
        .map(|r| Row {
            id: r.id.into(),
            url: match (r.remote_host.as_deref(), r.slug.as_deref()) {
                (Some(host), Some(slug)) => format!("https://{host}/{slug}"),
                _ => String::new(),
            }
            .into(),
            name: oneline(r.display_name).into(),
            slug: r.slug.unwrap_or_else(|| "no remote".into()).into(),
            root: r.root.into(),
            path: r.path.into(),
            description: oneline(
                r.description
                    .filter(|d| !d.trim().is_empty())
                    .unwrap_or_else(|| "No README description.".into()),
            )
            .into(),
            language: r.language.unwrap_or_default().into(),
            branch: r.git.branch.into(),
            age: rel_age(r.last_commit_unix, now).into(),
            release: oneline(r.latest_release.unwrap_or_default()).into(),
            ai_summary: oneline(r.ai_summary.unwrap_or_default()).into(),
            ahead: r.git.ahead,
            behind: r.git.behind,
            dirty: r.git.dirty,
            stars: fmt_stars(r.stars).into(),
            host: match r.host {
                Some(model::Host::Github) => "github",
                Some(model::Host::Gitlab) => "gitlab",
                None => "",
            }
            .into(),
            private: r.private,
            favorite: r.favorite,
            activity: r.activity,
            last_commit_unix: r.last_commit_unix,
        })
        .collect()
}

/// Current Unix time in seconds (for relative ages); 0 if the clock is before
/// the epoch.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn count_roots(repos: &[model::Repo]) -> usize {
    repos
        .iter()
        .map(|r| r.root.as_str())
        .collect::<HashSet<&str>>()
        .len()
}

/// Load real repos from the shipping SQLite cache. Returns the rows plus the
/// number of distinct scanned roots (for the header's "N roots · M repos").
pub fn load(now: i64) -> (Vec<Row>, usize) {
    let mut repos = cache::load_repos();
    // Overlay persisted host enrichment (stars/visibility/release) so the launch
    // paint — and the reload after an enrich pass — show it without a rescan.
    cache::apply_host_info(&mut repos);
    let n_roots = count_roots(&repos);
    (to_rows(repos, now), n_roots)
}

/// Re-scan the configured roots from disk (git-heavy — call off the UI thread),
/// refresh the cache, and return render-ready rows + root count. Mirrors the
/// Tauri `scan_repos` command: the filesystem watcher triggers this so the grid
/// reflects on-disk changes live.
pub fn rescan() -> (Vec<Row>, usize) {
    let now = now_unix();
    let cfg = config::load();
    let favorites = cache::favorites();
    let mut repos = scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &favorites, now);
    // Carry over persisted host enrichment (stars/visibility) until a fresh
    // enrich pass re-confirms it, then snapshot for instant next-launch paint.
    cache::apply_host_info(&mut repos);
    let _ = cache::store_repos(&repos);
    let n_roots = count_roots(&repos);
    (to_rows(repos, now), n_roots)
}

#[cfg(test)]
mod tests {
    use super::unwrap_soft_breaks;

    #[test]
    fn folds_hard_wrapped_paragraph() {
        let md = "This is a paragraph that is\nwrapped across\nmultiple source lines.";
        assert_eq!(
            unwrap_soft_breaks(md),
            "This is a paragraph that is wrapped across multiple source lines."
        );
        assert!(!unwrap_soft_breaks(md).contains('\n'));
    }

    #[test]
    fn keeps_paragraph_breaks() {
        let md = "Para one\nline two.\n\nPara two\nline two.";
        assert_eq!(
            unwrap_soft_breaks(md),
            "Para one line two.\n\nPara two line two."
        );
    }

    #[test]
    fn preserves_fenced_code() {
        let md = "Intro line\nmore intro.\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n\nAfter.";
        assert_eq!(
            unwrap_soft_breaks(md),
            "Intro line more intro.\n\n```rust\nlet a = 1;\nlet b = 2;\n```\n\nAfter."
        );
    }

    #[test]
    fn does_not_merge_heading_into_text() {
        let md = "# Title\nFirst paragraph\nwrapped.";
        assert_eq!(unwrap_soft_breaks(md), "# Title\nFirst paragraph wrapped.");
    }

    #[test]
    fn folds_list_item_continuations_not_items() {
        let md = "- item one that is\n  wrapped here\n- item two\n- item three";
        assert_eq!(
            unwrap_soft_breaks(md),
            "- item one that is wrapped here\n- item two\n- item three"
        );
    }

    #[test]
    fn folds_block_quote_lines() {
        let md = "> quoted line one\n> quoted line two";
        assert_eq!(unwrap_soft_breaks(md), "> quoted line one quoted line two");
    }

    #[test]
    fn keeps_table_rows_separate() {
        let md = "| a | b |\n| - | - |\n| 1 | 2 |";
        assert_eq!(unwrap_soft_breaks(md), "| a | b |\n| - | - |\n| 1 | 2 |");
    }
}
