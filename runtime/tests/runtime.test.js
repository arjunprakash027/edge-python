/* Single-file runtime tester. Serves runtime/ from disk, drives `createWorker(...)` in Chromium per case. Uses the CDN-deployed wasm so this test is decoupled from local builds (mirrors the capabilities pattern). Run: deno test --allow-all runtime/tests/runtime.test.js (one-time: deno run -A npm:playwright install chromium). */

import { chromium } from "npm:playwright@latest";
import { readFileSync } from "node:fs";

const REPO = new URL("../../", import.meta.url).pathname; // edge-python/
const WASM_URL = "https://runtime.edgepython.com/js/compiler_lib.wasm";

/* Named handler fixtures referenced by cases.main_thread — keeps cases declarative (no inline JS). */
const FIXTURES = {
    uppercase: "(s) => s.toUpperCase()",
    double:    "(n) => Number(n) * 2",
    echo_async: "async (s) => { await new Promise(r => setTimeout(r, 10)); return 'echo:' + s; }",
};

const cases = [
    {
        name: "boot + print literal",
        script: "print('hello')",
        output: ["hello"],
    },
    {
        name: "arithmetic + f-string",
        script: "x = 21\nprint(f'answer = {x * 2}')",
        output: ["answer = 42"],
    },
    {
        name: "receive() drains pushEvent queue",
        script: "for _ in range(2):\n    print(receive())",
        events: ["one", "two"],
        output: ["one", "two"],
    },
    {
        name: "mainThreadModule sync handler",
        script: "from m import uppercase\nprint(uppercase('hi'))",
        main_thread: { m: ["uppercase"] },
        output: ["HI"],
    },
    {
        name: "mainThreadModule async handler (deferred host-call path)",
        script: "from m import echo_async\nprint(echo_async('ping'))",
        main_thread: { m: ["echo_async"] },
        output: ["echo:ping"],
    },
    {
        name: "async handler inline in f-string (regression guard for dispatch.rs host_yield fix)",
        script: "from m import echo_async\nprint(f'got:{echo_async(\"x\")}')",
        main_thread: { m: ["echo_async"] },
        output: ["got:echo:x"],
    },
];

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
