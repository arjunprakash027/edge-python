/*
Agnostic runner. Discovers stdpkgs by walking the repo root for `<name>/<name>.json` corpora; each case runs through `sandbox/index.html` with `from <name> import *\n` prepended. Adding a stdpkg means dropping its folder plus a sibling `<name>.json` — no edits here.
*/

import { chromium } from "npm:playwright";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";

const root = new URL("../", import.meta.url).pathname;

/* Repo-root entries whose `<name>/<name>.json` corpus exists are treated as stdpkgs. */
const packages = readdirSync(root).filter((name) => {
    const full = root + name;
    if (!statSync(full).isDirectory()) return false;
    return existsSync(`${full}/${name}.json`);
});

const TYPES = {
    ".html": "text/html",
    ".wasm": "application/wasm",
    ".json": "application/json",
};

async function runPackage(pkg) {
    const wasmPath = `${root}${pkg}/target/wasm32-unknown-unknown/release/${pkg}.wasm`;
    if (!existsSync(wasmPath)) {
        throw new Error(`built artifact not found for '${pkg}' at ${wasmPath}\nrun (from ${pkg}/): cargo build --release --target wasm32-unknown-unknown`);
    }

    const cases = JSON.parse(readFileSync(`${root}${pkg}/${pkg}.json`, "utf-8"));

    const browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = [];
    page.on("console", (msg) => { if (msg.type() === "error") errors.push(msg.text()); });
    page.on("pageerror", (e) => errors.push(e.message));

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

    const failures = [];
    try {
        await page.goto(`http://x/sandbox/index.html?packages=${pkg}`);
        await page.waitForFunction(() => typeof globalThis.runEdgePython === "function", null, { timeout: 15000 });

        for (const [i, c] of cases.entries()) {
            const src = `from ${pkg} import *\n${c.src}`;
            const result = await page.evaluate((s) => globalThis.runEdgePython(s), src);

            if (c.error) {
                if (!result.error || !result.error.includes(c.error)) {
                    failures.push(`[${pkg} #${i}] expected error containing '${c.error}', got: ${result.error ?? "(none)"}`);
                }
                continue;
            }

            if (result.error) {
                failures.push(`[${pkg} #${i}] unexpected error: ${result.error}`);
                continue;
            }

            const expected = c.output ?? [];
            if (JSON.stringify(result.output) !== JSON.stringify(expected)) {
                failures.push(`[${pkg} #${i}] output mismatch\n  src: ${c.src}\n  expected: ${JSON.stringify(expected)}\n  got: ${JSON.stringify(result.output)}`);
            }
        }

        if (errors.length) failures.push(`[${pkg}] console errors: ${errors.join(" | ")}`);
    } finally {
        await browser.close();
    }

    if (failures.length) throw new Error("\n" + failures.join("\n"));
}

for (const pkg of packages) {
    Deno.test(`stdpkg: ${pkg}`, () => runPackage(pkg));
}
