# Lean First Request — design

> Date: 2026-08-18 · Status: approved (design summary; doc written after user approval) · Feature: `lean-first-request`

## 1. Motivation

Port the "first-turn anchoring" idea from `/workspace/deepseek-harness/pi-deepseek-route` into dirge as a native feature. The router's README describes it as 首轮锚定 ("first-turn anchoring"): the first request of a model session exposes only a minimal system prompt and a reduced core tool set; after the first tool call (or first turn completes) the full original system prompt and full tool surface are restored. The router's `core.ts` implements the same shape as a persona + tool-surface cut applied per request.

Target: **DeepSeek V4 Flash** (chat family). Today dirge ships every DeepSeek chat session with a large system preamble — base contract, AGENTS.md, memory, skills, persona, mode reminder, DeepSeek steering fragment, capability projection — on the FIRST request, which is often a cheap exploration turn. Most of that preamble is overhead on request 1: it is cold-cache (the provider has never seen it), it delays first-token latency, and it costs input tokens. Cutting it to a minimal contract + two tools, then restoring the full original prompt from request 2 on, reduces first-request cost and latency with no permanent capability loss.

## 2. Approved decisions

(from the design conversation; all confirmed by the user)

- **Activation** — automatic when the ACTIVE model family is DeepSeek chat (v3/v4, incl. `deepseek-v4-flash` and OpenRouter passthrough) via the existing `resolve_family` / `is_deepseek_chat` logic; plus a config key `lean_first_request` to force on/off.
- **Minimal prompt** — base opener (system identity + operating contract) + a core-tool line. The DeepSeek steering fragment and every other block are deferred to request 2 (user explicitly chose "base opener + core-tool line only").
- **Core tools** — exactly `read` and `bash` (user explicitly chose "only read and bash, two tools").
- **Subagents** — enabled for tooled subagents when the subagent's OWN model family is DeepSeek chat, under two guards; toolless one-shot subagents are excluded.
- **Upgrade mechanism** — truncate-then-grow (append). The full system prompt is assembled ONCE and never mutated; the lean text is a strict byte-prefix of it. Request 1 ships the prefix; every later request ships the whole string. The head bytes never change — only grow — so the provider-side prefix cache carries the lean block forward. (User's correction: a swap-and-restore would invalidate the cache at the swap point and could leak on error paths; append is the correct shape — "append the original full system prompt".)

## 3. Mechanism

### 3.1 Truncate-then-grow, not swap

Current flow: `build_agent_inner` assembles the base preamble → AGENTS.md → memory/skills/persona/mode reminder → DeepSeek steering (`src/agent/builder/agent_inner.rs:61-71, 285-301`); `provider/build.rs` appends the capability projection and optional tool-search nudge / coordinator preamble (`src/provider/build.rs:222-262`). The result is stored on `Context.system_prompt` and cloned into every per-request `LlmContext` (`src/agent/agent_loop/stream.rs:198-202`).

Lean flow:

1. The base assembly inserts a **permanent core-tool line** immediately after the system opener (only when the run is lean-enabled). This line is a literal part of the full string, so it can serve as the truncation boundary.
2. The lean system prompt = the first `lean_boundary` bytes of the full string: opener + core-tool line, and nothing else.
3. Request 1: `LlmContext.system_prompt` = the lean prefix; tool definitions = `{read, bash}` ∩ live registry, deny-filtered.
4. After request 1 completes (tool call or plain answer; success or error), the loop clears the lean arming flag. Request 2+ ships the full string unchanged and the full tool defs. `Context.system_prompt` is never mutated, so hooks, judges and the goal gate always see the real prompt.

Prefix-cache consequence: the system message is re-sent wholesale each request; requests 1 and 2 share the exact head bytes (opener + core line), so the provider's prefix cache carries them. The blocks truncated off on request 1 (memory, skills, persona, steering, projection) are a cache WRITE on request 2 — unavoidable in any design, since they weren't sent before. This design only removes the *avoidable* loss (the lean head itself).

### 3.2 Byte-prefix invariant

`lean_preamble == full_preamble[..lean_preamble.len()]` — enforced by construction, asserted in tests. Because `provider/build.rs` only ever appends at the tail, any prefix of the `build_agent_inner` string is also a prefix of the final shipping string.

### 3.3 What ships when

- **Request 1** (fresh DeepSeek chat session): base opener + core-tool line + the user's first message. Tool defs: read, bash. No AGENTS.md, memory, skills, persona, mode reminder, DeepSeek steering, capability projection, or tool-search nudge.
- **Request 2+**: the exact full preamble as assembled today (byte-identical to a non-lean run), full tool defs, normal deny filtering.
- **Non-DeepSeek / forced-off / resumed / subagent-excluded runs**: byte-identical to today, never enters the lean path.

## 4. Activation and configuration

- **Family gate**: `resolve_family(active_provider, active_model).is_deepseek_chat()` — the same predicate as the existing DeepSeek steering fragment (`agent_inner.rs:297`, `preamble.rs:84-88`). Mid-session `/model` / `/agent` swaps re-resolve on rebuild (dirge-5db6 pattern; steering today already keys off the active model).
- **Config**: `lean_first_request` in config.json — `Option<bool>` semantics: absent/null = auto (DeepSeek chat only), `true` = force for every model, `false` = force off. Resolved via a new `Config::resolve_lean_first_request() -> Option<bool>`, matching the existing `resolve_capability_projection` shape.
- **Fresh-session gate**: lean is armed ONLY when the agent is built for a session with no prior history. Resumed sessions (`--resume`), headless runs with loaded history, and mid-session rebuilds (`/model`, `/agent`, regen) are exempt — their first request already carries conversation context and must present the full system prompt. The provider knows resume/history state at build time and passes `fresh_session: bool` into lean assembly.
- **`--no-tools`**: the registry is empty, so the tool-narrowing half is a no-op; the lean prompt still applies (it saves tokens with no functionality loss).

## 5. Tool narrowing (first-request core set)

- The lean core set is a single constant: `LEAN_CORE_TOOLS = ["read", "bash"]`. Documented implication: on request 1 the model navigates the repo via `read` + in-shell bash; grep/glob/etc. unlock on request 2 via the normal registry.
- **Filtering**: a pure `retain_core_tools(tools, core, denied)` alongside the existing `retain_tool_defs` (`rig_stream_factory.rs:452-509`) — intersection with `core`, on top of the existing deny predicate. This is DISTINCT from the dynamic-tool-search filter (union-with-always-on semantics, `LoopConfig.tool_def_filter` at `types.rs:533`): the lean filter applies first, only on request 1, then the dynamic filter (when enabled) resumes from request 2.
- **Threading**: the stream closure consults a shared lean slot per request — same `Arc<Mutex<…>>` pattern as the dynamic filter. The loop arms it (`Some(core)`) before the first `stream_assistant_response` and clears it (`None`) immediately after it returns (call site at `run.rs:2985-2993`). Requests are serialized in the loop, so the write (after request 1 returns) and reads (inside the request) cannot race.
- **Permission layer untouched**: `--yolo`, prompt denies, and the permission checker keep their exact current behavior. Lean narrowing is purely a model-visible tool-definition reduction, not an enforcement relaxation.

## 6. Subagents

- **Tooled subagents** (`readonly`/`readwrite` → `run_tooled`, `task.rs:966`) run a real loop and can issue ≥2 requests, so lean applies the same way: request 1 gets the lean prefix + core tools; request 2 restores their full routed preamble (profile persona) and full allowed tool set.
- **Entry gate**: the subagent's OWN model family is DeepSeek chat — `route_model` (profile-pinned model or the live agent's) resolved via `resolve_family`. Independent of the main loop's family: a DeepSeek main loop with an Anthropic subagent does not lean the subagent, and vice versa.
- **Guards** (skip lean when either fails):
  - `max_turns >= 2` — with a 1-turn cap there is no request 2 to restore the full prompt, so lean would permanently strip the profile persona.
  - `{read, bash} ∩ allowed` non-empty — an allow-list excluding both would leave a first request with zero tools; skipping lean is safer.
- **Toolless one-shot subagents** (the default `btw_query` path): excluded — exactly one request by construction; lean would deny the full system prompt forever with no session to amortize the savings over.
- Subagent lean state is per-subagent-loop: `run_tooled` builds its own `LeanFirst` from the subagent's family / preamble / allowed set; nothing shares state with the parent loop.

## 7. Implementation touch points

- `src/agent/prompt.rs` — split `SYSTEM_PROMPT` into `SYSTEM_PROMPT_OPEN` (through the end of the first paragraph) + `SYSTEM_PROMPT_REST` (the remainder), with a test asserting `OPEN + REST == SYSTEM_PROMPT` byte-for-byte; add `LEAN_CORE_LINE` — the permanent core-tool sentence, phrased time-independently ("Always available: read, bash.") so it never reads as a constraint once the full list appears below it on request 2+.
- `src/agent/agent_loop/lean.rs` (new) — `LEAN_CORE_TOOLS: &[&str]`, `pub struct LeanFirst { system_prompt: String, core_tools: Arc<Mutex<Option<Vec<String>>>> }`, arm/clear helpers.
- `src/agent/builder/preamble.rs` — `assemble_base_preamble_with_lean(capability_projection, lean_enabled) -> (String, Option<usize>)` (full string + lean boundary); `assemble_base_preamble` keeps its exact current output for the non-lean path.
- `src/agent/builder/agent_inner.rs` — use the lean-aware base assembly; return the lean prefix alongside the full preamble (extend the existing return tuple).
- `src/provider/build.rs` — when lean is enabled (family × config × fresh-session), create the `LeanFirst` slot and thread the lean prefix + shared core slot into the LoopConfig and both stream fns (main + escalation).
- `src/agent/agent_loop/types.rs` — `LoopConfig.lean_first: Option<LeanFirst>`.
- `src/agent/agent_loop/stream.rs` — `stream_assistant_response`: when the lean slot is armed, use `lean_first.system_prompt` for the `LlmContext` instead of `context.system_prompt.clone()` (`stream.rs:198-202`).
- `src/agent/agent_loop/rig_stream_factory.rs` — consume the lean slot in the stream closure; add pure `retain_core_tools`.
- `src/agent/agent_loop/run.rs` — clear the lean slot right after the first `stream_assistant_response` returns (`run.rs:2993`). `Context.system_prompt` is never touched.
- `src/agent/tools/task.rs` / `src/provider/spawn.rs` — `run_tooled` accepts an optional `LeanFirst`, built under the subagent gate (family × max_turns ≥ 2 × core ∩ allowed).

## 8. Edge cases

- **Retry of request 1**: lean is cleared after the first stream call completes (any outcome); an immediate retry therefore ships the full prompt. Acceptable and safe.
- **Escalation stream fn** (degraded path): receives the same lean slot so its single call is consistent; if wiring is inconvenient, the escalation path simply skips lean and ships the full prompt — a degraded path should not be lean.
- **Goal gate / verifier / summarizer**: separate model calls with their own prompts — untouched, and they read `context.system_prompt`, which is never mutated.
- **Turn hooks** (`prepare_next_turn`, `should_stop_after_turn`, `run.rs:4034-4042`): receive the full context; the lean override exists only at the stream boundary.
- **Mid-session rebuild**: `fresh_session=false` → no re-arming.
- **Persona / tool-policy interaction**: deny_tools is still enforced; a persona that denies `read` or `bash` is respected by `retain_core_tools`' deny predicate (intersection after denial).

## 9. Acceptance criteria

- Fresh DeepSeek chat session: request 1 wire payload = lean prefix (`full[..lean_boundary]`) + tool defs exactly `{read, bash}` ∩ registry; request 2+ = full preamble + full defs; assert `lean == full[..lean.len()]`.
- Non-DeepSeek, forced-off, resumed, or headless-with-history runs: byte-identical to today.
- Tooled DeepSeek subagent with `max_turns >= 2`: lean on request 1, full from request 2. `max_turns == 1` or no core ∩ allowed: never lean. Toolless: never lean.
- `--no-tools`: lean prompt applies; tool-narrowing is a no-op.
- Deny filter honored inside the lean core set.
- `Context.system_prompt` never mutated; hooks see the full prompt on every request.

## 10. Tests

- `retain_core_tools` unit tests: intersection, deny-awareness, order stability.
- Byte-prefix invariant tests: `lean == full[..lean.len()]`; `SYSTEM_PROMPT_OPEN + SYSTEM_PROMPT_REST == SYSTEM_PROMPT`.
- Loop-level test mirroring `h7_smoke` (`src/agent/agent_loop/h7_smoke.rs`): capture the per-request `LlmContext` from a mock stream fn; assert request 1's system prompt == lean prefix and request 2's == full; assert the tool list narrows on request 1 only.
- Gate tests: family auto / forced on / forced off; fresh vs resume; subagent guards.

## 11. Risks and mitigations

- **DeepSeek V4 Flash behavior on request 1 without full context**: mitigations — the core tools enable real exploration; the full prompt returns within one turn on request 2; `lean_first_request: false` is a one-line escape hatch.
- **Request-2 cache write for the deferred blocks**: unavoidable (see 3.1); the lean head carries. For a session that would have run only one or two requests anyway the net cost may be slightly negative — the escape hatch covers it, and the change is cheap to A/B against the baseline.
- **The permanent core-tool line on request 2+**: phrased "always available", it is honest (read and bash ARE always present) and the capability projection below it adds the full surface.

## 12. Out of scope

- Changing the DeepSeek steering fragment or its gating.
- Task classification / personas (react/spec/weak) from pi — this ports only the first-request anchoring axis.
- Any change to permission enforcement or `--no-tools`.

## 13. Implementation notes (recorded after implementation)

The code deliberately deviates from the §7 wiring sketch in a few places. Each preserves the §9 acceptance criteria; recorded here so reviewers can assess the code against the approved decisions without chasing the wiring.

- **Lean slot shape** — §7 sketched `LeanFirst { system_prompt, core_tools: Arc<Mutex<Option<Vec<String>>>> }` read per-request by the stream closure, with a per-request tool-set rebuild. Implemented instead as `LeanFirst { system_prompt: Option<String>, stream_fn: StreamFn, armed: AtomicBool }`: the lean stream fn is **built once at spawn** from the core-only tool defs (`retain_core_tools` in `lean.rs`), and `stream_assistant_response` selects it while armed. Same observable contract — request 1 ships the lean prefix + core tools, request 2+ ships the full preamble + full tool defs — with lower per-request cost and **no change to the `dispatch_stream_fn!` macro or the `rig_stream_fn_from_model_with_filter` factory signatures**.
- **Where the lean slot is made** — §7 put it in `provider/build.rs`. It is actually created in `provider/spawn.rs` (`spawn_runner` / `spawn_subagent_runner_with_tools`), the only sites holding the rig `tool_defs` and the fresh-session signal together. `build.rs` only computes eligibility (family × config, via `Config::resolve_lean_first_request`) and threads the lean preamble through `AnyAgent`.
- **No `SYSTEM_PROMPT_REST` constant** — §7 proposed splitting `SYSTEM_PROMPT` into `OPEN + REST` with a reassembly test. Implemented as a `SYSTEM_PROMPT_OPEN` stem plus the `system_prompt_opener_is_a_byte_prefix` invariant test (prompt.rs): the full preamble starts with the opener, so the lean prefix is a strict byte-prefix by construction. Same invariant, one less constant.
- **`retain_core_tools` lives in `lean.rs`** — §7 placed it in `rig_stream_factory.rs`. Deny and allow filtering happens upstream (the handed-in registry is already deny-filtered; the subagent gate intersects core with `allowed`), so the helper only narrows within the already-legal set; its tests cover intersection, order stability, and empty registry/empty core.
- **Escalation on request 1** — §8 offered "skip lean on escalation" as an option "if wiring is inconvenient". Implemented inverse: the escalation stream fn takes precedence over the lean one, but the lean system prompt still applies if the escalation happens on request 1 (the slot is a request-1 property). The fallback model sees a valid request-1 payload; the full prompt returns on request 2. Not a deviation from any §9 acceptance criterion.
- **Subagent prompts** — §6's "request 1 gets the lean prefix" applies to tooled subagents as tool narrowing only: a subagent's system prompt is already the small persona text, so `system_prompt: None` keeps the normal loop prompt for every request and only the tool surface narrows (guards: own family × `max_turns >= 2` × non-empty `{read,bash} ∩ allowed`, computed by `resolving_lean_core` in `lean.rs`, applied in `task.rs::run_tooled`).