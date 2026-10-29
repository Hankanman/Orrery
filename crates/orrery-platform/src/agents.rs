//! Detect terminal coding-agent sessions running on the system by scanning
//! `/proc` — not just ones Orrery launched. A process counts as an agent session
//! if its program (argv[0], or the script argv[1] names when argv[0] is an
//! interpreter like `python`/`node`) is a known agent CLI *and* its working
//! directory is inside one of the scanned repos (the deepest one wins, so a
//! worktree nested in a repo is attributed to the worktree). Terminate is a
//! plain signal by pid, guarded against pid reuse by the process start time
//! recorded at detection, so no child-process bookkeeping is needed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `starttime` ticks of every agent pid returned by the last [`detect`], so
/// [`terminate`] can refuse to signal a pid that has since been reused.
static DETECTED: Mutex<Option<HashMap<u32, i64>>> = Mutex::new(None);

/// A detected agent process.
pub struct RunningAgent {
    pub pid: u32,
    /// The repo path whose tree the process is running in.
    pub repo: String,
    /// The full command line.
    pub command: String,
    /// Unix start time of the process (for a runtime readout).
    pub started_unix: i64,
}

/// Last path segment of `s` (the program basename).
fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Boot time in unix seconds, from `/proc/stat`'s `btime` line.
fn boot_time() -> i64 {
    std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("btime ").and_then(|n| n.trim().parse().ok()))
        })
        .unwrap_or(0)
}

/// Raw `starttime` (clock ticks since boot) of a live pid, from `/proc/<pid>/stat`.
fn pid_start_ticks(pid: u32) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    starttime_ticks(&stat)
}

/// Process start time in unix seconds from its `starttime` ticks.
fn start_unix(ticks: Option<i64>, btime: i64) -> i64 {
    // Clock ticks per second; 100 on effectively all Linux configs.
    const HZ: i64 = 100;
    match ticks {
        Some(ticks) => btime + ticks / HZ,
        None => btime,
    }
}

/// Interpreters that host agent CLIs as scripts (`node /usr/bin/gemini`,
/// `python3 ~/.local/bin/aider`): for these the program is in argv[1].
fn is_interpreter(name: &str) -> bool {
    name.starts_with("python") || matches!(name, "node" | "nodejs" | "bun" | "deno")
}

/// The program name to match against the agent list: argv[0]'s basename, or —
/// when that's an interpreter — the hosted script's basename (first non-flag
/// argument, or the module after `-m`), minus a `.js`/`.mjs`/`.py` extension.
fn program_name(args: &[String]) -> Option<&str> {
    let argv0 = basename(args.first()?);
    if !is_interpreter(argv0) {
        return Some(argv0);
    }
    let mut rest = args[1..].iter();
    let script = loop {
        let a = rest.next()?;
        if a == "-m" {
            break rest.next()?;
        }
        if !a.starts_with('-') {
            break a;
        }
    };
    let name = basename(script);
    Some(
        [".mjs", ".cjs", ".js", ".py"]
            .iter()
            .find_map(|ext| name.strip_suffix(ext))
            .unwrap_or(name),
    )
}

/// The deepest of `repos` (paired with its canonical path) containing `cwd`.
/// Longest-prefix, so a worktree nested inside a scanned repo wins over it.
fn attribute<'a>(cwd: &Path, repos: &'a [(String, PathBuf)]) -> Option<&'a String> {
    repos
        .iter()
        .filter(|(_, canon)| cwd.starts_with(canon))
        .max_by_key(|(_, canon)| canon.components().count())
        .map(|(repo, _)| repo)
}

/// Parse the `starttime` field (clock ticks since boot, field 22) out of a
/// `/proc/<pid>/stat` line. Pure, so it's unit-testable. Returns `None` if the
/// line is malformed. `comm` (field 2, parenthesized) can itself contain spaces
/// and parens, so we anchor on the *last* ')' and count from there: starttime is
/// the 20th whitespace-separated field after it (index 19).
fn starttime_ticks(stat: &str) -> Option<i64> {
    let rparen = stat.rfind(')')?;
    stat[rparen + 1..]
        .split_whitespace()
        .nth(19)
        .and_then(|s| s.parse::<i64>().ok())
}

/// Scan running processes for agent sessions inside `repos`. `programs` is the
/// set of agent CLI basenames to match (e.g. `claude`, `aider`).
pub fn detect(repos: &[String], programs: &[String]) -> Vec<RunningAgent> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    let btime = boot_time();
    // `/proc/<pid>/cwd` is fully resolved, so compare against resolved roots —
    // a symlinked repo/root would otherwise never match. Keep the caller's
    // spelling for the returned `repo` (the app keys repos by it).
    let repos: Vec<(String, PathBuf)> = repos
        .iter()
        .map(|r| {
            let canon = std::fs::canonicalize(r).unwrap_or_else(|_| PathBuf::from(r));
            (r.clone(), canon)
        })
        .collect();
    let mut ticks_by_pid = HashMap::new();

    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };

        // argv, NUL-separated.
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let args: Vec<String> = raw
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let Some(program) = program_name(&args) else {
            continue;
        };
        if !programs.iter().any(|p| program == p.as_str()) {
            continue;
        }

        // Working directory (own processes only — fine for the user's agents).
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        let Some(repo) = attribute(&cwd, &repos) else {
            continue;
        };

        let ticks = pid_start_ticks(pid);
        if let Some(t) = ticks {
            ticks_by_pid.insert(pid, t);
        }
        out.push(RunningAgent {
            pid,
            repo: repo.clone(),
            command: args.join(" "),
            started_unix: start_unix(ticks, btime),
        });
    }
    if let Ok(mut detected) = DETECTED.lock() {
        *detected = Some(ticks_by_pid);
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.started_unix));
    out
}

/// Send SIGTERM to an agent process returned by the most recent [`detect`].
/// Refuses (returns `false`) unless the pid was detected *and* still has the
/// same start time — i.e. it hasn't exited and been reused by an unrelated
/// process since the scan.
pub fn terminate(pid: u32) -> bool {
    let expected = DETECTED
        .lock()
        .ok()
        .and_then(|d| d.as_ref().and_then(|m| m.get(&pid).copied()));
    match expected {
        Some(ticks) => terminate_if_same(pid, ticks),
        None => false,
    }
}

/// SIGTERM `pid` only if its `/proc/<pid>/stat` start time still equals
/// `expected_ticks`. (A tiny race remains between the check and the signal;
/// a pidfd would close it, but that window is microseconds.)
fn terminate_if_same(pid: u32, expected_ticks: i64) -> bool {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if raw <= 0 || pid_start_ticks(pid) != Some(expected_ticks) {
        return false;
    }
    // SAFETY: kill(2) takes plain integers and has no memory-safety contract.
    unsafe { libc::kill(raw, libc::SIGTERM) == 0 }
}

#[cfg(test)]
mod tests {
    use super::{attribute, basename, program_name, starttime_ticks, terminate_if_same};
    use std::path::{Path, PathBuf};

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn program_name_reads_argv0_or_the_hosted_script() {
        assert_eq!(
            program_name(&argv("/usr/bin/claude --resume")),
            Some("claude")
        );
        assert_eq!(
            program_name(&argv("/usr/bin/python3 /home/u/.local/bin/aider --yes")),
            Some("aider")
        );
        assert_eq!(program_name(&argv("node /usr/bin/gemini")), Some("gemini"));
        assert_eq!(
            program_name(&argv("node --no-warnings /opt/qwen/qwen.js")),
            Some("qwen")
        );
        assert_eq!(program_name(&argv("python3.12 -m aider")), Some("aider"));
        assert_eq!(program_name(&argv("bun opencode.mjs")), Some("opencode"));
        assert_eq!(program_name(&argv("python3")), None, "bare interpreter");
        assert_eq!(program_name(&[]), None);
    }

    #[test]
    fn attribute_picks_the_deepest_repo() {
        let repos = vec![
            ("/r/app".to_string(), PathBuf::from("/r/app")),
            (
                "/r/app/.wt/feat".to_string(),
                PathBuf::from("/r/app/.wt/feat"),
            ),
            ("/r/other".to_string(), PathBuf::from("/r/other")),
        ];
        let at = |p: &str| attribute(Path::new(p), &repos).map(String::as_str);
        assert_eq!(at("/r/app/src"), Some("/r/app"));
        assert_eq!(at("/r/app/.wt/feat/src"), Some("/r/app/.wt/feat"));
        assert_eq!(at("/r/app/.wt/feat"), Some("/r/app/.wt/feat"));
        assert_eq!(at("/r/appendix"), None, "component-wise, not string prefix");
        assert_eq!(at("/elsewhere"), None);
    }

    #[test]
    fn attribute_matches_symlinked_roots_via_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("src")).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let spelled = link.to_string_lossy().into_owned();
        let repos = vec![(spelled.clone(), std::fs::canonicalize(&link).unwrap())];
        // /proc/<pid>/cwd is resolved, i.e. the real path.
        let cwd = std::fs::canonicalize(real.join("src")).unwrap();
        assert_eq!(attribute(&cwd, &repos), Some(&spelled));
    }

    #[test]
    fn terminate_refuses_a_reused_pid() {
        // Our own pid with a start time that can't be ours: must not signal.
        assert!(!terminate_if_same(std::process::id(), -1));
        assert!(!terminate_if_same(0, 0), "pid 0 = process group, never");
    }

    #[test]
    fn basename_takes_trailing_segment() {
        assert_eq!(basename("/usr/bin/claude"), "claude");
        assert_eq!(basename("claude"), "claude");
        assert_eq!(basename(""), "");
    }

    #[test]
    fn starttime_ticks_parses_field_22() {
        // Synthetic /proc/<pid>/stat: `pid (comm) <fields 3..=22>`, where each
        // token's value equals its field number, so field 22 reads back as 22.
        let after_comm: String = (3..=22)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let stat = format!("1234 (bash) {after_comm}\n");
        assert_eq!(starttime_ticks(&stat), Some(22));
    }

    #[test]
    fn starttime_ticks_handles_comm_with_spaces_and_parens() {
        // A program named "(weird) name" must not break field counting.
        let after_comm: String = (3..=22)
            .map(|n| (n * 10).to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let stat = format!("42 ((weird) name) {after_comm}");
        assert_eq!(starttime_ticks(&stat), Some(220));
    }

    #[test]
    fn starttime_ticks_rejects_malformed() {
        assert_eq!(starttime_ticks("no parens here"), None);
        assert_eq!(starttime_ticks("1 (x) S 1 2 3"), None, "too few fields");
    }
}
