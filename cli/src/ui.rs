/*
Minimalist terminal output. Color via owo-colors, off when piped, NO_COLOR, or --no-color.
*/

use owo_colors::OwoColorize;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

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

/// Summary printed by `edge build` after vendoring runtime + packages + scripts into dist/.
pub fn build_report(dir: &std::path::Path, runtime_files: usize, packages: usize, scripts: usize, size: u64, elapsed: std::time::Duration) {
    println!();
    if plain() {
        println!("  bundled to {}/", dir.display());
    } else {
        println!("  bundled to {}/", dir.display().to_string().cyan());
    }
    println!();
    println!("  {runtime_files} runtime files + compiler.wasm");
    println!("  {packages} packages");
    println!("  {scripts} scripts");
    println!();
    println!("  {:.2} MB · {:.1}s", size as f64 / 1_000_000.0, elapsed.as_secs_f64());
}

/// Render an error to stderr. `{:#}` joins the cause chain so the root reason isn't swallowed.
pub fn error(e: &anyhow::Error) {
    if plain() {
        eprintln!("error: {e:#}");
    } else {
        eprintln!("{} {e:#}", "error:".red());
    }
}

// Braille frames used by the spinner; match the rest of the UI's unicode style.
const SPIN_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Animated stderr spinner for long operations. Falls back to a static line in plain/non-TTY mode.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

/// Start a spinner with `label`. Drop it (or call `done`) to stop.
pub fn spinner(label: &str) -> Spinner {
    let stop = Arc::new(AtomicBool::new(false));
    // No animation when colors are off or stderr is redirected; just announce the step.
    if plain() || !std::io::stderr().is_terminal() {
        eprintln!("  {label}...");
        return Spinner { stop, handle: None };
    }
    let stop_thread = stop.clone();
    let label = label.to_string();
    let handle = thread::spawn(move || {
        let mut i = 0usize;
        while !stop_thread.load(Ordering::Relaxed) {
            let frame = SPIN_FRAMES[i % SPIN_FRAMES.len()];
            eprint!("\r  {} {}", frame, label.dimmed());
            let _ = std::io::stderr().flush();
            thread::sleep(Duration::from_millis(120));
            i += 1;
        }
        // ANSI: carriage return + erase-to-end-of-line so the next print starts clean.
        eprint!("\r\x1b[2K");
        let _ = std::io::stderr().flush();
    });
    Spinner { stop, handle: Some(handle) }
}

impl Spinner {
    /// Stop the spinner and print a `(successful) {msg}` line in green.
    pub fn done(self, msg: &str) {
        // Drop stops the thread and clears the line; the result line goes right after.
        drop(self);
        if plain() {
            eprintln!("  (successful) {msg}");
        } else {
            eprintln!("  {} {msg}", "(successful)".green());
        }
    }

    /// Stop the spinner and print an `(unsuccessful) {msg}` line in red.
    pub fn fail(self, msg: &str) {
        drop(self);
        if plain() {
            eprintln!("  (unsuccessful) {msg}");
        } else {
            eprintln!("  {} {msg}", "(unsuccessful)".red());
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
