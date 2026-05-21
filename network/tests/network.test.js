/* Smoke test: serves network/ from disk, mocks all outgoing HTTP/WS via page.route, loads web/index.html in Chromium, drives the example, and fails if any console error fires. */

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

Deno.test("demo: boot + interactions produce no console errors", async () => {
    const browser = await chromium.launch();
    const page = await browser.newPage();
    const errors = [];

    page.on("console", (msg) => { if (msg.type() === "error") errors.push(msg.text()); });
    page.on("pageerror", (e) => errors.push(e.message));

    /* Mock httpbin to avoid hitting the network in CI. */
    await page.route("**/httpbin.org/**", (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path === "/json") {
            return route.fulfill({ contentType: "application/json", body: '{"slideshow":{"title":"Sample"}}' });
        }
        if (path === "/uuid") {
            return route.fulfill({ contentType: "application/json", body: '{"uuid":"00000000-0000-0000-0000-000000000000"}' });
        }
        return route.fulfill({ status: 404 });
    });

    /* Local files (network/web, network/src, static). External CDNs (runtime.edgepython.com, cdn.tailwindcss.com) pass through. */
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
