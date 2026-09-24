#!/usr/bin/env bash
# Records a profiled play session of cw-client for frame-time analysis.
#
#   ./profile.sh                    build, play, then summarise
#   ./profile.sh --samply           also record a CPU sampling profile (needs `cargo install samply`)
#   ./profile.sh --debug-overlay    build with the debug overlay
#   ./profile.sh -- --quit-after 60 anything after `--` goes to cw-client
#
# Output goes to profiles/<timestamp>/:
#   trace.csv    one row per frame: wall time, update dt, every phase and lock wait (ms)
#   stats.log    5 s summaries, slow frames, worker operations over NOTE_MS (default 4 ms),
#                World lock waits over WAIT_MS (default 1 ms) with the worker holds that caused
#                them, per-holder totals at the end
#   meta.txt     commit, machine, display options
#   cpu.json.gz  (with --samply) open at https://profiler.firefox.com
#
# Quit through the game (menu or closing the window), not Ctrl-C, so the trace is flushed.
set -euo pipefail

cd "$(dirname "$0")"

samply=0
features=()
client_args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --samply) samply=1 ;;
        --debug-overlay) features=(--features debug-overlay) ;;
        -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        --) shift; client_args=("$@"); break ;;
        *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
    esac
    shift
done

if [ $samply -eq 1 ] && ! command -v samply >/dev/null 2>&1; then
    echo "samply not found; install it with: cargo install samply" >&2
    exit 1
fi

game_dir="${CW_GAME_DIR:-$PWD/game}"
if [ ! -d "$game_dir" ]; then
    echo "game folder not found at $game_dir (set CW_GAME_DIR)" >&2
    exit 1
fi

echo "== building cw-client (release${features:+, ${features[*]}})"
cargo build -p cw-client --release ${features[@]+"${features[@]}"}

out="profiles/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"

{
    echo "date: $(date)"
    echo "commit: $(git rev-parse --short HEAD 2>/dev/null || echo unknown)$(git diff --quiet 2>/dev/null || echo ' (uncommitted changes)')"
    echo "machine: $(uname -a)"
    echo "features: ${features[*]:-none}"
    echo "client args: ${client_args[*]:-none}"
    if [ -f "$game_dir/options.cfg" ]; then
        echo "options.cfg:"
        sed 's/^/  /' "$game_dir/options.cfg"
    fi
} > "$out/meta.txt"

export CW_GAME_DIR="$game_dir"
export CW_CLIENT_TRACE="$PWD/$out/trace.csv"
export CW_CLIENT_STATS=1
export CW_CLIENT_STATS_NOTE_MS="${NOTE_MS:-4}"
export CW_CLIENT_STATS_WAIT_MS="${WAIT_MS:-1}"

client=(./target/release/cw-client ${client_args[@]+"${client_args[@]}"})
if [ $samply -eq 1 ]; then
    client=(samply record --save-only -o "$out/cpu.json.gz" "${client[@]}")
fi

echo "== recording to $out (play, then quit through the game)"
status=0
"${client[@]}" 2> >(tee "$out/stats.log" >&2) || status=$?
# Let the tee on stderr finish writing.
wait

echo
echo "== summary"
if command -v python3 >/dev/null 2>&1 && [ -s "$out/trace.csv" ]; then
    python3 - "$out/trace.csv" <<'EOF'
import csv, sys

rows = list(csv.DictReader(open(sys.argv[1])))
# The first frames load the world and build the GUI; leave them out of the statistics.
warm = rows[60:] if len(rows) > 120 else rows
if not warm:
    print("no frames recorded")
    sys.exit()
ft = sorted(float(r["frame_ms"]) for r in warm)
pct = lambda p: ft[int((len(ft) - 1) * p)]
secs = float(warm[-1]["t_s"]) - float(warm[0]["t_s"])
print(f"{len(ft)} frames over {secs:.0f} s (first {len(rows) - len(warm)} skipped as warm-up)")
print(f"frame ms: mean {sum(ft) / len(ft):.2f}, p50 {pct(0.5):.2f}, p90 {pct(0.9):.2f}, p99 {pct(0.99):.2f}, max {ft[-1]:.1f}")
for t in (20, 33, 50, 100):
    print(f"  over {t:>3} ms: {sum(x > t for x in ft)}")
phases = [k for k in rows[0] if k not in ("t_s", "frame_ms", "dt_ms")]
print("slowest frames (top phases, ms):")
for r in sorted(warm, key=lambda r: -float(r["frame_ms"]))[:10]:
    top = sorted(phases, key=lambda k: -float(r[k]))[:4]
    parts = ", ".join(f"{k} {float(r[k]):.1f}" for k in top if float(r[k]) >= 0.5)
    print(f"  t={float(r['t_s']):8.2f} s  {float(r['frame_ms']):6.1f} ms  dt {r['dt_ms']:>3}  {parts}")
EOF
else
    echo "(no trace, or python3 missing)"
fi

echo
echo "== files in $out:"
ls -l "$out"
[ $status -eq 0 ] || echo "cw-client exited with status $status"
