/*
Drives <edge-python> through index.html, comparing #app to each runtime.json case.
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

    let script = "";
    await page.route("**/*", (r) => {
        const u = new URL(r.request().url());
        if (u.host !== "localhost") return r.continue(); // CDN wasm passes through
        if (u.pathname === "/case.py") return r.fulfill({ contentType: "text/x-python", body: script });
        const ext = u.pathname.slice(u.pathname.lastIndexOf("."));
        try { return r.fulfill({ contentType: TYPES[ext] ?? "application/octet-stream", body: readFileSync(REPO + u.pathname.slice(1)) }); }
        catch { return r.fulfill({ status: 404 }); }
    });
    await page.goto("http://localhost/runtime/tests/index.html");

    try {
        for (const c of cases) {
            script = PRELUDE + c.script; // served as /case.py
            errors.length = 0;
            const got = await page.evaluate(async () => {
                document.querySelectorAll("edge-python").forEach((e) => e.remove());
                const app = document.querySelector("#app");
                app.textContent = "";
                const el = document.createElement("edge-python");
                el.setAttribute("entry", "/case.py");
                el.setAttribute("packages", "./app/packages.json");
                document.body.appendChild(el);
                const end = Date.now() + 30000;
                while (!app.textContent && Date.now() < end) await new Promise((res) => setTimeout(res, 50));
                return app.textContent;
            });
            if (got !== c.expect) {
                throw new Error(`script:\n${c.script}\n  got:  ${JSON.stringify(got)}\n  want: ${JSON.stringify(c.expect)}\n  errors: ${errors.join(" | ") || "(none)"}`);
            }
        }
    } finally {
        await browser.close();
    }
});
