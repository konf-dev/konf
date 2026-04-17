#!/usr/bin/env bash
# fleet/watch.sh — tail every signal from the konf-prime fleet in one place.
#
# Prints lines prefixed by source so you can see who said what:
#   [prime]        ... docker log line from konf-prime
#   [prime:mon]    ... SSE monitor event from konf-prime (/v1/monitor/stream)
#   [gemini]       ... docker log line from konf-prime-gemini
#   [gemini:mon]   ... SSE monitor event from konf-prime-gemini
#   [hybrid]       ... docker log line from konf-prime-hybrid
#   [hybrid:mon]   ... SSE monitor event from konf-prime-hybrid
#
# Runs all six streams in parallel and interleaves into one terminal.
# Ctrl-C ends all of them cleanly.
#
# Usage:
#   bash fleet/watch.sh
#   bash fleet/watch.sh prime                 # only one variant
#   bash fleet/watch.sh prime hybrid          # subset
#   bash fleet/watch.sh --no-monitor          # docker logs only, skip SSE
#   bash fleet/watch.sh --no-docker           # SSE only, skip docker logs
#
# The monitor stream carries tool_start / tool_end / text_delta / done
# events for every workflow run. Docker logs carry the konf-backend
# process-level tracing (tool registrations, errors, durable scheduler,
# MCP child process failures, etc.).

set -euo pipefail

want_docker=1
want_monitor=1
variants=()

for arg in "$@"; do
  case "$arg" in
    --no-monitor) want_monitor=0 ;;
    --no-docker)  want_docker=0 ;;
    prime|gemini|hybrid) variants+=("$arg") ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done
if [[ ${#variants[@]} -eq 0 ]]; then
  variants=(prime gemini hybrid)
fi

declare -A CONTAINER=(
  [prime]=konf-prime
  [gemini]=konf-prime-gemini
  [hybrid]=konf-prime-hybrid
)
declare -A PORT=(
  [prime]=8002
  [gemini]=8003
  [hybrid]=8004
)

pids=()
cleanup() {
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done
}
trap cleanup EXIT INT TERM

for v in "${variants[@]}"; do
  if [[ $want_docker -eq 1 ]]; then
    (
      docker logs -f --tail 0 "${CONTAINER[$v]}" 2>&1 | \
        awk -v tag="[$v]" '{print tag" "$0; fflush()}'
    ) &
    pids+=($!)
  fi
  if [[ $want_monitor -eq 1 ]]; then
    (
      while true; do
        curl -sN --max-time 86400 "http://localhost:${PORT[$v]}/v1/monitor/stream" 2>/dev/null | \
          awk -v tag="[$v:mon]" 'NF{print tag" "$0; fflush()}' || true
        sleep 2
      done
    ) &
    pids+=($!)
  fi
done

echo "watching: ${variants[*]}  docker=$want_docker  monitor=$want_monitor" >&2
wait
