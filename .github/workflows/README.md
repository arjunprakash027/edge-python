# Edge Python Host CI/CD

```
lint -> test (matrix-fanned per capability)
```

| Workflow | Role |
|----------|------|
| `pipeline.yml` | Orchestrator. Defines the capability matrix once via YAML anchor (`&capability-matrix`), aliased by both `_lint` and `_test`. Test is gated on lint |
| `_lint.yml` | `deno lint` against the capability's `src/` |
| `_test.yml` | Deno + cached Chromium; runs `deno test --allow-all` against the capability's `tests/` |

Triggers: push to `main`, tags `v*`, PRs against `main`.

## Adding a capability

The list lives **in one place**: the anchored `strategy` block on the `lint` job in `pipeline.yml`. Edit the array:

```yaml
lint:
  strategy: &capability-matrix
    matrix:
      capability: [dom, forms] # ← edit only here; `test` aliases via *capability-matrix
```

GitHub Actions supports YAML anchors (since Sep 2025), so the alias on the `test` job picks up the change automatically. The reusable workflows run against `${{ inputs.capability }}/src/` and `${{ inputs.capability }}/tests/` — no per-capability config beyond the array entry.

## Caches

| Cache | Path | Used by | Key |
|-------|------|---------|-----|
| Deno modules | `~/.cache/deno` | `_lint.yml`, `_test.yml` | `deno.json` / `deno.lock` hash; invalidates on dep changes |
| Playwright Chromium | `~/.cache/ms-playwright` | `_test.yml` | `runner.os + chromium`; ~150MB binary, hit makes `playwright install` a no-op |

## Local parity

```bash
# Lint
deno lint dom/src/

# One-time setup
deno run -A npm:playwright install chromium

# Test
cd dom && deno test --allow-all tests/
```
