/*
The runtime engine: drive Edge Python in a headless browser, one-shot via `run` or persistent via `Session`.
*/

use anyhow::{anyhow, bail, Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Response, Server};

use crate::pkg::Manifest;

// Harness exposes `window.__edgeRun(src)` and `window.__edgeReady` once the worker boots.
const HARNESS: &str = include_str!("templates/harness.html");

// The runtime is async, so we poll state instead of blocking on one CDP call.
const POLL_JS: &str = "window.__edge ? JSON.stringify(window.__edge) : ''";
const READY_JS: &str = "!!window.__edgeReady";

// Hard ceiling so a hung script or a failed CDN fetch can't wedge the CLI.
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const EVAL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct State {
    lines: Vec<String>,
    done: bool,
    ok: bool,
    #[serde(default)]
    err: String,
}

/// Result of one eval: streamed lines went out via the `on_line` callback; only the error survives.
pub struct Outcome {
    pub err: Option<String>,
}

/// A live runtime session: browser, tab and harness stay up so successive `eval` calls share state.
pub struct Session {
    _browser: Browser,
    tab: Arc<headless_chrome::Tab>,
}

impl Session {
    /// Boot the harness page and wait until the worker is ready to receive eval calls.
    pub fn open(manifest: &Manifest) -> Result<Self> {
        let packages = serde_json::to_string(manifest)?;
        let port = serve(packages)?;
        let url = format!("http://127.0.0.1:{port}/");

        let browser = launch().context("launching headless Chromium")?;
        let tab = browser.new_tab().map_err(|e| anyhow!("opening a tab: {e}"))?;
        tab.navigate_to(&url).map_err(|e| anyhow!("navigating to the harness: {e}"))?;
        tab.wait_until_navigated().map_err(|e| anyhow!("waiting for page load: {e}"))?;
        wait_ready(&tab)?;
        Ok(Self { _browser: browser, tab })
    }

    /// Run `src` on the existing worker; stream each printed line through `on_line` and report errors.
    pub fn eval<F: FnMut(&str)>(&mut self, src: &str, mut on_line: F) -> Result<Outcome> {
        let literal = serde_json::to_string(src)?;
        let expr = format!("__edgeRun({literal})");
        self.tab.evaluate(&expr, false).map_err(|e| anyhow!("starting eval: {e}"))?;
        drain(&self.tab, &mut on_line)
    }
}

/// One-shot: open a session, eval `src`, print lines to stdout, tear down. Ok(true) on clean exit.
pub fn run(src: &str, manifest: &Manifest) -> Result<bool> {
    let mut session = Session::open(manifest)?;
    let outcome = session.eval(src, |line| println!("{line}"))?;
    if let Some(err) = outcome.err {
        crate::ui::traceback(&err);
        return Ok(false);
    }
    Ok(true)
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

/// Block until the harness has set `window.__edgeReady = true` (worker created, ready for evals).
fn wait_ready(tab: &headless_chrome::Tab) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if Instant::now() > deadline {
            bail!("timed out after {}s waiting for the runtime to load", READY_TIMEOUT.as_secs());
        }
        let raw = tab.evaluate(READY_JS, false).map_err(|e| anyhow!("polling runtime ready: {e}"))?;
        if raw.value.as_ref().and_then(|v| v.as_bool()) == Some(true) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(60));
    }
}

/// Poll the page, stream new output lines, and resolve when the current eval finishes.
fn drain<F: FnMut(&str)>(tab: &headless_chrome::Tab, on_line: &mut F) -> Result<Outcome> {
    let mut printed = 0usize;
    let deadline = Instant::now() + EVAL_TIMEOUT;

    loop {
        if Instant::now() > deadline {
            bail!("timed out after {}s waiting for the script", EVAL_TIMEOUT.as_secs());
        }
        let raw = tab.evaluate(POLL_JS, false).map_err(|e| anyhow!("reading page state: {e}"))?;
        let json = raw.value.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        if json.is_empty() {
            thread::sleep(Duration::from_millis(60));
            continue;
        }
        let state: State = serde_json::from_str(json).context("parsing page state")?;
        for line in state.lines.iter().skip(printed) {
            on_line(line);
        }
        printed = state.lines.len();
        if state.done {
            return Ok(Outcome { err: if state.ok { None } else { Some(state.err) } });
        }
        thread::sleep(Duration::from_millis(60));
    }
}

/// Serve the harness at `/` and the manifest at `/packages.json` on a free loopback port. The thread is a daemon.
fn serve(packages: String) -> Result<u16> {
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
                "/" => Response::from_string(HARNESS).with_header(ctype("text/html; charset=utf-8")),
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
