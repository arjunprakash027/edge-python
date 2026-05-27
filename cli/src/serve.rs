/*
`edge serve`: static dev server with live reload. Serves the project dir, reloads the page on any change.
*/

use anyhow::Result;
use axum::Router;
use notify::{RecursiveMode, Watcher};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::services::ServeDir;
use tower_livereload::LiveReloadLayer;

pub async fn run(dir: PathBuf, port: u16, open: bool) -> Result<()> {
    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();

    let app = Router::new()
        .fallback_service(ServeDir::new(&dir))
        .layer(livereload);

    // Reload the page on any change under the served directory.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            reloader.reload();
        }
    })?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    crate::ui::serve_banner(port, &dir);
    if open {
        let _ = open_url(&format!("http://localhost:{port}"));
    }
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(url).spawn().map(|_| ())
}

#[cfg(target_os = "windows")]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd").args(["/C", "start", url]).spawn().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_url(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
}
