#!/bin/sh
set -eu

# Runs five independent release-process measurements. The harness emits only
# aggregate metrics; this script filters /usr/bin/time -l to its peak RSS line.

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
crate_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)

cd "$crate_dir"
cargo build --release
binary="$crate_dir/target/release/full-scan-feasibility"

run=1
while [ "$run" -le 5 ]; do
  output=$(/usr/bin/time -l "$binary" 2>&1)
  scan_ms=$(printf '%s\n' "$output" | awk -F '\t' '/^sequential_scan_wall_time_ms\t/ { print $2 }')
  audit_ms=$(printf '%s\n' "$output" | awk -F '\t' '/^hash_audit_wall_time_ms\t/ { print $2 }')
  peak_rss=$(printf '%s\n' "$output" | awk '/maximum resident set size/ { print $1 }')
  immutable=$(printf '%s\n' "$output" | awk -F '\t' '
    /^provider\t/ { next }
    /^(codex|claude|opencode|gemini|cursor|kimi)\t/ && $10 != "true" { changed = 1 }
    END { print changed ? "false" : "true" }
  ')
  printf 'run=%s scan_ms=%s hash_audit_ms=%s peak_rss_bytes=%s all_provider_sources_immutable=%s\n' \
    "$run" "$scan_ms" "$audit_ms" "$peak_rss" "$immutable"
  run=$((run + 1))
done
