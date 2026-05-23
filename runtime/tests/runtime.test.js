/*
Runtime tester. Cases live in `./cases.json` (table-driven, mirrors `compiler/tests/cases/vm.json`).
Serves runtime/ from disk, drives `createWorker(...)` in Chromium, uses CDN wasm to decouple from local builds.
Run: `deno test --allow-all runtime/tests/runtime.test.js` (one-time: `deno run -A npm:playwright install chromium`).
*/

import { chromium } from "npm:playwright@latest";
import { readFileSync } from "node:fs";

const REPO = new URL("../../", import.meta.url).pathname; // edge-python/
const WASM_URL = "https://runtime.edgepython.com/js/compiler_lib.wasm";
const cases = JSON.parse(readFileSync(new URL("./cases.json", import.meta.url)));

/* Named handler fixtures referenced by `cases.main_thread`, kept in JS because handler bodies are functions (not JSON-serializable). Each case lists fixture names; the harness rehydrates them inside the browser. */
const FIXTURES = {
    uppercase: "(s) => s.toUpperCase()",
    double:    "(n) => Number(n) * 2",
    echo_async: "async (s) => { await new Promise(r => setTimeout(r, 10)); return 'echo:' + s; }",
};

const TYPES = { ".js": "text/javascript", ".wasm": "application/wasm", ".html": "text/html" };

Deno.test("runtime", async (t) => {
    const browser = await chromium.launch();
    const page = await browser.newPage();

    /* Intercept only the local-virtual host; let external URLs (CDN wasm, Anthropic runtime) pass through. */
    await page.route("**/*", (route) => {
        const url = new URL(route.request().url());
        if (url.host !== "x") return route.continue();
        const p = url.pathname;
        if (p === "/") return route.fulfill({ contentType: "text/html", body: "<!doctype html><body></body>" });
        if (p.startsWith("/runtime/")) {
            try {
                const ext = p.slice(p.lastIndexOf("."));
                return route.fulfill({ contentType: TYPES[ext] ?? "application/octet-stream", body: readFileSync(REPO + p.slice(1)) });
            } catch { return route.fulfill({ status: 404 }); }
        }
        return route.fulfill({ status: 404 });
    });
    await page.goto("http://x/");

    try {
        for (const c of cases) {
            await t.step(c.name, async () => {
                const result = await page.evaluate(async ({ c, FIXTURES, WASM_URL }) => {
                    const { createWorker } = await import("/runtime/src/index.js");
                    /* Rehydrate handler fixtures (source strings -> functions) inside the browser. */
                    const handlers = Object.fromEntries(
                        Object.entries(FIXTURES).map(([k, src]) => [k, new Function(`return (${src});`)()]),
                    );
                    const mainThreadModules = {};
                    for (const [mod, fns] of Object.entries(c.main_thread ?? {})) {
                        mainThreadModules[mod] = () => Object.fromEntries(fns.map((f) => [f, handlers[f]]));
                    }
                    const worker = await createWorker({ wasmUrl: WASM_URL, mainThreadModules });
                    const out = [];
                    worker.onOutput((line) => out.push(line));
                    for (const e of c.events ?? []) worker.pushEvent(e);
                    const run = await worker.run(c.script);
                    worker.dispose();
                    return { out, trace: run.out ?? "" };
                }, { c, FIXTURES, WASM_URL });

                if (c.output) {
                    const got = JSON.stringify(result.out);
                    const want = JSON.stringify(c.output);
                    if (got !== want) throw new Error(`output mismatch:\n  got:  ${got}\n  want: ${want}\n  trace: ${result.trace}`);
                }
                if (c.error) {
                    if (!result.trace.includes(c.error)) {
                        throw new Error(`expected error containing '${c.error}', got trace: ${result.trace || "(none)"}, output: ${JSON.stringify(result.out)}`);
                    }
                }
            });
        }
    } finally {
        await browser.close();
    }
});
