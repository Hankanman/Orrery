//! A small headless CLI (#49): `orrery list | scan | open <query>`. Parsed
//! before the GUI starts; if a known subcommand is present we handle it and
//! exit, otherwise the desktop app launches as usual.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::Repo;
use crate::{config, launch, scan};

fn scan_now() -> Vec<Repo> {
    let cfg = config::load();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    scan::scan(&cfg.roots, cfg.scan_depth, &cfg.ignore, &HashSet::new(), now)
}

fn list() {
    let repos = scan_now();
    for r in &repos {
        let state = if r.git.dirty > 0 {
            format!("●{}", r.git.dirty)
        } else {
            "✓".to_string()
        };
        println!("{:<30} {:<16} {:<6} {}", r.display_name, r.git.branch, state, r.path);
    }
    eprintln!("{} repos", repos.len());
}

fn open(query: &str) {
    if query.is_empty() {
        eprintln!("usage: orrery open <query>");
        return;
    }
    let q = query.to_lowercase();
    let repos = scan_now();
    let hit = repos.iter().find(|r| {
        r.display_name.to_lowercase().contains(&q)
            || r.slug.as_deref().unwrap_or("").to_lowercase().contains(&q)
            || r.path.to_lowercase().contains(&q)
    });
    match hit {
        Some(r) => match launch::launch(&config::load().ide_command, &r.id) {
            Ok(()) => println!("Opening {} in your IDE…", r.display_name),
            Err(e) => eprintln!("failed to open: {e}"),
        },
        None => eprintln!("no repo matching '{query}'"),
    }
}

fn help() {
    eprintln!(
        "orrery — every repo in orbit\n\n\
         Usage:\n  orrery               launch the desktop app\n\
         \x20 orrery list          list discovered repos\n\
         \x20 orrery open <query>  open the best-matching repo in your IDE\n"
    );
}

/// Handle a CLI subcommand if present; returns true if the GUI should NOT start.
pub fn maybe_run() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") | Some("ls") | Some("scan") => {
            list();
            true
        }
        Some("open") => {
            open(args.get(1).map(String::as_str).unwrap_or(""));
            true
        }
        Some("help") | Some("--help") | Some("-h") => {
            help();
            true
        }
        // No args (normal launch) or anything unrecognized → start the GUI.
        _ => false,
    }
}
