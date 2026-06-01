//! Spawn external programs from `{path}`-templated command strings (IDE /
//! terminal coding agent). Children are launched detached from the UI.

use std::process::{Command, Stdio};

/// Substitute `{path}` into the template, spawn the program in `path`, and
/// return immediately. Each whitespace-separated token is substituted, so both
/// `code {path}` and `term --working-directory={path} -- claude` work.
pub fn launch(template: &str, path: &str) -> Result<(), String> {
    let mut tokens = template
        .split_whitespace()
        .map(|t| t.replace("{path}", path));

    let program = tokens.next().ok_or("empty command template")?;
    let args: Vec<String> = tokens.collect();

    Command::new(&program)
        .args(&args)
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to launch `{program}`: {e}"))?;
    Ok(())
}
