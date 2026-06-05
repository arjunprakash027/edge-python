#!/usr/bin/env bash
# Monitor a running campaign from another shell with: cargo afl whatsup out
set -euo pipefail
cd "$(dirname "$0")"

CPU_PERCENT="${CPU_PERCENT:-75}"
DURATION="${DURATION:-0}"
FRESH="${FRESH:-0}"
TIMEOUT_MS="${TIMEOUT_MS:-5000}" # > max bounded run, so slow-but-terminating inputs aren't false hangs

export AFL_SKIP_CPUFREQ=1
export AFL_I_DONT_CARE_ABOUT_MISSING_CRASHES=1

total=$(nproc)
jobs=$(( total * CPU_PERCENT / 100 ))
(( jobs < 1 )) && jobs=1
echo "cores: $total, using ${CPU_PERCENT}% -> $jobs instance(s)"

# Regenerate the seed corpus / dictionary if absent, then build the instrumented target.
[ -d in ] && [ -n "$(ls -A in 2>/dev/null)" ] || ./seeds.sh
cargo afl build

[ "$FRESH" = "1" ] && rm -rf out
mkdir -p logs

# -V time-box only when DURATION > 0.
vflag=()
(( DURATION > 0 )) && vflag=(-V "$DURATION")

pids=()
cleanup() { kill "${pids[@]}" 2>/dev/null || true; }
trap cleanup INT TERM EXIT

# One main (deterministic) instance, the rest secondaries (havoc); all share -o out and sync.
cargo afl fuzz "${vflag[@]}" -t "$TIMEOUT_MS" -i in -o out -x edge.dict -M m0 target/debug/afl-pipeline >logs/m0.log 2>&1 &
pids+=($!)
for i in $(seq 1 $(( jobs - 1 ))); do
  cargo afl fuzz "${vflag[@]}" -t "$TIMEOUT_MS" -i in -o out -x edge.dict -S "s$i" target/debug/afl-pipeline >"logs/s$i.log" 2>&1 &
  pids+=($!)
done

echo "running $jobs instance(s); logs in ./logs, status: cargo afl whatsup out"
wait
