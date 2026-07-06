//! Bulk ("fleet") execution engine (#184): run one operation across N repos
//! with bounded concurrency, per-repo result collection, progress reporting,
//! and cancellation. UI-free — the GPUI side marshals progress events onto the
//! foreground itself (the `live.rs` channel pattern); this engine only calls
//! the provided callback from worker threads.
//!
//! Uses the same idiom as `scan`: `thread::scope` workers pulling indices from
//! an atomic counter, results collected under a `Mutex` — no thread-pool
//! dependency, and results land in a fixed slot per input index so the report
//! is always in input order regardless of completion order.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::git_ops::{self, OpOutcome};

/// Per-repo outcome of a fleet operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The op ran to completion; the string is a short human-readable summary.
    Ok(String),
    /// The op deliberately did nothing — not applicable to this repo (e.g. no
    /// upstream), or the run was cancelled before this repo started.
    Skipped(String),
    /// The op was wanted but could not complete; the string says why.
    Failed(String),
}

/// One repo's result. `repo` is the repo id — its absolute path, exactly how
/// `scan` identifies repos (`Repo::id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoResult {
    pub repo: String,
    pub outcome: Outcome,
}

/// Progress event, fired once per repo completion — including cancellation
/// skips, so `done` always reaches `total` by the end of a run. Fired from a
/// worker thread; the UI side is responsible for marshalling to the foreground.
#[derive(Debug, Clone)]
pub struct FleetEvent {
    /// Index of this repo in the input slice.
    pub index: usize,
    /// Completions so far, including this one.
    pub done: usize,
    /// Total number of repos in the run.
    pub total: usize,
    pub result: RepoResult,
}

/// Aggregate result of a fleet run. `results` is in input order.
#[derive(Debug, Clone)]
pub struct FleetReport {
    pub results: Vec<RepoResult>,
    /// Ops actually invoked (repos skipped by cancellation are never started).
    pub started: usize,
    /// Ops that ran to completion (equals `started` unless a worker panicked).
    pub completed: usize,
    /// The cancel flag was set at some point during the run.
    pub cancelled: bool,
}

impl FleetReport {
    pub fn ok_count(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Ok(_)))
    }
    pub fn skipped_count(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Skipped(_)))
    }
    pub fn failed_count(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Failed(_)))
    }
    fn count(&self, pred: impl Fn(&Outcome) -> bool) -> usize {
        self.results.iter().filter(|r| pred(&r.outcome)).count()
    }
}

/// Default worker count: enough to hide network latency across a fleet without
/// hammering the disk or the remote — min(8, available cores).
pub fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
}

/// Run `op` across `repos` (repo ids, i.e. absolute paths) on up to `workers`
/// threads. Cancellation is checked *between* repos — an in-flight git op is
/// never interrupted; repos not yet started report `Skipped("cancelled")`.
/// `progress` fires once per completed repo (from a worker thread).
pub fn run<F>(
    repos: &[String],
    workers: usize,
    cancel: &AtomicBool,
    progress: impl Fn(FleetEvent) + Sync,
    op: F,
) -> FleetReport
where
    F: Fn(&str) -> Outcome + Sync,
{
    let total = repos.len();
    let workers = workers.max(1).min(total);
    // Fixed slot per input index → deterministic input-order results.
    let slots: Mutex<Vec<Option<RepoResult>>> = Mutex::new(vec![None; total]);
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let started = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);

    std::thread::scope(|scope| {
        let (slots, next, done, started, completed) = (&slots, &next, &done, &started, &completed);
        let (progress, op) = (&progress, &op);
        for _ in 0..workers {
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(repo) = repos.get(i) else {
                    break;
                };
                let outcome = if cancel.load(Ordering::SeqCst) {
                    Outcome::Skipped("cancelled".into())
                } else {
                    started.fetch_add(1, Ordering::SeqCst);
                    let out = op(repo);
                    completed.fetch_add(1, Ordering::SeqCst);
                    out
                };
                let result = RepoResult {
                    repo: repo.clone(),
                    outcome,
                };
                slots.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(result.clone());
                let done_now = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress(FleetEvent {
                    index: i,
                    done: done_now,
                    total,
                    result,
                });
            });
        }
    });

    FleetReport {
        results: slots
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .into_iter()
            .flatten()
            .collect(),
        started: started.into_inner(),
        completed: completed.into_inner(),
        cancelled: cancel.load(Ordering::SeqCst),
    }
}

/// Fleet fetch: `git_ops::fetch` per repo, summarising the refreshed status.
pub fn fetch_op() -> impl Fn(&str) -> Outcome + Sync {
    |path| match git_ops::fetch(path) {
        Ok(st) => Outcome::Ok(format!(
            "fetched — {} ahead {}, behind {}",
            st.branch, st.ahead, st.behind
        )),
        Err(e) => Outcome::Failed(e),
    }
}

/// Fleet pull, delegating to `git_ops::pull` (fast-forward only — never forces
/// or merges). Safety refusals where a pull was wanted but couldn't happen
/// (diverged history, dirty tree) surface as `Failed` so a fleet pull never
/// hides them; not-applicable states (no upstream, detached HEAD) stay skips.
pub fn pull_op() -> impl Fn(&str) -> Outcome + Sync {
    |path| match git_ops::pull(path) {
        Ok(OpOutcome::Done(s)) => Outcome::Ok(s),
        Ok(OpOutcome::Skipped(r)) if r == git_ops::SKIP_DIVERGED || r == git_ops::SKIP_DIRTY => {
            Outcome::Failed(r)
        }
        Ok(OpOutcome::Skipped(r)) => Outcome::Skipped(r),
        Err(e) => Outcome::Failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::time::Duration;

    fn repo_names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("repo-{i}")).collect()
    }

    #[test]
    fn results_keep_input_order_regardless_of_completion_order() {
        let repos = repo_names(8);
        let cancel = AtomicBool::new(false);
        let report = run(
            &repos,
            4,
            &cancel,
            |_| {},
            |repo| {
                // Later repos finish first: earlier ones sleep longer.
                let i: u64 = repo.rsplit('-').next().unwrap().parse().unwrap();
                std::thread::sleep(Duration::from_millis((8 - i) * 3));
                Outcome::Ok(repo.to_string())
            },
        );
        let order: Vec<&str> = report.results.iter().map(|r| r.repo.as_str()).collect();
        let expected: Vec<&str> = repos.iter().map(String::as_str).collect();
        assert_eq!(order, expected);
        assert!(!report.cancelled);
        assert_eq!(report.started, 8);
        assert_eq!(report.completed, 8);
        assert_eq!(report.ok_count(), 8);
    }

    #[test]
    fn concurrency_is_bounded_and_reaches_worker_count() {
        // 6 tasks over 2 workers; every op waits at a 2-party barrier, so both
        // workers must be in-flight together (max ≥ 2) and, with only 2 worker
        // threads, never more (max ≤ 2). Deterministic — no sleep needed.
        let repos = repo_names(6);
        let cancel = AtomicBool::new(false);
        let barrier = Barrier::new(2);
        let in_flight = AtomicUsize::new(0);
        let max_seen = AtomicUsize::new(0);
        run(
            &repos,
            2,
            &cancel,
            |_| {},
            |_| {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(now, Ordering::SeqCst);
                barrier.wait();
                in_flight.fetch_sub(1, Ordering::SeqCst);
                Outcome::Ok(String::new())
            },
        );
        assert_eq!(max_seen.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancellation_skips_remaining_repos() {
        let repos = repo_names(4);
        let cancel = AtomicBool::new(false);
        // One worker → deterministic order; the first op flips the flag, so
        // the remaining three must be skipped without being started.
        let report = run(
            &repos,
            1,
            &cancel,
            |_| {},
            |repo| {
                cancel.store(true, Ordering::SeqCst);
                Outcome::Ok(repo.to_string())
            },
        );
        assert!(report.cancelled);
        assert_eq!(report.started, 1);
        assert_eq!(report.completed, 1);
        assert_eq!(report.results.len(), 4);
        assert_eq!(report.results[0].outcome, Outcome::Ok("repo-0".into()));
        for r in &report.results[1..] {
            assert_eq!(r.outcome, Outcome::Skipped("cancelled".into()));
        }
    }

    #[test]
    fn mixed_outcomes_counted_and_progress_fires_per_repo() {
        let repos = vec!["ok".to_string(), "fail".to_string(), "skip".to_string()];
        let cancel = AtomicBool::new(false);
        let events: Mutex<Vec<FleetEvent>> = Mutex::new(Vec::new());
        let report = run(
            &repos,
            2,
            &cancel,
            |e| events.lock().unwrap().push(e),
            |repo| match repo {
                "ok" => Outcome::Ok("done".into()),
                "fail" => Outcome::Failed("boom".into()),
                _ => Outcome::Skipped("nah".into()),
            },
        );
        assert_eq!(report.results[0].outcome, Outcome::Ok("done".into()));
        assert_eq!(report.results[1].outcome, Outcome::Failed("boom".into()));
        assert_eq!(report.results[2].outcome, Outcome::Skipped("nah".into()));
        assert_eq!(
            (
                report.ok_count(),
                report.failed_count(),
                report.skipped_count()
            ),
            (1, 1, 1)
        );

        let events = events.into_inner().unwrap();
        assert_eq!(events.len(), 3, "one progress event per repo");
        assert!(events.iter().all(|e| e.total == 3));
        let mut dones: Vec<usize> = events.iter().map(|e| e.done).collect();
        dones.sort_unstable();
        assert_eq!(dones, vec![1, 2, 3], "done counts each completion once");
        // Each event's result matches the input repo at its index.
        assert!(events.iter().all(|e| e.result.repo == repos[e.index]));
    }

    #[test]
    fn empty_input_yields_empty_report() {
        let cancel = AtomicBool::new(false);
        let report = run(&[], 4, &cancel, |_| {}, |_| Outcome::Ok(String::new()));
        assert!(report.results.is_empty());
        assert_eq!(report.started, 0);
        assert_eq!(report.completed, 0);
        assert!(!report.cancelled);
    }

    #[test]
    fn default_workers_is_bounded() {
        let w = default_workers();
        assert!((1..=8).contains(&w));
    }

    /// Init a temp repo with one commit; return (tempdir, path). Mirrors the
    /// fixture in `git_ops::tests`.
    fn init_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "t").unwrap();
            cfg.set_str("user.email", "t@t").unwrap();
        }
        std::fs::write(dir.path().join("README.md"), "# Test").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("README.md")).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        (dir, path)
    }

    #[test]
    fn real_git_pull_and_fetch_ops_over_temp_repos() {
        let (_a, path_a) = init_repo();
        let (_b, path_b) = init_repo();
        let repos = vec![path_a.clone(), path_b.clone()];
        let cancel = AtomicBool::new(false);

        // No origin/upstream → pull is a safe skip per repo, in input order.
        let report = run(&repos, 2, &cancel, |_| {}, pull_op());
        assert_eq!(report.results[0].repo, path_a);
        assert_eq!(report.results[1].repo, path_b);
        assert_eq!(report.skipped_count(), 2);
        assert!(report
            .results
            .iter()
            .all(|r| matches!(&r.outcome, Outcome::Skipped(reason) if reason == "no upstream")));

        // fetch has no origin to contact → succeeds with a status summary.
        let report = run(&repos, 2, &cancel, |_| {}, fetch_op());
        assert_eq!(report.ok_count(), 2);
        assert!(report
            .results
            .iter()
            .all(|r| matches!(&r.outcome, Outcome::Ok(s) if s.starts_with("fetched"))));
    }
}
