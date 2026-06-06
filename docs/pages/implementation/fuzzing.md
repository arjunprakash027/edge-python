---
title: "Fuzzing"
description: "Coverage-guided fuzzing of the lex, parse, and VM pipeline with cargo-afl on stable Rust."
---

## Overview

The fuzzer drives the full `lex -> parse -> VM` pipeline against mutated input, looking for panics, arithmetic overflow, and memory faults. It lives in [`compiler/fuzz-afl/`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/compiler/fuzz-afl) and is built on [cargo-afl](https://github.com/rust-fuzz/afl.rs) (AFL++), which on stable instruments through rustc's own LLVM SanitizerCoverage (`-sanitizer-coverage-trace-pc-guard`) and links the AFL++ runtime — so it runs on **stable Rust**, no nightly toolchain required. (AFL++'s own LLVM passes — CMPLOG, IJON — are an optional, nightly-only path the project does not use.)

The target runs the VM under the sandbox profile so runaway loops and allocations become a `VmErr` instead of a hang, and any real crash is a genuine bug rather than resource exhaustion. The harness tightens one field — `Limits { ops: 100_000, ..Limits::sandbox() }` — because the default 100M-op budget, while bounded, takes long enough that AFL would flag a legitimately-terminating loop (or wide recursion) as a hang; the smaller budget keeps each execution inside AFL's hang timeout while still reaching deep into the language. It also sets `strict_input = true` so `input()` raises instead of blocking on real stdin, which AFL feeds through shared memory. See [Limits and errors](/reference/limits-and-errors).

The build runs `--release`; `[profile.release]` sets `debug = "line-tables-only"` for `file:line` backtraces without the `dev` profile's heavier debuginfo. (cargo-afl forces `opt-level=3`, `debug-assertions`, and `overflow-checks` into `RUSTFLAGS` regardless of profile, so the checks the fuzzer relies on are always on.)

## Running it

```bash
cd compiler/fuzz-afl
./seeds.sh # generate corpus + dictionary from vm.json (once)
cargo afl build --release # instrument on stable, no nightly
cargo afl fuzz -i in -o out -x edge.dict target/release/afl-pipeline # runs until Ctrl-C; add -V 300 to stop after 300s

cargo afl whatsup out # status summary of the ./out campaign; run in another terminal while fuzzing
```

For a parallel run across the host cores, `./deploy.sh` builds, regenerates seeds, and launches one `-M` plus N-1 `-S` instances sharing one `out/`. It runs one instance per logical core by default; override with `JOBS`, and `DURATION` / `FRESH` / `TIMEOUT_MS` are optional too. The same target runs on a daily schedule in CI via [`.github/workflows/fuzzer.yml`](https://github.com/dylan-sutton-chavez/edge-python/tree/main/.github/workflows/fuzzer.yml) — which calls `deploy.sh` directly on the runner (no container) and fails the run on any saved crash.

For a long-running campaign in a container, `compose.yml` builds the image from `Dockerfile` and runs the same `deploy.sh`; findings persist in the `findings` volume mounted at `compiler/fuzz-afl/out/` instead of CI's 14-day artifact. It sets `restart: unless-stopped` so the campaign survives host reboots and only stops when you explicitly run `docker compose down`. It also sets `AFL_NO_AFFINITY=1`, since a container hides the host topology and AFL must not pin instances to cores it cannot see:

| Variable | Default | Description |
|----------|---------|-------------|
| `JOBS` | `$(nproc)` | Number of AFL instances (one per logical core). |
| `DURATION` | `0` | Campaign length in seconds. `0` runs until stopped. |
| `FRESH` | `0` | Set to `1` to delete `out/` and start a clean campaign. |
| `TIMEOUT_MS` | `5000` | Per-input hang threshold in ms. Should exceed the max bounded VM run. |

```bash
cd compiler/fuzz-afl
DURATION=3600 docker compose up --build -d # detached; same JOBS / FRESH / TIMEOUT_MS overrides apply

# Container runs in the background; stream raw deploy output (seed count, instance count, startup errors).
docker compose logs -f

# Watch the live campaign status once instances are running.
docker compose exec fuzzer bash
cd compiler/fuzz-afl
watch -n 10 cargo afl whatsup out # individual metrics per instance
watch -n 10 cargo afl whatsup -s out # aggregated summary only
```

Reusing the same `out/` resumes the campaign: AFL recalibrates the saved queue (the dry-run pass) before fuzzing, so `execs` sits at 0 for a while; delete it with `rm -rf out` for a clean start.

`deploy.sh` sets the bypass vars itself; a bare `cargo afl fuzz` under WSL needs `AFL_SKIP_CPUFREQ=1 AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1` prefixed to skip the core-pattern and CPU-governor checks.

Where findings land depends on how you launched: a bare `cargo afl fuzz` (no `-M`/`-S`) writes to `out/default/`, while `deploy.sh`, compose, and CI pass `-M m0` / `-S s1…`, so crashes and hangs land in `out/m0/`, `out/s1/`, etc. Reproduce one by piping it back into the target:

```bash
./target/release/afl-pipeline < out/m0/crashes/<id> # out/default/crashes/<id> for a bare single-instance run
```

## Inputs are generated, not committed

The seed corpus (`in/`) and the token dictionary (`edge.dict`) are derived from a single source of truth, `tests/cases/vm.json`, so they are gitignored and regenerated by `seeds.sh`:

- **`in/`**: one file per unique program `src` in the VM test fixtures, giving AFL valid starting points that already exercise most of the language.
- **`edge.dict`**: keywords, operators, and common builtins, so the byte mutator splices real tokens instead of discovering them blindly.

Only six files are tracked: `Cargo.toml`, `src/main.rs`, `seeds.sh`, `deploy.sh`, `Dockerfile`, and `compose.yml`. The corpus, dictionary, AFL output, and build artifacts are all reproducible.
