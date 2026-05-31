# Edge Python CI/CD

One workflow, [`main.yml`](workflows/main.yml), drives the whole monorepo, so the Actions tab shows a single "CI / CD" run per push/PR with every job as a node in one graph. Each package's logic lives in a **composite action** under [`actions/`](actions); `main.yml` only wires the dependency graph. The composite actions are not workflows and do not appear in the Actions tab.

`compiler-check`, `runtime-lint` and `cli-lint` start at t=0. If any job fails the dependents never run (`needs:`), so a red build stops the deploys. `docs-build` runs on every event so a PR that breaks the docs is caught early; the deploy chain (`cdn → demo → docs-deploy`) is gated to `main` and pinned to the production branch. The `host` and `std` matrices use `fail-fast: false` so one capability / package failure still reports the others. `cli-lint` runs clippy + check once, then the heavy per-target `cli-release` build runs; `cli-test` waits on `host`, `std`, and the release artifacts.

## Composite actions

| Action | Inputs | Role |
|--------|--------|------|
| `compiler` | `mode: check\|build` | check: `cargo shear` + clippy (host and wasm targets). build: build + optimize `compiler_lib.wasm`, test, upload the artifact (and attach it to the GitHub Release on tags) |
| `runtime` | `mode: lint\|test` | lint: `deno lint runtime/`. test: Deno + Playwright suite (Chromium driving `createWorker` against the CDN wasm) |
| `host` | `capability` | Deno-lints and smoke-tests one capability (`dom`, `network`, `storage`, `time`) in headless Chromium. All JS, no release |
| `std` | `package` | Clippy + build + optimize + corpus test for one stdpkg (`json`, `re`, `math` as wasm; `test` is pure Edge Python, so it skips the wasm build and only runs the corpus). Stages `<pkg>.wasm` / `<pkg>.py`. No release |
| `cli` | `mode: lint\|release\|test`, `target` | lint: `cargo clippy -D warnings` + `cargo check` (once). release: `cargo build --release` per target → tarball artifact. test: `cargo test` (drives a real Chromium) |
| `demo` | CF token + account | Hashes deps into `version.json` (cache-busting), builds Tailwind, deploys `demo/` to `edge-python-demo` |
| `docs` | `mode: build\|deploy`, CF token + account | build: `npm ci` + `next build` static export (`docs/out`, sitemap via `postbuild`), upload artifact. deploy: pull artifact + push to `edge-python-docs` |
| `cdn-deploy` | CF token + account | Pulls every artifact, stages `./compiler ./runtime ./std ./host ./cli`, one `wrangler pages deploy` to `edge-python-cdn` |

## Cloudflare Pages

Three **Direct Upload** projects. Actions push prebuilt directories via `wrangler pages deploy`; Cloudflare doesn't clone or build.

| Project | Source | Production URL |
|---------|--------|----------------|
| `edge-python-cdn` | `_site/{compiler,runtime,std,host,cli}` (consolidates the old per-package `-runtime` / `-host` / `-std` projects) | `https://edge-python-cdn.pages.dev` |
| `edge-python-demo` | `demo/` (wasm hashed for `version.json`, not bundled) | `https://edge-python-demo.pages.dev` |
| `edge-python-docs` | `docs/out` (Nextra static export) | `https://edgepython.com` (custom domain; also `https://edge-python-docs.pages.dev`) |

All deploys run **only on pushes to `main`** and are pinned to the production `main` branch. PRs and tags never deploy; the next `main` push refreshes the projects.

### Cloudflare and GitHub setup

```bash
# Wrangler CLI (Node 22+)
npx wrangler login
npx wrangler pages project create edge-python-cdn --production-branch=main
npx wrangler pages project create edge-python-demo --production-branch=main
npx wrangler pages project create edge-python-docs --production-branch=main
```

`edge-python-docs` serves `edgepython.com` (replacing the old Mintlify docs): after
the first deploy, add `edgepython.com` as a custom domain on the project
(Pages -> Custom domains) and remove it from Mintlify.

Repo secrets (*Settings -> Secrets and variables -> Actions*):

- `CLOUDFLARE_API_TOKEN`, `Account -> Cloudflare Pages -> Edit`. Create via dashboard: <https://dash.cloudflare.com/profile/api-tokens>.
- `CLOUDFLARE_ACCOUNT_ID`, from `npx wrangler whoami` or any dashboard sidebar.

Rotate: create new token -> update secret -> revoke old token.

## Releases

Pushing a `v*` tag runs the pipeline; the `compiler` build job uploads `compiler_lib.wasm` to the matching Release. Tag must match workspace version.

1. Bump `version` under `[workspace.package]` in root `Cargo.toml` (every crate inherits via `version.workspace = true`). Run `cargo check` to refresh `Cargo.lock`, commit.
2. Tag and push:

```bash
git tag v0.1.0
git push origin v0.1.0
```

On tag push: `compiler-check` lints, then the `compiler` build job optimizes the artifact and attaches it to a fresh Release with auto-generated notes. The CDN, demo and docs deploys do not run on tags; they already deployed from the preceding `main` push.

Nothing is published to crates.io, distribution is the `.wasm` on the Release. `starter-module` carries its own version and isn't bumped with the workspace.

Consumer crates pick up the release automatically: `compiler/Cargo.toml` declares `links = "compiler_lib"` and `compiler/build.rs` downloads `<repository>/releases/download/v<version>/compiler_lib.wasm` into `OUT_DIR`. Downstreams read `DEP_COMPILER_LIB_WASM` in their own `build.rs`, see [root README](../../README.md#consume-the-release-from-a-rust-host). Tag bumps flow via `cargo update`.

Gated behind the default-on `prebuilt` feature. Producer-side compiler steps pass `--no-default-features` to avoid fetching the asset that this same pipeline uploads later.
