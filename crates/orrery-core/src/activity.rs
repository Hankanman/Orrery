//! Daily commit counts across the scanned repos, for the Mission Control
//! contribution heatmap ("N commits in the last year").
//!
//! A `git2` revwalk over every repo's branch tips counts each commit once
//! (deduped across branches), bucketed by the commit's own-timezone calendar
//! date. The window is the 53-week grid GitHub shows, ending on today.

use std::collections::HashMap;

use chrono::{Datelike, Duration, NaiveDate};
use git2::{Repository, Sort};

/// Week columns in the heatmap (GitHub shows 53).
pub const WEEKS: usize = 53;
/// Cells in the grid (53 weeks × 7 weekdays).
pub const CELLS: usize = WEEKS * 7;

/// The contribution heatmap: a 53-week × 7-day grid ending in the week of
/// `start + (WEEKS-1) weeks`, which contains today.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// Total commits counted across the window.
    pub total: u32,
    /// First day of the grid — always a Sunday.
    pub start: NaiveDate,
    /// `CELLS` cells, column-major (`week * 7 + weekday`, weekday 0 = Sunday):
    /// the day's commit count, or `None` for days after today in the trailing
    /// partial week.
    pub cells: Vec<Option<u32>>,
    /// Largest single-day count (drives color bucketing / the legend).
    pub max: u32,
}

/// The grid's first day (a Sunday) for a window ending in `today`'s week.
fn grid_start(today: NaiveDate) -> NaiveDate {
    let weekday = today.weekday().num_days_from_sunday() as i64; // 0 = Sunday
    today - Duration::days(weekday + ((WEEKS - 1) * 7) as i64)
}

/// Build the grid from `today` and an iterator of commit dates. Pure (no git or
/// I/O), so it's unit-testable; [`compute`] supplies the real commit dates.
pub fn tally(today: NaiveDate, dates: impl IntoIterator<Item = NaiveDate>) -> Activity {
    let start = grid_start(today);

    let mut counts: HashMap<NaiveDate, u32> = HashMap::new();
    for d in dates {
        if d >= start && d <= today {
            *counts.entry(d).or_default() += 1;
        }
    }

    let mut cells = vec![None; CELLS];
    let (mut total, mut max) = (0u32, 0u32);
    for (i, cell) in cells.iter_mut().enumerate() {
        let date = start + Duration::days(i as i64);
        if date > today {
            continue; // trailing future days stay None
        }
        let c = counts.get(&date).copied().unwrap_or(0);
        *cell = Some(c);
        total += c;
        max = max.max(c);
    }
    Activity {
        total,
        start,
        cells,
        max,
    }
}

/// Compute the heatmap by walking every repo's history. `repo_paths` are absolute
/// repo roots. Commits within the window are counted once each; older history is
/// skipped (the walk is time-sorted and stops past the window).
pub fn compute(repo_paths: &[String]) -> Activity {
    let today = chrono::Local::now().date_naive();
    let cutoff = grid_start(today)
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0);

    let mut dates: Vec<NaiveDate> = Vec::new();
    for path in repo_paths {
        if let Ok(repo) = Repository::open(path) {
            collect_dates(&repo, cutoff, &mut dates);
        }
    }
    tally(today, dates)
}

/// Push every local branch tip onto a time-sorted revwalk and collect the
/// calendar date of each commit newer than `cutoff_secs`. Time-sorted, so once a
/// commit predates the cutoff every remaining commit does too — we stop there.
fn collect_dates(repo: &Repository, cutoff_secs: i64, out: &mut Vec<NaiveDate>) {
    let Ok(mut walk) = repo.revwalk() else {
        return;
    };
    if walk.set_sorting(Sort::TIME).is_err() {
        return;
    }
    // All branch tips (the revwalk dedupes shared history); fall back to HEAD.
    if walk.push_glob("refs/heads/*").is_err() {
        let _ = walk.push_head();
    }
    for oid in walk.flatten() {
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let t = commit.time();
        if t.seconds() < cutoff_secs {
            break;
        }
        // Bucket by the commit's own local calendar date (apply its tz offset).
        let local = t.seconds() + (t.offset_minutes() as i64) * 60;
        if let Some(dt) = chrono::DateTime::from_timestamp(local, 0) {
            out.push(dt.date_naive());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn tally_buckets_counts_and_windows() {
        // Wednesday 2026-06-24.
        let today = d("2026-06-24");
        let start = grid_start(today);
        assert_eq!(
            start.weekday().num_days_from_sunday(),
            0,
            "start is a Sunday"
        );

        let dates = vec![
            today,
            today,                     // two commits today
            d("2026-06-23"),           // one yesterday
            start,                     // oldest in-window day
            start - Duration::days(1), // just before the window → ignored
            d("2030-01-01"),           // future → ignored
        ];
        let a = tally(today, dates);

        assert_eq!(a.total, 4, "in-window commits only");
        assert_eq!(a.max, 2, "busiest day had two");
        assert_eq!(a.cells.len(), CELLS);

        // today sits in the final column; future days in that week are None.
        let today_idx = (today - start).num_days() as usize;
        assert_eq!(a.cells[today_idx], Some(2));
        assert_eq!(a.cells[0], Some(1), "the window's first day");
        // A day after today in the trailing partial week stays None.
        if today_idx + 1 < CELLS {
            assert_eq!(a.cells[today_idx + 1], None);
        }
    }

    #[test]
    fn empty_history_is_all_zero() {
        let today = d("2026-06-24");
        let a = tally(today, []);
        assert_eq!((a.total, a.max), (0, 0));
        // Every past day is Some(0); only trailing future days are None.
        assert_eq!(a.cells[0], Some(0));
    }
}
