/*
Drives <edge-python> through index.html: boots one tag, then feeds every runtime.json case to its worker
via run(), comparing #app for output cases and the run trace for error cases.
Run: deno test --allow-all runtime/tests/runtime.test.js
*/

import { chromium } from "npm:playwright@latest";
import { readFileSync } from "node:fs";

const REPO = new URL("../../", import.meta.url).pathname; // edge-python/ repo root
const cases = JSON.parse(readFileSync(new URL("./runtime.json", import.meta.url)));
const PKG = JSON.parse(readFileSync(new URL("./app/packages.json", import.meta.url)));
// star-import every module key, recursing through the imports/host category containers
const star = (m) => Object.entries(m).flatMap(([k, v]) => (k === "imports" || k === "host" ? star(v) : `from ${k} import *`));
const PRELUDE = star(PKG).join("\n") + "\n";
const TYPES = {
    ".js": "text/javascript", ".wasm": "application/wasm", ".html": "text/html",
    ".py": "text/x-python", ".json": "application/json",
};

Deno.test("runtime: <edge-python> runs the corpus through index.html", async () => {
    const browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
    const requested = [];
    page.on("request", (q) => requested.push(q.url()));

    const STD_JSON = new URL("../../../edge-python-std/json/target/wasm32-unknown-unknown/release/json.wasm", import.meta.url).pathname;
    const HOST_REPO = new URL("../../../edge-python-host", import.meta.url).pathname;
    await page.route("**/*", (r) => {
        const u = new URL(r.request().url());
        // Prefer the sibling repos' artifacts; if absent (CI checks out only this repo), fall back to the CDN-deployed copy.
        if (u.href.includes("std.edgepython.com/json.wasm")) {
            try { return r.fulfill({ contentType: "application/wasm", body: readFileSync(STD_JSON) }); }
            catch { return r.continue(); } // no sibling std repo: use the deployed wasm
        }
        if (u.host === "host.edgepython.com") {
            // Production (Pages) flattens <cap>/src/* to <cap>/*; map back to the repo layout.
            const repoPath = u.pathname.replace(/^\/([^/]+)\//, "/$1/src/");
            try { return r.fulfill({ contentType: "text/javascript", body: readFileSync(HOST_REPO + repoPath) }); }
            catch { return r.continue(); } // no sibling host repo: use the deployed module
        }
        if (u.host !== "localhost") return r.continue(); // any other CDN asset passes through
        const ext = u.pathname.slice(u.pathname.lastIndexOf("."));
        try { return r.fulfill({ contentType: TYPES[ext] ?? "application/octet-stream", body: readFileSync(REPO + u.pathname.slice(1)) }); }
        catch { return r.fulfill({ status: 404 }); }
    });
    await page.goto("http://localhost/runtime/tests/index.html");

    try {
        // Boot one tag without an entry, then reuse its worker for every case via run().
        await page.evaluate(async () => {
            const el = document.createElement("edge-python");
            el.setAttribute("packages", "./app/packages.json");
            const ready = new Promise((res) => el.addEventListener("ready", res, { once: true }));
            document.body.appendChild(el);
            await ready;
            globalThis.el = el;
        });

        const reqd = (frag) => requested.some((u) => u.includes(frag));
        // Lazy host: a host ESM must not load at boot, only when a run first imports it.
        if (reqd("/app/ui.js")) throw new Error("host ui.js loaded at boot; host modules must be lazy");

        for (const c of cases) {
            errors.length = 0;
            const got = await page.evaluate(async (src) => {
                const app = document.querySelector("#app");
                app.textContent = "";
                const { out } = await globalThis.el.run(src);
                return { app: app.textContent, out };
            }, PRELUDE + c.script);

            if (c.error) {
                if (!got.out.includes(c.error)) {
                    throw new Error(`script:\n${c.script}\n  want error containing: ${JSON.stringify(c.error)}\n  got out: ${JSON.stringify(got.out)}\n  errors: ${errors.join(" | ") || "(none)"}`);
                }
            } else if (got.app !== c.expect) {
                throw new Error(`script:\n${c.script}\n  got:  ${JSON.stringify(got.app)}\n  want: ${JSON.stringify(c.expect)}\n  errors: ${errors.join(" | ") || "(none)"}`);
            }
        }

        // Laziness: only what the corpus imports gets fetched; declared-but-unused stays untouched.
        if (!reqd("/app/ui.js")) throw new Error("host ui was used but ui.js never loaded");
        if (!reqd("json.wasm")) throw new Error("json default imported but json.wasm never fetched");
        if (!reqd("host.edgepython.com/time")) throw new Error("time host default imported but never loaded");
        if (reqd("re.wasm")) throw new Error("re default never imported yet re.wasm was fetched (not lazy)");
        if (reqd("host.edgepython.com/network")) throw new Error("network host default never imported yet fetched (not lazy)");
    } finally {
        await browser.close();
    }
});
