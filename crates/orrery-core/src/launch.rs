//! Spawn external programs from `{path}`-templated command strings (IDE /
//! terminal coding agent). Children are launched detached from the UI.

use std::process::{Child, Command, Stdio};

/// Substitute `{path}` into the template and spawn the program detached in
/// `path`, returning the child handle. Each whitespace-separated token is
/// substituted, so both `code {path}` and `term --cwd={path} -- claude` work.
pub fn spawn(template: &str, path: &str) -> Result<Child, String> {
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
        .map_err(|e| format!("failed to launch `{program}`: {e}"))
}

/// Fire-and-forget launch (IDE etc.).
pub fn launch(template: &str, path: &str) -> Result<(), String> {
    spawn(template, path).map(|_| ())
}

/// Open a folder path or URL in the system default handler (xdg-open), detached.
pub fn open(target: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open `{target}`: {e}"))
}
