# Edge Python CI/CD

```
check -> wasm -> runtime -> demo
```

| Workflow | Role |
|----------|------|
| `_check.yml`   | `cargo shear` + `clippy` (host and wasm targets) |
| `_wasm.yml`    | Builds and optimizes `compiler_lib.wasm`. On tags, attaches the `.wasm` to the GitHub Release |
| `_runtime.yml` | Bundles `runtime/` + `compiler_lib.wasm` and deploys them to Cloudflare Pages |
| `_demo.yml`    | Deploys `demo/` to Cloudflare Pages |

## Cloudflare Pages

Two projects, both in **Direct Upload** mode, where actions pushes prebuilt directories via `wrangler pages deploy`; Cloudflare does not clone or build the repo.

| Project | Source | Production URL |
|---------|--------|----------------|
| `edge-python-demo` | `demo/` + downloaded `compiler_lib.wasm` | `https://edge-python-demo.pages.dev` |
| `edge-python-cdn`  | `runtime/` + downloaded `compiler_lib.wasm` | `https://edge-python-cdn.pages.dev`  |

Both projects produce one preview URL per branch / PR.

### Cloudflare and GitHub Setup

```bash
# Wrangler CLI (requires Node 22+)
npx wrangler login
npx wrangler pages project create edge-python-demo --production-branch=main
npx wrangler pages project create edge-python-cdn  --production-branch=main
```

Then add the secrets at *Settings -> Secrets and variables -> Actions*:

- `CLOUDFLARE_API_TOKEN` — token with `Account -> Cloudflare Pages -> Edit` permission. **Must be created via dashboard** at <https://dash.cloudflare.com/profile/api-tokens>.
- `CLOUDFLARE_ACCOUNT_ID` — printed by `npx wrangler whoami`, or shown in the right sidebar of any Cloudflare dashboard page.

### Rotate the API token

1. Create a new token at <https://dash.cloudflare.com/profile/api-tokens>.
2. Update `CLOUDFLARE_API_TOKEN` in repo secrets.
3. Revoke the old token on the same Cloudflare page.

## Releases

Pushing a `v*` tag triggers the pipeline and `_wasm.yml` uploads `compiler_lib.wasm` to the matching GitHub Release. Tag name must match the workspace version.

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml`. Every crate inherits via `version.workspace = true`, so this single line covers `edge-python`, `wasm-abi`, `wasm-pdk`, and `wasm-pdk-macros` at once. Run `cargo check` to refresh `Cargo.lock`, then commit.

2. Tag and push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

**On a tag push, `_check` lints, `_wasm` builds and optimizes `compiler_lib.wasm` then attaches it to a fresh GitHub Release with auto-generated notes from commits since the previous tag, and `_runtime` + `_demo` redeploy the CDN and the playground with the new binary.**

Nothing is published to crates.io — distribution is the `.wasm` artifact attached to the Release. The `starter-module` example carries its own `version` and is intentionally not bumped with the workspace.

Rust crates that consume the release pick it up automatically: `compiler/Cargo.toml` declares `links = "compiler_lib"` and `compiler/build.rs` downloads `<repository>/releases/download/v<version>/compiler_lib.wasm` into `OUT_DIR`. A downstream that depends on `edge-python` reads the resulting path from `DEP_COMPILER_LIB_WASM` in its own `build.rs` — see the consumer pattern in the [root README](../../README.md#consume-the-release-from-a-rust-host). Tag bumps in this repo flow through to consumers via `cargo update`.

The download is gated behind the default-on `prebuilt` feature. Producer-side steps in this pipeline (host clippy in `_check.yml`, host tests in `_wasm.yml`) pass `--no-default-features` so they don't try to fetch a release asset that, on a first-tag publish, only gets uploaded later in this same pipeline.
