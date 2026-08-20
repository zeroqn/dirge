# Delay the First User Message to the Second Request — design

> Date: 2026-08-21 · Status: proposed (awaiting approval) · Feature: `delay-first-user-message`
> Bases on commit `0565e674` ("feat(agent): DSH-minimal first request for fresh DeepSeek sessions")

## 1. Goal

Extend the current first-request mechanism (DSH-minimal, `src/agent/agent_loop/dsh_minimal.rs`) so that on **request 1** of a fresh DeepSeek-chat session, the wire payload carries a `"hi"` placeholder in place of the user's real first message. The real first user message stays in `context.messages` (the transcript) — it just does not reach the model until request 2, which is the same request that already carries the DSH-minimal → full preamble and the full tool set.

- **Before:** request 1 wire = DSH one-liner + `bash` + `str_replace_editor` + **real first user message**.
- **After:** request 1 wire = DSH one-liner + `bash` + `str_replace_editor` + **`"hi"`**.
- Request 2 and beyond = `dsh_minimal_full_prompt` + full tools + **real first user message**.

## 2. Current state on 0565e674 (what this builds on)

The older lean-first design (`lean.rs`, `LEAN_CORE_TOOLS = ["read","bash"]`) has been superseded on this commit by the DSH-minimal contract:

- **Request-1 system prompt** = exactly `"You are a helpful software engineer assistant."` (`DSH_MINIMAL_SYSTEM_PROMPT`).
- **Request-1 tool surface** = exactly `bash` + `str_replace_editor` (`dsh_minimal_tool_defs()`).
- **Request-2+ system prompt** = `DSH_MINIMAL_SYSTEM_PROMPT "\n\n" full_preamble` (`dsh_minimal_full_prompt`), so request 1's bytes are a strict prefix of every later request — truncate-then-grow, never a swap.
- **Slot lifecycle** is unchanged and shared: `LoopConfig.lean_first: Option<LeanFirst>` armed for request 1, cleared right after the first `stream_assistant_response` returns (`run.rs:3001-3003`). `stream_assistant_response` selects the lean system prompt (`stream.rs:198-206`) and lean stream fn (`stream.rs:253-262`) only while armed.

## 3. Approved decisions (from the design conversation)

- **Placeholder exchange stays in the transcript.** After request 1, `context.messages` = `[real user msg, assistant("Hi!…")]`. On request 2 the model reads the real message followed by its own greeting, then continues. No transcript rewriting — preserves the truncate-then-grow / prefix-cache invariant and mirrors how the DSH-minimal request 1 already persists.
- **Tie to the existing armed slot** — no new config. Whenever request 1 ships the DSH-minimal persona + core tools (DeepSeek family × fresh session × gate), it also substitutes `"hi"`. Subagents that share the slot get it too.
- **Placeholder is literally `"hi"`** (user's explicit choice).
- **Usage is interactive** (TUI/harness): request 1 = `"hi"` → greeting with no tool call → the loop awaits the user's next input, which is the real first user message → request 2 ships it with the DSH-minimal-grown full preamble and the full tool set. Single-shot `-p` is out of scope (a `"hi"` greeting with no tool call would end the run without the task).

## 4. Why this works with the current mechanism

The DSH-minimal request-1 contract is armed by the same `LeanFirst.armed` `AtomicBool` that `stream_assistant_response` already reads for both the system prompt and the stream fn. When the loop waits for the user's next input after request 1, that next input — the real first user message — is what triggers request 2, and request 2 is the first request with the full preamble and full tool surface.

Resulting flow:

- **Request 1 (turn 1):** `"You are a helpful software engineer assistant."` + `bash` + `str_replace_editor` + **`"hi"`** → model replies "Hi! How can I help?" → no tool call → loop awaits user input.
- **Request 2 (turn 2):** `dsh_minimal_full_prompt` + full tool set + **real first user message** → real work begins.

This delivers the user's intent: the real first user message reaches the model on the same request that first carries the full system prompt and full tools.

## 5. Mechanism — placeholder substitution at the stream boundary

A request-1-only rewrite of the **wire** messages, exactly parallel to the existing request-1-only rewrite of the system prompt. It never touches `context.messages`.

**Interception point:** `stream_assistant_response` (`src/agent/agent_loop/stream.rs`), right after `convert_to_llm` produces `llm_messages` (line 177), when `config.lean_first` is armed. Replace the content of the **first `role == "user"` message** in `llm_messages` with `"hi"`.

**Why the first user-role message:** at request 1, `context.messages` begins with the real first user message (`run.rs` pushes `prompts` — a `UserMessage` — then appends exemplar/envelope/pre-recall notes after it, all as subsequent user-role messages). So the FIRST user-role message is unambiguously the real first user message. On later requests the slot is disarmed, so nothing is rewritten.

**Content forms:** the first user message may serialize `content` either as a plain string (`loop_message_to_value` text-only path, `message.rs:464`) or as a part-array (multipart/image, `message.rs:468`). Both become `"hi"` (a plain string) on the wire. The real content (including any image) stays in the transcript and ships on request 2.

**Slot lifecycle:** reuses the same `LeanFirst.armed` `AtomicBool` — armed for request 1, cleared right after in `run.rs:3001-3003`. No new state, no new threading.

## 6. Files touched

- `src/agent/agent_loop/dsh_minimal.rs` — add `DSH_MINIMAL_HI_PLACEHOLDER: &str = "hi"` and `pub(crate) fn rewrite_first_user_message(msgs: &mut [serde_json::Value]) -> bool`, plus unit tests. (Putting it in `dsh_minimal.rs` keeps the request-1 contract in one module; `lean.rs` holds the shared slot type used by both `request_1` helpers.)
- `src/agent/agent_loop/stream.rs` — in `stream_assistant_response`, in the same `if let Some(lean) = &config.lean_first && lean.is_armed()` branch that already selects the lean system prompt (lines 198-206), apply `rewrite_first_user_message` to `llm_messages` before building `LlmContext`. The two request-1 overrides (prompt + first-user) are co-located.
- No changes to `run.rs`, `types.rs`, `spawn.rs`, `build.rs` — the armed slot already threads through; only the stream boundary reads it. No new config key.

## 7. Edge cases

- **Multipart first user message (image):** the placeholder drops the image from request 1's wire payload, but the transcript keeps it, so request 2 carries the real image with the full prompt. No data loss.
- **Subagents:** a tooled subagent that shares the armed slot sends `"hi"` on its first request too, then its real first message on request 2 — same mechanism, per-subagent slot.
- **Retry / escalation on request 1:** the rewrite rides the same armed flag as the DSH-minimal system prompt; a request-1 retry still ships `"hi"`, later requests don't.
- **`context.messages` never mutated:** hooks, judges, goal gate and pre-recall all read the real transcript; only the per-request serialized copy is rewritten.

## 8. Out of scope

- No config flag; no change to the DeepSeek-family / fresh-session / subagent gates.
- No change to the DSH-minimal truncate-then-grow mechanism or tool surface.
- No change to permission enforcement.
- Single-shot (`-p`) invocations (see §3).

## 9. Testing

- **Unit (`dsh_minimal.rs`):** `rewrite_first_user_message` on a string-content first user message → becomes `"hi"`, only the first user message changes, later messages untouched; on a multipart array-content first user message → becomes `"hi"`; no-op when no user message is present.
- **Loop-level (`stream.rs`), extending `test_minimal_first_uses_dsh_prompt_then_grows_to_full`:** capture the per-request `LlmContext.messages`; assert request 1's first user message == `"hi"`, request 2's first user message == the real message.
