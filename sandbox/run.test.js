/*
Agnostic runner. Discovers capabilities via `<cap>/<cap>.json`; cases run through `sandbox/index.html` with `from <cap> import *\n` prepended.
*/

import { chromium } from "npm:playwright";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";

const root = new URL("../", import.meta.url).pathname;

/* Repo-root entries whose `<cap>/<cap>.json` corpus exists are treated as host capabilities. `HOSTCAP=<name>` narrows discovery to a single capability, used by the matrix-fanned CI to isolate per-shard work. */
const onlyCap = Deno.env.get("HOSTCAP");
const capabilities = readdirSync(root).filter((name) => {
    const full = root + name;
    if (!statSync(full).isDirectory()) return false;
    if (onlyCap && name !== onlyCap) return false;
    return existsSync(`${full}/${name}.json`);
});

const TYPES = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".wasm": "application/wasm",
    ".json": "application/json",
    ".svg": "image/svg+xml",
    ".py": "text/plain",
    ".css": "text/css",
};

async function runCapability(cap) {
    const cases = JSON.parse(readFileSync(`${root}${cap}/${cap}.json`, "utf-8"));

    const browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = [];
    page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
    page.on("pageerror", (e) => errors.push(e.message));

    /* Serve repo files from disk. External CDNs (runtime.edgepython.com) pass through. */
    await page.route("**/*", (route) => {
        const url = new URL(route.request().url());
        if (url.host !== "x") return route.continue();
        const path = root + url.pathname.slice(1);
        try {
            const ext = path.slice(path.lastIndexOf("."));
            return route.fulfill({
                body: readFileSync(path),
                contentType: TYPES[ext] ?? "application/octet-stream",
            });
        } catch {
            return route.fulfill({ status: 404 });
        }
    });

    /* Per-case mocks; uninstalled after each case so they don't leak into later ones. */
    const httpMocks = [];
    const wsMocks = [];
    const installMocks = async (c) => {
        for (const m of c.http_mocks ?? []) {
            const handler = (route) => route.fulfill({
                status: m.status ?? 200,
                contentType: m.contentType ?? "application/json",
                body: m.body ?? "",
            });
            await page.route(m.url, handler);
            httpMocks.push({ url: m.url, handler });
        }
        for (const m of c.ws_mocks ?? []) {
            const handler = (ws) => {
                if (m.echo) ws.onMessage((message) => ws.send(message));
            };
            await page.routeWebSocket(m.url, handler);
            wsMocks.push({ url: m.url, handler });
        }
    };
    const uninstallMocks = async () => {
        for (const { url, handler } of httpMocks.splice(0)) await page.unroute(url, handler);
        wsMocks.splice(0);
    };

    const failures = [];
    try {
        await page.goto(`http://x/sandbox/index.html?capability=${cap}`);
        await page.waitForFunction(() => typeof globalThis.runHostCase === "function", null, { timeout: 15000 });

        for (const [i, c] of cases.entries()) {
            await installMocks(c);
            const src = `from ${cap} import *\n${c.src}`;
            const result = await page.evaluate(
                ({ src, html }) => globalThis.runHostCase(src, html),
                { src, html: c.html },
            );
            await uninstallMocks();

            if (c.error) {
                if (!result.error || !result.error.includes(c.error)) {
                    failures.push(`[${cap} #${i}] expected error containing '${c.error}', got: ${result.error ?? "(none)"}`);
                }
                continue;
            }
            if (result.error) {
                failures.push(`[${cap} #${i}] unexpected error: ${result.error}`);
                continue;
            }
            const expected = c.output ?? [];
            if (JSON.stringify(result.output) !== JSON.stringify(expected)) {
                failures.push(`[${cap} #${i}] output mismatch\n  src: ${c.src.replaceAll("\n", " / ")}\n  expected: ${JSON.stringify(expected)}\n  got: ${JSON.stringify(result.output)}`);
            }
        }

        if (errors.length) failures.push(`[${cap}] console errors: ${errors.join(" | ")}`);
    } finally {
        await browser.close();
    }

    if (failures.length) throw new Error("\n" + failures.join("\n"));
}

for (const cap of capabilities) {
    Deno.test(`host capability: ${cap}`, () => runCapability(cap));
}
