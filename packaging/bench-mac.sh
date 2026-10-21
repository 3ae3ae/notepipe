#!/usr/bin/env bash
# macOS Apple Silicon 基准：速度 + 功耗（powermetrics 需要 sudo 凭据）。
# 用法: packaging/bench-mac.sh <video> [output-dir]
set -uo pipefail

VIDEO="${1:?usage: bench-mac.sh <video> [outdir]}"
OUT="$(mktemp -d "${2:-/tmp/notepipe-bench}.XXXXXX")"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/target/release/notepipe"
PM_LOG="$(mktemp -t notepipe-pm)"
RUN_LOG="$(mktemp -t notepipe-run)"
TIME_LOG="$(mktemp -t notepipe-time)"
trap 'rm -f "$PM_LOG" "$RUN_LOG" "$TIME_LOG"' EXIT

test -x "$BIN" || { echo "missing $BIN (run cargo build --release first)" >&2; exit 1; }
sudo -v || { echo "sudo required for powermetrics sampling" >&2; exit 1; }

# Average one powermetrics power line (mW) over the run.
avg() { awk -v key="$1" '$0 ~ "^" key " Power" {s+=$3; n++} END {if (n) printf "%.2f", s/n; else print 0}' "$PM_LOG"; }

run_one() {
  local label="$1"; shift
  echo "=== $label"
  rm -rf "$OUT"
  sudo powermetrics -s cpu_power,gpu_power,ane_power -i 1000 > "$PM_LOG" 2>/dev/null &
  local pm_pid=$!
  sleep 1
  # /usr/bin/time -l reports the peak resident set size of the run, including waited children.
  /usr/bin/time -l "$BIN" "$VIDEO" -o "$OUT" --no-llm --json "$@" > "$RUN_LOG" 2> "$TIME_LOG"
  local status=$?
  sudo kill "$pm_pid" 2>/dev/null
  wait "$pm_pid" 2>/dev/null
  if [ "$status" -ne 0 ]; then
    echo "FAILED (exit $status); excluded from results"
    tail -5 "$TIME_LOG" | sed 's/^/    /'
    return
  fi
  local wall rss cpu gpu ane
  wall=$(grep '"type":"done"' "$RUN_LOG" | tail -1 | sed -n 's/.*"elapsed_secs":\([0-9.]*\).*/\1/p')
  rss=$(awk '/maximum resident set size/ {printf "%.0f", $1/1048576}' "$TIME_LOG")
  cpu=$(avg CPU); gpu=$(avg GPU); ane=$(avg ANE)
  # 能量 = 平均功率 × 时长
  awk -v w="${wall:-0}" -v c="$cpu" -v g="$gpu" -v a="$ane" -v r="${rss:-0}" 'BEGIN {
    printf "wall=%.1fs avg_cpu=%smW avg_gpu=%smW avg_ane=%smW E_cpu=%.0fJ E_gpu=%.0fJ E_ane=%.0fJ peak_rss=%sMB\n",
      w, c, g, a, c*w/1000, g*w/1000, a*w/1000, r }'
}

run_one "coreml-qwen3-1.7b" --provider coreml --asr-model qwen3-1.7b
run_one "coreml-qwen3-0.6b" --provider coreml --asr-model qwen3-0.6b
run_one "coreml-whisper"    --provider coreml --asr-model whisper
run_one "gpu-llama"         --provider gpu
run_one "cpu-llama"         --provider cpu
echo "=== done"
