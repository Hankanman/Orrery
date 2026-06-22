//! Detect terminal coding-agent sessions running on the system by scanning
//! `/proc` — not just ones Orrery launched. A process counts as an agent session
//! if its program (argv[0]) is a known agent CLI *and* its working directory is
//! inside one of the scanned repos. Terminate is a plain signal by pid, so no
//! child-process bookkeeping is needed.

use std::path::Path;

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

/// Process start time in unix seconds, from `/proc/<pid>/stat` field 22.
fn start_time(pid: u32, btime: i64) -> i64 {
    // Clock ticks per second; 100 on effectively all Linux configs.
    const HZ: i64 = 100;
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return btime;
    };
    // `comm` (field 2) may contain spaces/parens, so split after the last ')'.
    let Some(rparen) = stat.rfind(')') else {
        return btime;
    };
    let fields: Vec<&str> = stat[rparen + 1..].split_whitespace().collect();
    // After comm, starttime (field 22) is index 19.
    let ticks = fields
        .get(19)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    btime + ticks / HZ
}

/// Scan running processes for agent sessions inside `repos`. `programs` is the
/// set of agent CLI basenames to match (e.g. `claude`, `aider`).
pub fn detect(repos: &[String], programs: &[String]) -> Vec<RunningAgent> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    let btime = boot_time();

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
        let Some(argv0) = args.first() else {
            continue;
        };
        if !programs.iter().any(|p| basename(argv0) == p.as_str()) {
            continue;
        }

        // Working directory (own processes only — fine for the user's agents).
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        let Some(repo) = repos.iter().find(|r| {
            let r = Path::new(r);
            cwd == r || cwd.starts_with(r)
        }) else {
            continue;
        };

        out.push(RunningAgent {
            pid,
            repo: repo.clone(),
            command: args.join(" "),
            started_unix: start_time(pid, btime),
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.started_unix));
    out
}

/// Send SIGTERM to a process. Best-effort via the `kill` binary (no extra deps).
pub fn terminate(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
