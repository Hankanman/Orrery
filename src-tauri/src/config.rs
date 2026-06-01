//! Load and persist `AppConfig` as TOML under the XDG config dir
//! (`~/.config/orrery/config.toml`), with sensible PATH-detected defaults.

use std::fs;
use std::path::PathBuf;

use crate::model::AppConfig;

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("orrery").join("config.toml"))
}

/// First command on PATH from `candidates`, formatted into a `{path}` template.
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
        let term = ["kitty", "alacritty", "foot", "wezterm", "konsole", "gnome-terminal"]
            .iter()
            .find(|t| which::which(t).is_ok())
            .copied();
        let agent_command = match term {
            Some("konsole") => "konsole --workdir {path} -e claude".to_string(),
            Some("gnome-terminal") => "gnome-terminal --working-directory={path} -- claude".to_string(),
            Some("wezterm") => "wezterm start --cwd {path} -- claude".to_string(),
            Some(t) => format!("{t} --working-directory {{path}} -e claude"),
            None => "xterm -e claude".to_string(),
        };

        Self {
            roots: vec![home],
            scan_depth: 3,
            ignore: ["node_modules", ".cache", "vendor", "target", "dist", ".git"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ide_command,
            agent_command,
            github_client_id: String::new(),
        }
    }
}

/// Load config, falling back to (and writing) defaults if absent/invalid.
pub fn load() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).unwrap_or_else(|_| AppConfig::default()),
        Err(_) => {
            let cfg = AppConfig::default();
            let _ = save(&cfg);
            cfg
        }
    }
}

/// Persist config as TOML, creating the config directory if needed.
pub fn save(config: &AppConfig) -> Result<(), String> {
    let path = config_path().ok_or("no config directory")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| e.to_string())
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
        assert!(cfg.ide_command.contains("{path}"), "ide template needs {{path}}");
        assert!(cfg.agent_command.contains("{path}"), "agent template needs {{path}}");
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
    }
}
