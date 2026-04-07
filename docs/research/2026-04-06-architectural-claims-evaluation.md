# Architectural Claims Evaluation

**Date:** 2026-04-06
**Context:** Extended audit raised 6 architectural vulnerabilities. Each was independently verified.

---

## Verdicts

### 1. PyO3 cancellation doesn't stop Python tools — TRUE

**The issue:** When Rust cancels a tokio task via `CancellationToken` or `abort()`, a sync Python function running inside `Python::with_gil()` continues to completion. There are no `.await` points inside `with_gil`, so tokio cannot interrupt it.

**Impact:** A cancelled workflow's current Python tool (e.g., a 30-second LLM call) runs to completion, consuming API tokens. The CancellationToken prevents the NEXT node from starting, but the CURRENT one finishes.

**Decision:** Accept as known behavior. This is inherent to all FFI boundaries with synchronous foreign calls. Document it. The tool's result is discarded (the workflow is already cancelled), so the only cost is the wasted API call.

**Fix:** Document in llms.txt and README. No code change needed — changing to async Python tools would add far more complexity than this edge case warrants.

### 2. HITL needs Suspend/Resume — OVERBLOWN

**The claim:** If a workflow needs human approval mid-ReAct loop, it must suspend and resume, or the context is "vaporized."

**Why it's wrong:** The context is NOT vaporized. It's in smrti (memory graph + session state + conversation history). The pattern is:
1. Approval tool returns `{"status": "pending", "approval_id": "..."}`
2. Workflow completes normally (approval is just another tool result)
3. Webhook fires when human approves → triggers a NEW workflow
4. New workflow rebuilds context from smrti (which has everything)
5. Continues with the approval result

This is how AWS Step Functions callbacks work, how Inngest's `waitForEvent` works, and how most production systems handle HITL without durable execution.

**Decision:** No framework change. Document the HITL pattern. If Suspend/Resume is needed later (genuinely long-running workflows that can't rebuild from smrti), it can be added without breaking changes.

### 3. Session state LWW data loss — OVERBLOWN

**The claim:** Parallel branches writing to the same session state key causes data loss.

**Why it's a non-issue:** The workflow engine already keys each node's output by its `node_id` (executor.rs `State::outputs` map). Parallel branches naturally write different keys. Session state (smrti `state_set`) is for application-level scratch data — if a developer writes two branches to the same key, that's a workflow design bug, not a framework bug.

**Decision:** Document the anti-pattern. No code change. If CAS or JSON merge is needed for advanced use cases, add it as a v2 feature.

### 4. SSE backpressure stalls workflow — PARTIALLY TRUE

**The issue:** `tokio::sync::mpsc::Sender::send().await` blocks (suspends the async task) when the buffer is full. If a slow SSE client doesn't consume events, the 256-event buffer fills, and `send().await` in the executor stalls the workflow task.

**Impact:** One slow client stalls their own workflow. NOT a global deadlock — each workflow has its own channel. The workflow resumes when the client catches up.

**Fix:** Change Progress event sends from `send().await.ok()` to `try_send().ok()` (non-blocking, drops if full). Keep `send().await` for Done and Error events (must be delivered). This means a slow client may miss some TextDelta/ToolStart/ToolEnd events but will always get the final result.

### 5. Token exhaustion from ReAct loops — OVERBLOWN

**The claim:** 10 iterations × 5 retries = 50 LLM calls per trigger.

**Why the math is wrong:** Retries wrap the entire tool invocation, not individual ReAct iterations. One tool invocation = one ReAct loop of up to 10 iterations. A retry re-runs the ENTIRE loop from scratch. Retries only trigger on transient errors (timeouts, network failures), not on successful-but-wrong LLM outputs.

**Realistic worst case:** 10 LLM calls per `ai:complete` node. With the default retry policy (3 attempts), a persistent timeout would cause 3 × 10 = 30 calls — but this requires the LLM API to timeout 3 times consecutively, which means something is fundamentally wrong and the workflow fails.

**Decision:** max_iterations + max_steps + timeout is sufficient for v1. A token budget (tracking cumulative tokens across a workflow run) is a good v2 feature for cost management, not a safety requirement.

### 6. Extraction race with debounce — PARTIALLY TRUE

**The issue:** If user sends message 4 while extraction for messages 1-3 is running, the next extraction could re-process messages 1-3.

**Mitigations already in place:**
- Idempotency key (SHA256 of user_id:session_id) prevents full re-extraction of the same session
- Semantic dedup (cosine distance < 0.1) catches near-duplicate nodes
- Debounce prevents triggering extraction while user is still typing

**Remaining gap:** If extraction overlaps with new messages, the overlap window could produce near-duplicate nodes that semantic dedup misses (because LLM re-extracts slightly differently).

**Decision:** Adequate for v1. Add high-water mark (last_extracted_message_id in session state) as a v2 improvement for exact-once extraction.
