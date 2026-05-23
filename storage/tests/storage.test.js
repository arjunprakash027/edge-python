/* Smoke test: serves storage/, drives web/index.html in Chromium, fails on console error. localStorage and IndexedDB are native browser APIs, no mocking needed. */

import { chromium } from "npm:playwright";
import { readFileSync } from "node:fs";

const root = new URL("../", import.meta.url).pathname;

const TYPES = {
    ".html": "text/html",
    ".js": "text/javascript",
    ".svg": "image/svg+xml",
    ".py": "text/plain",
    ".css": "text/css",
    ".json": "application/json",
};

Deno.test("demo: boot + storage operations produce no console errors", async () => {
    const browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = [];

    page.on("console", (msg) => { if (msg.type() === "error") errors.push(msg.text()); });
    page.on("pageerror", (e) => errors.push(e.message));

    await page.route("**/*", (route) => {
        const url = new URL(route.request().url());
        if (url.host !== "x") return route.continue();
        const ext = url.pathname.slice(url.pathname.lastIndexOf("."));
        try {
            return route.fulfill({
                body: readFileSync(root + url.pathname.slice(1)),
                contentType: TYPES[ext] ?? "application/octet-stream",
            });
        } catch {
            return route.fulfill({ status: 404 });
        }
    });

    try {
        await page.goto("http://x/web/index.html");

        await page.waitForFunction(
            () => document.querySelector("#output")?.textContent.includes("done"),
            null,
            { timeout: 15000 },
        );

        if (errors.length) {
            throw new Error("console errors:\n  " + errors.join("\n  "));
        }
    } finally {
        await browser.close();
    }
});
