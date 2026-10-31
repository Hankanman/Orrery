//! Spawn external programs from `{path}`-templated command strings (IDE /
//! terminal coding agent). Children are launched detached from the UI.

use std::process::{Child, Command, Stdio};

/// Spawn `tokens[0]` with the remaining tokens as args, detached, in `dir`.
fn spawn_tokens(tokens: Vec<String>, dir: &str) -> Result<Child, String> {
    let mut iter = tokens.into_iter();
    let program = iter.next().ok_or("empty command template")?;
    let args: Vec<String> = iter.collect();

    Command::new(&program)
        .args(&args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to launch `{program}`: {e}"))
}

/// Substitute `{path}` into the template and spawn the program detached in
/// `path`, returning the child handle. Each whitespace-separated token is
/// substituted, so both `code {path}` and `term --cwd={path} -- claude` work.
pub fn spawn(template: &str, path: &str) -> Result<Child, String> {
    let tokens = template
        .split_whitespace()
        .map(|t| t.replace("{path}", path))
        .collect();
    spawn_tokens(tokens, path)
}

/// Spawn the agent command with a task prompt appended: the `dispatch_args`
/// template's tokens are added after `template`'s, with `{prompt}` substituted
/// *after* tokenizing — so a multi-word prompt stays one argv entry (the
/// default `{prompt}` becomes `claude "fix the tests"`, not four args).
pub fn spawn_with_prompt(
    template: &str,
    dispatch_args: &str,
    path: &str,
    prompt: &str,
) -> Result<Child, String> {
    spawn_tokens(dispatch_tokens(template, dispatch_args, path, prompt), path)
}

/// Build the argv for [`spawn_with_prompt`] (pure, so it's unit-testable).
fn dispatch_tokens(template: &str, dispatch_args: &str, path: &str, prompt: &str) -> Vec<String> {
    template
        .split_whitespace()
        .map(|t| t.replace("{path}", path))
        .chain(
            dispatch_args
                .split_whitespace()
                .map(|t| t.replace("{path}", path).replace("{prompt}", prompt)),
        )
        .collect()
}

/// Fire-and-forget launch (IDE etc.).
pub fn launch(template: &str, path: &str) -> Result<(), String> {
    spawn(template, path).map(|_| ())
}

/// Open a folder path or URL in the system default handler (xdg-open), detached.
///
/// Many targets come from remote hosts (inbox items, CI `target_url`s, feed
/// links), and anyone who can push a commit status controls those strings.
/// xdg-open dispatches on scheme to arbitrary desktop handlers (`file:`,
/// `smb:`, custom app schemes), so only http(s) URLs and existing absolute
/// local paths are opened; everything else is refused.
pub fn open(target: &str) -> Result<(), String> {
    let target = openable(target)?;
    Command::new("xdg-open")
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open `{target}`: {e}"))
}

/// Validate an [`open`] target: an absolute http(s) URL with a host (returned
/// normalised by the URL parser), or an absolute path that exists locally.
fn openable(target: &str) -> Result<String, String> {
    let target = target.trim();
    if let Ok(url) = reqwest::Url::parse(target) {
        return match url.scheme() {
            "http" | "https" if url.host_str().is_some_and(|h| !h.is_empty()) => {
                Ok(url.to_string())
            }
            _ if std::path::Path::new(target).is_absolute() => local(target),
            scheme => Err(format!("refusing to open `{scheme}:` link")),
        };
    }
    local(target)
}

fn local(target: &str) -> Result<String, String> {
    let path = std::path::Path::new(target);
    if path.is_absolute() && path.exists() {
        Ok(target.to_string())
    } else {
        Err(format!(
            "refusing to open `{target}`: not a URL or an existing absolute path"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{dispatch_tokens, openable};

    #[test]
    fn opens_http_urls_and_existing_paths() {
        assert_eq!(
            openable("https://github.com/o/r/pull/1").unwrap(),
            "https://github.com/o/r/pull/1"
        );
        assert!(openable("http://ci.example.com/build/9").is_ok());
        assert!(openable("  https://codeberg.org/o/r  ").is_ok());
        let dir = std::env::temp_dir();
        assert!(openable(dir.to_str().unwrap()).is_ok());
    }

    #[test]
    fn refuses_other_schemes_and_junk() {
        for bad in [
            "file:///etc/passwd",
            "smb://evil.example/share",
            "javascript:alert(1)",
            "vscode://ext/run",
            "steam://run/1",
            "https://",
            "relative/path",
            "-flag-looking",
            "/definitely/not/a/real/path/orrery-test",
            "",
        ] {
            assert!(openable(bad).is_err(), "should refuse {bad:?}");
        }
    }

    #[test]
    fn prompt_stays_one_token() {
        let argv = dispatch_tokens(
            "kitty --working-directory {path} -e claude",
            "{prompt}",
            "/repo",
            "fix the flaky tests",
        );
        assert_eq!(
            argv,
            vec![
                "kitty",
                "--working-directory",
                "/repo",
                "-e",
                "claude",
                "fix the flaky tests",
            ]
        );
    }

    #[test]
    fn dispatch_args_can_carry_flags_and_path() {
        let argv = dispatch_tokens("claude", "--cwd {path} {prompt}", "/r", "do x");
        assert_eq!(argv, vec!["claude", "--cwd", "/r", "do x"]);
    }

    #[test]
    fn empty_dispatch_args_appends_nothing() {
        let argv = dispatch_tokens("claude", "", "/r", "ignored");
        assert_eq!(argv, vec!["claude"]);
    }
}
