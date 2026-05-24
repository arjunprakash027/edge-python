# Edge Python Std CI/CD

```
lint -> wasm -> deploy (lint and wasm fan out per stdpkg)
```

| Workflow | Role |
|----------|------|
| `pipeline.yml` | Orchestrator. Declares the package matrix once via YAML anchor (`&package-matrix`), aliased by `wasm`. Chains `lint -> wasm -> deploy`. |
| `_lint.yml` | `cargo clippy` on the stdpkg's `src/` for `wasm32-unknown-unknown` with `-D warnings` (cdylib only, not `--all-targets`). |
| `_wasm.yml` | Builds the `.wasm` (nightly + `build-std`), shrinks it with `wasm-opt`, drives its corpus through `tests/` in Chromium, then uploads the artifact. |
| `_deploy.yml` | Downloads every package's `.wasm` and publishes them to Cloudflare Pages. |

Triggers: push to `main`, tags `v*`, PRs against `main`. `lint` and `wasm` run on all of these; `deploy` runs only on pushes to `main`, so PRs and tags never publish (the next `main` push refreshes the CDN).

## Adding a stdpkg

The list lives **in one place**: the anchored `strategy` block on the `lint` job in `pipeline.yml`. Edit the array:

```yaml
lint:
  strategy: &package-matrix
    matrix:
      package: [json, re] # edit only here; wasm aliases via *package-matrix
```

GitHub Actions supports YAML anchors, so the alias on `wasm` picks up the change automatically. The reusable workflows run clippy against `${{ inputs.package }}/src/` and pass `STDPKG=${{ inputs.package }}` to `tests/std.test.js` so each shard's Chromium drives only its own corpus.

## Caches

| Cache | Path | Used by | Key |
|-------|------|---------|-----|
| Cargo (stable) | `~/.cargo/{registry,git}`, `<pkg>/target` | `_lint.yml` | `cargo-stable-`, per-package `Cargo.toml` hash |
| Cargo (nightly) | `~/.cargo/{registry,git}`, `<pkg>/target` | `_wasm.yml` | `cargo-nightly-`, per-package `Cargo.toml` hash |
| Deno modules | `~/.cache/deno` | `_wasm.yml` | `deno.json` / `deno.lock` hash |
| Playwright Chromium | `~/.cache/ms-playwright` | `_wasm.yml` | runner OS + `chromium` (~150MB, hit skips the download) |

Lint (stable clippy) and wasm (nightly `build-std`) use distinct cache prefixes so their incompatible `target/` builds never collide on one key.

## Deploy

`_deploy.yml` runs only on pushes to `main`. It downloads each `wasm-<pkg>` artifact into `_site/js/`, then runs `wrangler pages deploy _site` pinned to the production `--branch=main`. No checkout is needed: unlike the host runtime (which bundles JS sources), this project serves only the bare `.wasm` at `runtime.edgepython.com/js/<pkg>.wasm`. Credentials come from the `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` repo secrets.

## Local parity

```bash
# Lint
( cd json && cargo clippy --release --target wasm32-unknown-unknown -- -D warnings )

# One-time setup
deno run -A npm:playwright install chromium

# Build + test one package
( cd json && cargo build --release --target wasm32-unknown-unknown )
STDPKG=json deno test --allow-all tests/

# Or test every package the runner discovers (no STDPKG)
deno test --allow-all tests/
```
