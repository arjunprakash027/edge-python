/*
The runtime engine: run a script against Edge Python inside a headless browser.
We serve a one-page harness (reusing the `<edge-python>` element), point a downloaded Chromium at it, and stream the script's output back.
*/

use anyhow::{anyhow, bail, Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Response, Server};

use crate::pkg::Manifest;

// The harness page; `__EDGE_SRC__` is replaced with the script as a JS string literal.
const HARNESS: &str = include_str!("harness.html");

// Page state we poll for: the runtime is async, so we read its progress instead of blocking one CDP call.
const POLL_JS: &str = "window.__edge ? JSON.stringify(window.__edge) : ''";

// Hard ceiling so a hung script or a failed CDN fetch can't wedge the CLI forever.
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct State {
    lines: Vec<String>,
    done: bool,
    ok: bool,
    #[serde(default)]
    err: String,
}

/// Run `src` against the runtime. Returns Ok(true) on a clean exit, Ok(false) if the script raised.
pub fn run(src: &str, manifest: &Manifest) -> Result<bool> {
    let page = HARNESS.replace("__EDGE_SRC__", &serde_json::to_string(src)?);
    let packages = serde_json::to_string(manifest)?;
    let port = serve(page, packages)?;
    let url = format!("http://127.0.0.1:{port}/");

    let browser = launch().context("launching headless Chromium")?;
    let tab = browser.new_tab().map_err(|e| anyhow!("opening a tab: {e}"))?;
    tab.navigate_to(&url).map_err(|e| anyhow!("navigating to the harness: {e}"))?;
    tab.wait_until_navigated().map_err(|e| anyhow!("waiting for page load: {e}"))?;

    drain(&tab)
}

/// Launch headless Chromium; arch decides between the bundled x86_64 fetcher and system Chrome.
fn launch() -> Result<Browser> {
    let mut builder = LaunchOptions::default_builder();
    builder.sandbox(false); // headless under WSL/containers typically can't sandbox
    if let Some(p) = resolve_chrome()? {
        builder.path(Some(p));
    }
    let options = builder.build().map_err(|e| anyhow!("building launch options: {e}"))?;
    Browser::new(options).map_err(|e| anyhow!("{e}"))
}

/// Returns a path to drive, or None to let the x86_64 fetcher download; `EDGE_CHROME_PATH` always wins.
fn resolve_chrome() -> Result<Option<PathBuf>> {
    if let Some(p) = std::env::var_os("EDGE_CHROME_PATH") {
        return Ok(Some(PathBuf::from(p)));
    }
    if cfg!(target_arch = "x86_64") {
        return Ok(None);
    }
    headless_chrome::browser::default_executable().map(Some).map_err(|e| {
        anyhow!("no Chrome on {}; bundled fetcher is x86_64-only. Install Chrome or set EDGE_CHROME_PATH ({e})", std::env::consts::ARCH)
    })
}

/// Poll the page, stream new output lines, and resolve when the script finishes.
fn drain(tab: &headless_chrome::Tab) -> Result<bool> {
    let stdout = std::io::stdout();
    let mut printed = 0usize;
    let deadline = Instant::now() + RUN_TIMEOUT;

    loop {
        if Instant::now() > deadline {
            bail!("timed out after {}s waiting for the script", RUN_TIMEOUT.as_secs());
        }

        let raw = tab.evaluate(POLL_JS, false).map_err(|e| anyhow!("reading page state: {e}"))?;
        let json = raw.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        if json.is_empty() {
            thread::sleep(Duration::from_millis(60));
            continue;
        }

        let state: State = serde_json::from_str(json).context("parsing page state")?;
        for line in state.lines.iter().skip(printed) {
            let _ = writeln!(stdout.lock(), "{line}");
        }
        printed = state.lines.len();

        if state.done {
            if state.ok {
                return Ok(true);
            }
            crate::ui::traceback(&state.err);
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(60));
    }
}

/// Serve the harness at `/` and the manifest at `/packages.json` on a free loopback port. The thread is a daemon.
fn serve(page: String, packages: String) -> Result<u16> {
    let server = Server::http("127.0.0.1:0").map_err(|e| anyhow!("starting local server: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("local server has no TCP address"))?
        .port();

    thread::spawn(move || {
        for req in server.incoming_requests() {
            let path = req.url().split('?').next().unwrap_or("/");
            let resp = match path {
                "/" => Response::from_string(page.clone()).with_header(ctype("text/html; charset=utf-8")),
                "/packages.json" => Response::from_string(packages.clone()).with_header(ctype("application/json")),
                _ => Response::from_string("not found").with_status_code(404),
            };
            let _ = req.respond(resp);
        }
    });
    Ok(port)
}

fn ctype(value: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], value.as_bytes()).expect("static header is valid")
}
