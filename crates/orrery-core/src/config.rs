//! Load and persist `AppConfig` as TOML under the XDG config dir
//! (`~/.config/orrery/config.toml`), with sensible PATH-detected defaults.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::AppConfig;

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("orrery").join("config.toml"))
}

// Process-lifetime cache: `load()` is called many times per request cycle
// (scan, enrich, ai, watcher…). The config only changes through `save()` (the
// settings UI), which refreshes the cache, so we avoid re-reading/parsing the
// TOML on every call.
static CACHE: LazyLock<RwLock<Option<AppConfig>>> = LazyLock::new(|| RwLock::new(None));

/// First command on PATH from `candidates`, substituted for `{cmd}` in `template`.
fn detect(candidates: &[&str], template: &str) -> Option<String> {
    candidates
        .iter()
        .find(|c| which::which(c).is_ok())
        .map(|c| template.replace("{cmd}", c))
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = dirs::home_dir()
            .map(|h| h.join("dev").to_string_lossy().into_owned())
            .unwrap_or_else(|| "~/dev".to_string());

        // Prefer an installed GUI editor; fall back to a sensible default.
        let ide_command = detect(&["code", "zed", "subl"], "{cmd} {path}")
            .or_else(|| detect(&["nvim", "vim"], "{cmd} {path}"))
            .unwrap_or_else(|| "xdg-open {path}".to_string());

        // Open the user's terminal at the repo and start a coding agent.
        let term = [
            "kitty",
            "alacritty",
            "foot",
            "wezterm",
            "konsole",
            "gnome-terminal",
        ]
        .iter()
        .find(|t| which::which(t).is_ok())
        .copied();
        let agent_command = match term {
            Some("konsole") => "konsole --workdir {path} -e claude".to_string(),
            Some("gnome-terminal") => {
                "gnome-terminal --working-directory={path} -- claude".to_string()
            }
            Some("wezterm") => "wezterm start --cwd {path} -- claude".to_string(),
            Some(t) => format!("{t} --working-directory {{path}} -e claude"),
            None => "xterm -e claude".to_string(),
        };

        Self {
            roots: vec![home],
            scan_depth: 3,
            // `.claude` keeps tool-managed worktrees (`.claude/worktrees/`)
            // out of `scan::scan_with_worktrees`.
            ignore: [
                "node_modules",
                ".cache",
                "vendor",
                "target",
                "dist",
                ".git",
                ".claude",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            ide_command,
            agent_command,
            agent_dispatch_args: crate::model::default_agent_dispatch_args(),
            github_client_id: String::new(),
            gitlab_hosts: Vec::new(),
            ai_model: crate::model::default_ai_model(),
            ai_enabled: true,
            ai_backend: crate::model::default_ai_backend(),
            llama_server_path: String::new(),
            llama_model_path: String::new(),
            embed_model: crate::model::default_embed_model(),
            ollama_host: crate::model::default_ollama_host(),
            notify_enabled: true,
            notify_new_pr: true,
            notify_review_requested: true,
            notify_ci_failure: true,
            notify_attention: true,
            notify_agent_finished: true,
        }
    }
}

/// Load config, falling back to defaults if absent (written out) or invalid
/// (the bad file is kept and backed up — see [`load_from`]).
/// Cached after the first read; `save()` keeps the cache current.
pub fn load() -> AppConfig {
    if let Some(cfg) = CACHE.read().ok().and_then(|g| g.clone()) {
        return cfg;
    }
    let cfg = load_uncached();
    if let Ok(mut g) = CACHE.write() {
        *g = Some(cfg.clone());
    }
    cfg
}

fn load_uncached() -> AppConfig {
    match config_path() {
        Some(path) => load_from(&path),
        None => AppConfig::default(),
    }
}

/// Read and parse the config at `path`.
///
/// - Missing file → defaults, written out so the user has a file to edit.
/// - Unparseable file (bad TOML, a wrong value type, non-UTF-8) → defaults for
///   this session, but the original is left in place and a byte-for-byte
///   backup (`config.toml.bad-<unix-secs>`) is kept, so nothing the user wrote
///   is lost even after a later `save()` replaces it.
/// - Unreadable for another reason (permissions, I/O) → defaults, file untouched.
fn load_from(path: &Path) -> AppConfig {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let cfg = AppConfig::default();
            if let Err(e) = save_to(path, &cfg) {
                eprintln!("[config] could not write default {}: {e}", path.display());
            }
            return cfg;
        }
        Err(e) => {
            eprintln!(
                "[config] could not read {}: {e}; using defaults",
                path.display()
            );
            return AppConfig::default();
        }
    };
    match parse(&bytes) {
        Ok(cfg) => cfg,
        Err(err) => {
            let backup = match ensure_backup(path, &bytes) {
                Ok(b) => format!("backed up to {}", b.display()),
                Err(e) => format!("backup failed: {e}"),
            };
            eprintln!(
                "[config] {} is invalid ({backup}); using defaults until fixed: {err}",
                path.display()
            );
            AppConfig::default()
        }
    }
}

fn parse(bytes: &[u8]) -> Result<AppConfig, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Make sure a backup of the bad config `bytes` exists beside `path`, reusing
/// an identical earlier backup rather than piling up copies on every launch.
fn ensure_backup(path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    let prefix = format!("{name}.bad-");
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let is_backup = entry.file_name().to_string_lossy().starts_with(&prefix);
            if is_backup && fs::read(entry.path()).is_ok_and(|b| b == bytes) {
                return Ok(entry.path());
            }
        }
    }
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for n in 0u32.. {
        let suffix = if n == 0 {
            secs.to_string()
        } else {
            format!("{secs}-{n}")
        };
        let backup = dir.join(format!("{prefix}{suffix}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup)
        {
            Ok(mut f) => {
                f.write_all(bytes)?;
                f.sync_all()?;
                return Ok(backup);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!("u32 backup suffixes exhausted")
}

/// Persist config as TOML, creating the config directory if needed.
pub fn save(config: &AppConfig) -> Result<(), String> {
    let path = config_path().ok_or("no config directory")?;
    save_to(&path, config)?;
    // Keep the cache in step with what we just wrote.
    if let Ok(mut g) = CACHE.write() {
        *g = Some(config.clone());
    }
    Ok(())
}

/// Atomically write `config` to `path`: temp file in the same directory,
/// fsync, rename over the original (so a crash mid-write can't leave a
/// truncated config). If the file being replaced doesn't parse, it is backed
/// up first — and the save refuses rather than destroy it unbacked.
fn save_to(path: &Path, config: &AppConfig) -> Result<(), String> {
    let dir = path.parent().ok_or("config path has no parent directory")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if let Ok(existing) = fs::read(path) {
        if parse(&existing).is_err() {
            ensure_backup(path, &existing)
                .map_err(|e| format!("not overwriting invalid config without a backup: {e}"))?;
        }
    }
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or("config path has no file name")?;
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let write = || -> io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    // Make the rename itself durable.
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let cfg = AppConfig::default();
        assert!(!cfg.roots.is_empty(), "must have at least one root");
        assert_eq!(cfg.scan_depth, 3);
        assert!(cfg.ignore.iter().any(|i| i == "node_modules"));
        assert!(
            cfg.ide_command.contains("{path}"),
            "ide template needs {{path}}"
        );
        // The agent command launches the agent; the repo dir comes from the
        // launcher's working directory (launch::spawn sets current_dir), so
        // {path} isn't required — and the fallback terminal (xterm) has no
        // working-dir flag. Asserting {path} here would be env-dependent (it
        // only appears when a terminal with such a flag is detected).
        assert!(
            cfg.agent_command.contains("claude"),
            "agent command should launch the agent"
        );
        assert!(!cfg.agent_command.is_empty());
    }

    #[test]
    fn toml_round_trips() {
        let cfg = AppConfig::default();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.roots, cfg.roots);
        assert_eq!(back.scan_depth, cfg.scan_depth);
        assert_eq!(back.ignore, cfg.ignore);
        assert_eq!(back.ide_command, cfg.ide_command);
        assert_eq!(back.agent_command, cfg.agent_command);
        assert_eq!(back.agent_dispatch_args, cfg.agent_dispatch_args);
    }

    #[test]
    fn dispatch_args_default_passes_prompt() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.agent_dispatch_args, "{prompt}");
        // Older configs without the key must default the same way.
        let back: AppConfig = toml::from_str(
            "roots = []\nscanDepth = 3\nignore = []\nideCommand = \"c {path}\"\nagentCommand = \"claude\"",
        )
        .unwrap();
        assert_eq!(back.agent_dispatch_args, "{prompt}");
    }

    fn backups(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().contains("config.toml.bad-"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn partial_config_keeps_set_keys_and_defaults_the_rest() {
        // Only a couple of keys set: before struct-level #[serde(default)] this
        // failed to parse and reset every setting.
        let cfg: AppConfig =
            toml::from_str("roots = [\"/srv/code\"]\nnotifyNewPr = false").unwrap();
        assert_eq!(cfg.roots, vec!["/srv/code".to_string()]);
        assert!(!cfg.notify_new_pr);
        let d = AppConfig::default();
        assert_eq!(cfg.scan_depth, d.scan_depth);
        assert_eq!(cfg.ignore, d.ignore);
        assert_eq!(cfg.agent_dispatch_args, d.agent_dispatch_args);
        assert!(cfg.ai_enabled);
    }

    #[test]
    fn unknown_or_typoed_key_does_not_reset_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // `scanDepht` is a typo: it's ignored and scanDepth falls back, but
        // the other settings survive.
        fs::write(
            &path,
            "roots = [\"/srv/code\"]\nscanDepht = 7\nideCommand = \"zed {path}\"",
        )
        .unwrap();
        let cfg = load_from(&path);
        assert_eq!(cfg.roots, vec!["/srv/code".to_string()]);
        assert_eq!(cfg.ide_command, "zed {path}");
        assert_eq!(cfg.scan_depth, 3);
        assert!(backups(dir.path()).is_empty(), "valid file needs no backup");
    }

    #[test]
    fn bad_toml_is_backed_up_and_left_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let bad = "roots = [\"/srv/code\"\nscanDepth = ";
        fs::write(&path, bad).unwrap();

        let cfg = load_from(&path);
        assert_eq!(cfg.scan_depth, AppConfig::default().scan_depth);
        // Original untouched, one backup with identical bytes.
        assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        let b = backups(dir.path());
        assert_eq!(b.len(), 1);
        assert_eq!(fs::read_to_string(&b[0]).unwrap(), bad);

        // Reloading the same bad file reuses the backup rather than piling up.
        load_from(&path);
        assert_eq!(backups(dir.path()).len(), 1);

        // A save replaces the bad file, and the backup still holds the original.
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).roots, cfg.roots);
        let b = backups(dir.path());
        assert_eq!(b.len(), 1);
        assert_eq!(fs::read_to_string(&b[0]).unwrap(), bad);
    }

    #[test]
    fn wrong_value_type_counts_as_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "scanDepth = \"three\"").unwrap();
        load_from(&path);
        assert_eq!(backups(dir.path()).len(), 1);
    }

    #[test]
    fn save_backs_up_an_invalid_file_it_replaces() {
        // Even without a prior load, a save over an unparseable file keeps it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "not = [valid").unwrap();
        save_to(&path, &AppConfig::default()).unwrap();
        let b = backups(dir.path());
        assert_eq!(b.len(), 1);
        assert_eq!(fs::read_to_string(&b[0]).unwrap(), "not = [valid");
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let cfg = AppConfig {
            scan_depth: 9,
            ..AppConfig::default()
        };
        save_to(&path, &cfg).unwrap();
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path).scan_depth, 9);
        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["config.toml".to_string()]);
    }

    #[test]
    fn missing_file_writes_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = load_from(&path);
        assert!(path.exists());
        assert_eq!(load_from(&path).scan_depth, cfg.scan_depth);
    }
}
