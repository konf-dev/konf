#!/usr/bin/env bash
# konf-prime smoke test — runs after `docker compose up -d prime` to
# verify every substrate capability the orchestrator will need.
# Exit 0 if all green, non-zero otherwise. Prints a terse per-route line.

set -euo pipefail
PORT="${PORT:-8002}"
SESSION="${SESSION:-smoke-$(date +%s)}"
URL="http://localhost:${PORT}/v1/chat"

post() {
    # Stream the SSE response, find the last `event: done` or `event: error`
    # payload and echo it on one line for inspection.
    local message="$1"
    curl -sS --max-time 120 \
        -H 'content-type: application/json' \
        -d "$(jq -nc --arg m "$message" --arg s "$SESSION" '{message:$m,session_id:$s}')" \
        "$URL" | awk '
            /^event: done/  { t="done"; next }
            /^event: error/ { t="error"; next }
            /^data: /       { if (t) { sub(/^data: /, ""); print t "\t" $0; t="" } }
        ' | tail -1
}

pass() { printf "  \033[32mok\033[0m   %s\n" "$1"; }
fail() { printf "  \033[31mFAIL\033[0m %s — %s\n" "$1" "$2"; failed=1; }

echo "konf-prime smoke — session=$SESSION  URL=$URL"
failed=0

# 1. health
if curl -sf "http://localhost:${PORT}/v1/health" > /dev/null; then
    pass "health"
else
    fail "health" "probe not reachable on ${PORT}"
    exit 1
fi

# 2. state round-trip within run
out=$(post 'probe:smoke-state')
if [[ "$out" == done* && "$out" == *hello-from-smoke* ]]; then
    pass "smoke-state"
else
    fail "smoke-state" "$out"
fi

# 3. pure-ref passthrough — LLM should mention green (per prior history)
out=$(post 'probe:smoke-passthrough')
# Accept any case variant of "green"
if [[ "$out" == done* && $(echo "$out" | tr A-Z a-z) == *green* ]]; then
    pass "smoke-passthrough (#24 fix)"
else
    fail "smoke-passthrough" "$out"
fi

# 4. multi-turn messages — history has "my name is Bert", asks the name
out=$(post 'probe:smoke-self-intro')
if [[ "$out" == done* && "$out" == *Bert* ]]; then
    pass "smoke-self-intro (ai:complete messages)"
else
    fail "smoke-self-intro" "$out"
fi

# 5. self-authoring — write workflow + config:reload, new tool registered
out=$(post 'probe:smoke-author')
if [[ "$out" == done* && "$out" == *reloaded* ]]; then
    pass "smoke-author (self-write + reload)"
else
    fail "smoke-author" "$out"
fi

echo
if (( failed )); then
    echo "FAIL — konf-prime substrate NOT stable"
    exit 1
fi
echo "PASS — konf-prime substrate stable"
