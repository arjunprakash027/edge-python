# Edge Python Std CI/CD

```
lint -> test (matrix-fanned per stdpkg)
```

| Workflow | Role |
|----------|------|
| `pipeline.yml` | Orchestrator. Defines the package matrix once via YAML anchor (`&package-matrix`), aliased by both `_lint` and `_test`. Test is gated on lint |
| `_lint.yml` | `cargo clippy` against the stdpkg's `src/` on `wasm32-unknown-unknown` with `-D warnings` (cdylib only, not `--all-targets`) |
| `_test.yml` | Builds the package's `.wasm`, then drives its corpus through `sandbox/` in cached Chromium |

Triggers: push to `main`, tags `v*`, PRs against `main`.

## Adding a stdpkg

The list lives **in one place**: the anchored `strategy` block on the `lint` job in `pipeline.yml`. Edit the array:

```yaml
lint:
  strategy: &package-matrix
    matrix:
      package: [json, re] # <- edit only here; `test` aliases via *package-matrix
```

GitHub Actions supports YAML anchors (since Sep 2025), so the alias on the `test` job picks up the change automatically. The reusable workflows run against `${{ inputs.package }}/src/` for clippy and pass `STDPKG=${{ inputs.package }}` to `sandbox/run.test.js` so that shard's Chromium only drives its own corpus.

## Caches

| Cache | Path | Used by | Key |
|-------|------|---------|-----|
| Cargo | `~/.cargo/registry`, `~/.cargo/git`, `<pkg>/target` | `_lint.yml`, `_test.yml` | per-package `Cargo.toml` hash; invalidates on dep changes |
| Deno modules | `~/.cache/deno` | `_test.yml` | `deno.json` / `deno.lock` hash |
| Playwright Chromium | `~/.cache/ms-playwright` | `_test.yml` | `runner.os + chromium`; ~150MB binary, hit makes `playwright install` a no-op |

## Local parity

```bash
# Lint
( cd json && cargo clippy --release --target wasm32-unknown-unknown -- -D warnings )

# One-time setup
deno run -A npm:playwright install chromium

# Build + test one package
( cd json && cargo build --release --target wasm32-unknown-unknown )
STDPKG=json deno test --allow-all sandbox/

# Or test every package the runner discovers (no STDPKG)
deno test --allow-all sandbox/
```
