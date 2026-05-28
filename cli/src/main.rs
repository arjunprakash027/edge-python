/*
edge: the Edge Python developer CLI. Run, serve, test, and scaffold Edge Python apps.
*/

mod ui;
mod init;
mod pkg;
mod serve;
mod engine;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::{Path, PathBuf};

use pkg::Manifest;

#[derive(Parser)]
#[command(name = "edge", version, about = "The Edge Python developer CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,

    /// Use a specific manifest instead of ./packages.json.
    #[arg(long, global = true)]
    packages: Option<PathBuf>,

    /// Disable colored output.
    #[arg(long, global = true)]
    no_color: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a script.
    Run { file: Option<PathBuf> },
    /// Interactive shell.
    Repl,
    /// Dev server with live reload.
    Serve {
        #[arg(long, default_value_t = 5173)]
        port: u16,
        #[arg(long)]
        open: bool,
    },
    /// Run *_test.py files.
    Test { path: Option<PathBuf> },
    /// Scaffold a new project.
    Init {
        name: Option<String>,
        #[arg(long)]
        bare: bool,
    },
    /// Add packages to packages.json.
    Add { pkgs: Vec<String> },
    /// Remove packages from packages.json.
    Remove { pkgs: Vec<String> },
    /// Bundle the app into dist/.
    Build {
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    ui::init(cli.no_color);
    let manifest = cli.packages.unwrap_or_else(|| PathBuf::from("packages.json"));

    let result = match cli.cmd {
        Cmd::Init { name, bare } => init::run(name.as_deref(), bare),
        Cmd::Add { pkgs } => pkg::add(&manifest, &pkgs),
        Cmd::Remove { pkgs } => pkg::remove(&manifest, &pkgs),
        Cmd::Serve { port, open } => serve::run(PathBuf::from("."), port, open),
        Cmd::Run { file } => run_script(&manifest, file.as_deref()),
        // These build on the engine but still need their own driver, landing next.
        Cmd::Repl | Cmd::Test { .. } | Cmd::Build { .. } => {
            bail!("not wired yet: this command needs the runtime engine")
        }
    };

    if let Err(e) = result {
        ui::error(&e);
        std::process::exit(1);
    }
    Ok(())
}

/// Read a script from `file` (or stdin when absent) and run it; a script that raises exits non-zero.
fn run_script(manifest_path: &Path, file: Option<&Path>) -> Result<()> {
    let src = match file {
        Some(p) => std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
            s
        }
    };
    let manifest = Manifest::load(manifest_path)?;
    if !engine::run(&src, &manifest)? {
        std::process::exit(1);
    }
    Ok(())
}
