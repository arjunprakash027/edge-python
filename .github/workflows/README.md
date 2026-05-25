# Edge Python Host CI/CD

```
lint -> test -> deploy (lint and test fan out per capability)
```

| Workflow | Role |
|----------|------|
| `pipeline.yml` | Orchestrator. Defines the capability matrix once via YAML anchor (`&capability-matrix`), aliased by both `_lint` and `_test`. Chains `lint -> test -> deploy` |
| `_lint.yml` | `deno lint` against the capability's `src/` |
| `_test.yml` | Deno + cached Chromium; runs `deno test --allow-all tests/` with `HOSTCAP=<capability>` so the shared runner only drives that capability's corpus |
| `_deploy.yml` | Flattens every capability's `src/` into `_site/<cap>` and publishes it to Cloudflare Pages |

Triggers: push to `main`, tags `v*`, PRs against `main`. `lint` and `test` run on all of these; `deploy` runs only on pushes to `main`, so PRs and tags never publish (the next `main` push refreshes the CDN).

## Adding a capability

The list lives **in one place**: the anchored `strategy` block on the `lint` job in `pipeline.yml`. Edit the array:

```yaml
lint:
  strategy: &capability-matrix
    matrix:
      capability: [dom, forms] # <- edit only here; `test` aliases via *capability-matrix
```

GitHub Actions supports YAML anchors (since Sep 2025), so the alias on the `test` job picks up the change automatically. `_lint.yml` runs against `${{ inputs.capability }}/src/`; `_test.yml` drives `tests/` with `HOSTCAP=${{ inputs.capability }}` so the shared runner narrows to that capability's `<name>.json` corpus, no per-capability config beyond the array entry.

## Caches

| Cache | Path | Used by | Key |
|-------|------|---------|-----|
| Deno modules | `~/.cache/deno` | `_lint.yml`, `_test.yml` | `deno.json` / `deno.lock` hash; invalidates on dep changes |
| Playwright Chromium | `~/.cache/ms-playwright` | `_test.yml` | `runner.os + chromium`; ~150MB binary, hit makes `playwright install` a no-op |

## Deploy

`_deploy.yml` runs only on pushes to `main`. It checks out the tree and flattens each capability's `src/` into `_site/<cap>` (dropping the `src` segment), then runs `wrangler pages deploy _site` pinned to the production `--branch=main` for the `edge-python-host` project. Unlike std (which uploads bare `.wasm` artifacts and needs no checkout), the host serves its ESM sources directly, so consumers import `https://host.edgepython.com/<cap>/index.js`. The assembly globs `*/src`, so any new capability dir publishes without editing the deploy workflow. Credentials come from the `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` repo secrets.

## Local parity

```bash
# Lint
deno lint dom/src/

# One-time setup
deno run -A npm:playwright install chromium

# Test (drives dom/dom.json through the shared harness)
HOSTCAP=dom deno test --allow-all tests/
```
