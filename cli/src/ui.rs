/*
Minimalist terminal output. Color via owo-colors, off when piped, NO_COLOR, or --no-color.
*/

use owo_colors::OwoColorize;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static PLAIN: AtomicBool = AtomicBool::new(false);

/// Decide coloring once at startup.
pub fn init(no_color_flag: bool) {
    let off = no_color_flag || std::env::var_os("NO_COLOR").is_some() || !std::io::stdout().is_terminal();
    PLAIN.store(off, Ordering::Relaxed);
}

fn plain() -> bool {
    PLAIN.load(Ordering::Relaxed)
}

/// A `+ name   kind` line for an added package.
pub fn added(name: &str, kind: &str) {
    if plain() {
        println!("  + {name:<10} {kind}");
    } else {
        println!("  {} {name:<10} {}", "+".green(), kind.dimmed());
    }
}

/// A `- name` line for a removed package.
pub fn removed(name: &str) {
    if plain() {
        println!("  - {name}");
    } else {
        println!("  {} {name}", "-".red());
    }
}

/// A blank line then a dim trailing note.
pub fn note(msg: &str) {
    println!();
    if plain() {
        println!("  {msg}");
    } else {
        println!("  {}", msg.dimmed());
    }
}

/// The created-files tree printed by `edge init`.
pub fn scaffolded(dir: &str, items: &[&str], next: &str) {
    let label = if dir == "." { "project" } else { dir };
    println!("  created {label}/");
    for (i, item) in items.iter().enumerate() {
        let branch = if i + 1 == items.len() { "└─" } else { "├─" };
        println!("    {branch} {item}");
    }
    note(next);
}

/// The `edge serve` banner.
pub fn serve_banner(port: u16, dir: &Path) {
    let url = format!("http://localhost:{port}");
    let path = dir.display().to_string();
    if plain() {
        println!("  {url}");
        println!("  watching {path}");
    } else {
        println!("  {}", url.cyan());
        println!("  {} {}", "watching".dimmed(), path.dimmed());
    }
}

/// Print a script's traceback to stderr, verbatim from the runtime.
pub fn traceback(msg: &str) {
    eprintln!("{msg}");
}

/// Render an error to stderr. `{:#}` joins the cause chain so the root reason isn't swallowed.
pub fn error(e: &anyhow::Error) {
    if plain() {
        eprintln!("error: {e:#}");
    } else {
        eprintln!("{} {e:#}", "error:".red());
    }
}
