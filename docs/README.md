# Edge Python Docs

## Local Development

Install dependencies and update `package-lock.json`:

```sh
npm install
```

To run locally:

```sh
npm run dev
```

## Note on dev speed

In dev (`npm run dev`), the first visit to each page takes a few seconds because Next.js compiles routes **on demand** (slower here since the repo lives on the Windows drive mounted in WSL, `/mnt/c`). Once a page is compiled, navigation is instant.

This does **not** happen in production: `npm run build` pre-renders every page at build time (Nextra is static/SSG), so the deployed site serves pre-built HTML and all navigation is instant.

## Runnable playgrounds

Any `python` code block immediately followed by a `text Output` block is upgraded into an interactive, editable `<Playground>` — an in-page editor with a **Run** button that executes the snippet in the real Edge Python runtime (no server round-trip).

Write the pair like this in any `.md`/`.mdx` page:

````md
```python
print("Hello from Python")
```

```text Output
Hello from Python
```
````

## Build & deploy

The site is a **static export** (`output: 'export'` in `next.config.js`), so `npm run build` emits a fully pre-rendered `out/` directory — no Node server in production.

```sh
npm run build      # → out/  (also runs scripts/sitemap.mjs via postbuild)
```
