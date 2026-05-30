# Edge Python Docs

## Local Development
Install dependencies

```sh
npm install
```

To run locally:

```sh
npm run dev
```

Submit a PR to Edge Python.

## Note on dev speed

In dev (`npm run dev`), the first visit to each page takes a few seconds because
Next.js compiles routes **on demand** (slower here since the repo lives on the
Windows drive mounted in WSL, `/mnt/c`). Once a page is compiled, navigation is
instant.

This does **not** happen in production: `npm run build` pre-renders every page at
build time (Nextra is static/SSG), so the deployed site serves pre-built HTML and all
navigation is instant.

