#!/usr/bin/env bash
# check-generic-substrate.sh — guard the "substrate stays generic" invariant.
#
# Greps for agent/role/namespace-shaped opinion in the substrate crate's source.
# The substrate (konflux-substrate) must contain ONLY mechanism — no agent
# roles, no namespace semantics, no resource-limit opinions. Those live in
# konf-runtime.
#
# Exit 1 if any banned term appears in src/. Warnings (doc-only hits, test
# fixtures) go to stderr but do not fail.

set -euo pipefail

cd "$(dirname "$0")/.."

SUBSTRATE_DIR="crates/konflux-substrate/src"

if [[ ! -d "$SUBSTRATE_DIR" ]]; then
    echo "error: substrate dir not found: $SUBSTRATE_DIR" >&2
    exit 2
fi

# Terms that indicate agent/policy opinion leaking into substrate.
# If any of these appears in substrate source, it's a boundary violation.
BANNED_IN_SUBSTRATE=(
    'ActorRole'         # agent role enum — runtime concern
    'actor_role'
    'ResourceLimits'    # quota opinion — runtime concern
    'resource_limits'
    'InfraAdmin'        # specific role variants
    'ProductAdmin'
    'InfraAgent'
    'ProductAgent'
    'UserAgent'
    'LLM'               # model-specific references
    'llm'
    'agent'             # agent-shaped vocabulary
)

violations=0

for term in "${BANNED_IN_SUBSTRATE[@]}"; do
    # Exclude: comments, test files, doc-comments (///), doctest blocks.
    # We want hits in production code paths.
    hits=$(
        grep -rn --include='*.rs' "$term" "$SUBSTRATE_DIR" \
            | grep -v '^\s*//' \
            | grep -v '//!' \
            | grep -v '/// ' \
            || true
    )
    if [[ -n "$hits" ]]; then
        echo "substrate-boundary violation: '$term' found in substrate source" >&2
        echo "$hits" >&2
        echo "" >&2
        violations=$((violations + 1))
    fi
done

if [[ $violations -gt 0 ]]; then
    echo "FAIL: $violations banned term(s) appear in $SUBSTRATE_DIR" >&2
    echo "The substrate must not contain agent/role/namespace policy." >&2
    echo "Move offending code to konf-runtime or open an RFC if the term is justified." >&2
    exit 1
fi

echo "OK: substrate source is free of banned opinion terms"
