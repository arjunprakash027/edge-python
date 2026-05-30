# Edge Python CI/CD

```
deno lint -> deno test ┐
check -> wasm -> runtime -> demo
```

| Workflow | Role |
|----------|------|
| `_check.yml` | `cargo shear` + `clippy` (host and wasm targets) |
| `_wasm.yml` | Builds and optimizes `compiler_lib.wasm`. On tags, attaches the `.wasm` to the GitHub Release |
| `_runtime_check.yml` | JS-side gate: `deno lint runtime/` + `deno test runtime/tests/` (Playwright + Chromium driving `createWorker` against the CDN-deployed wasm). Independent branch, runs in parallel with the Rust pipeline; only the CDN upload below blocks on it |
| `_runtime.yml` | Bundles `runtime/` + `compiler_lib.wasm` and deploys them to Cloudflare Pages |
| `_demo.yml` | Hashes `compiler_lib.wasm` into `version.json` (cache-busting) and deploys `demo/` to Cloudflare Pages |
| `cli.yml` | Standalone (not part of the pipeline above): builds and tests `cli/`; on `main` pushes also publishes the release binary + `cli/setup/` scripts (`install.sh`, `uninstall.sh`) to GitHub Pages |
| `host.yml` | Standalone: deno-lints and tests each host capability (`dom`, `network`, `storage`, `time`) in headless Chromium; on `main` pushes also deploys their ESM sources to Cloudflare Pages (`edge-python-host`) |
| `std.yml` | Standalone: clippy + build + optimize + test each stdpkg (`json`, `re`, `math` as wasm; `test` is pure Edge Python, so its steps skip the wasm build and only run the corpus); on `main` pushes also deploys the per-package `.wasm` to Cloudflare Pages (`edge-python-std`) |
| `docs.yml` | Standalone (triggered only by `docs/**` changes): `npm ci` + `next build` static export of the Nextra docs (`docs/out`, sitemap via `postbuild`); PRs build only, `main` pushes also deploy to Cloudflare Pages (`edge-python-docs`) |

## Cloudflare Pages

Five **Direct Upload** projects, Actions pushes prebuilt directories via `wrangler pages deploy`; Cloudflare doesn't clone or build.

| Project | Source | Production URL |
|---------|--------|----------------|
| `edge-python-demo` | `demo/` (wasm hashed for `version.json`, not bundled) | `https://edge-python-demo.pages.dev` |
| `edge-python-runtime` | `runtime/` + bundled `compiler_lib.wasm` | `https://edge-python-runtime.pages.dev` |
| `edge-python-host` | `host/<cap>/src/` for each capability, flattened to `<cap>/` | `https://edge-python-host.pages.dev` |
| `edge-python-std` | per-package optimized `.wasm` from `std/<pkg>/` | `https://edge-python-std.pages.dev` |
| `edge-python-docs` | `docs/out` (Nextra static export) | `https://edgepython.com` (custom domain; also `https://edge-python-docs.pages.dev`) |

All five deploys run **only on pushes to `main`** and are pinned to the production `main` branch in the matching workflow (`_runtime.yml` / `_demo.yml` / `host.yml` / `std.yml` / `docs.yml`). PRs and tags never deploy; the next `main` push refreshes the projects.

### Cloudflare and GitHub setup

```bash
# Wrangler CLI (Node 22+)
npx wrangler login
npx wrangler pages project create edge-python-demo --production-branch=main
npx wrangler pages project create edge-python-runtime --production-branch=main
npx wrangler pages project create edge-python-docs --production-branch=main
```

`edge-python-docs` serves `edgepython.com` (replacing the old Mintlify docs): after the first deploy, add `edgepython.com` as a custom domain on the project (Pages -> Custom domains) and remove it from Mintlify.

Repo secrets (*Settings -> Secrets and variables -> Actions*):

- `CLOUDFLARE_API_TOKEN`, `Account -> Cloudflare Pages -> Edit`. Create via dashboard: <https://dash.cloudflare.com/profile/api-tokens>.
- `CLOUDFLARE_ACCOUNT_ID`, from `npx wrangler whoami` or any dashboard sidebar.

Rotate: create new token -> update secret -> revoke old token.

## Releases

Pushing a `v*` tag triggers the pipeline; `_wasm.yml` uploads `compiler_lib.wasm` to the matching Release. Tag must match workspace version.

1. Bump `version` under `[workspace.package]` in root `Cargo.toml` (every crate inherits via `version.workspace = true`). Run `cargo check` to refresh `Cargo.lock`, commit.
2. Tag and push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

On tag push: `_check` lints, `_wasm` builds and optimizes the artifact and attaches it to a fresh Release with auto-generated notes. The CDN deploys (`_runtime` + `_demo`) do not run on tags; they already deployed from the preceding `main` push.

Nothing is published to crates.io, distribution is the `.wasm` on the Release. `starter-module` carries its own version and isn't bumped with the workspace.

Consumer crates pick up the release automatically: `compiler/Cargo.toml` declares `links = "compiler_lib"` and `compiler/build.rs` downloads `<repository>/releases/download/v<version>/compiler_lib.wasm` into `OUT_DIR`. Downstreams read `DEP_COMPILER_LIB_WASM` in their own `build.rs`, see [root README](../../README.md#consume-the-release-from-a-rust-host). Tag bumps flow via `cargo update`.

Gated behind the default-on `prebuilt` feature. Producer-side steps (`_check`, `_wasm`) pass `--no-default-features` to avoid fetching the asset that this same pipeline uploads later.
