//! Preamble / system-prompt assembly helpers for the agent builder.
//! Split out of `agent/builder.rs` (dirge-4y4l stage 11a): the small,
//! independently-testable text-building helpers that `build_agent_inner`
//! layers into the system prompt.

use crate::agent::model_family::ModelFamily;
use crate::agent::prompt::{
    CODE_MODE_GUIDANCE, DEEPSEEK_GUIDANCE, LEAN_CORE_LINE, MEMORY_GUIDANCE, SESSION_SEARCH_GUIDANCE,
    SKILLS_GUIDANCE, STATIC_TOOL_LIST, SYSTEM_PROMPT, SYSTEM_PROMPT_OPEN, TODO_TOOLS_PROMPT,
};

/// Append a memory provider's prompt block to the assembled preamble.
/// Goes through `MemoryProvider::format_for_system_prompt`
/// (trait-dispatched) so a non-default backend's block lands in the
/// preamble too — pre-fix `builder.rs` called the concrete
/// `MemoryToolStore::format_for_system_prompt` directly, which broke
/// any future plugin provider's prompt contribution. See dirge-fmau.
pub(crate) fn append_memory_to_preamble(
    preamble: &mut String,
    provider: &std::sync::Arc<dyn crate::extras::memory_provider::MemoryProvider>,
) {
    tracing::debug!(
        target: "dirge::memory",
        provider = provider.name(),
        "Injecting memory provider prompt block"
    );
    let block = provider.format_for_system_prompt();
    if !block.is_empty() {
        preamble.push_str(&block);
    }
}

/// Append the GLOBAL (cross-project) memory tier's block, under a header
/// that distinguishes it from the project memory above so the model knows
/// these are durable user preferences carried across every project. No-op
/// when the global store is empty.
pub(crate) fn append_global_memory_to_preamble(
    preamble: &mut String,
    provider: &std::sync::Arc<dyn crate::extras::memory_provider::MemoryProvider>,
) {
    let block = provider.format_for_system_prompt();
    if !block.is_empty() {
        preamble.push_str("\n\n## Global memory (cross-project user preferences)\n");
        preamble.push_str(&block);
    }
}

/// Assemble the always-on base preamble — `SYSTEM_PROMPT`,
/// `TODO_TOOLS_PROMPT`, and the in-session `SKILLS_GUIDANCE`
/// (dirge-xxun, mirroring hermes `SKILLS_GUIDANCE`). Other contextual
/// blocks (AGENTS.md, prompts, project skills, memory) are layered on
/// top by `build_agent_inner`. Extracted so the assembly is testable
/// without exercising the full DI signature.
/// Non-lean entry point (used by the reminder/preamble test suite; the
/// production call path goes through the lean-aware variant below).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn assemble_base_preamble(capability_projection: bool) -> String {
    assemble_base_preamble_with_lean(capability_projection, false).0
}

/// Lean variant of [`assemble_base_preamble`]. When `lean_enabled`, inserts
/// [`crate::agent::prompt::LEAN_CORE_LINE`] immediately after the system
/// opener and returns the byte offset of that insertion point as `Some(len)`,
/// so the caller can ship only that prefix on the session's first request and
/// the full string from then on.
///
/// The lean prefix is a STRICT byte-prefix of the returned full string: the
/// head bytes never change across the upgrade, only grow, so the provider's
/// prefix cache carries the lean block instead of being invalidated at a swap
/// point. `lean_enabled = false` produces the exact bytes of
/// [`assemble_base_preamble`].
pub(crate) fn assemble_base_preamble_with_lean(
    capability_projection: bool,
    lean_enabled: bool,
) -> (String, Option<usize>) {
    assert!(
        SYSTEM_PROMPT.starts_with(SYSTEM_PROMPT_OPEN),
        "SYSTEM_PROMPT_OPEN must stay a byte-prefix of SYSTEM_PROMPT (unit test \
         system_prompt_opener_is_a_byte_prefix guards this)"
    );
    let mut p = String::new();
    p.push_str(SYSTEM_PROMPT_OPEN);
    if lean_enabled {
        p.push_str(LEAN_CORE_LINE);
    }
    p.push_str(&SYSTEM_PROMPT[SYSTEM_PROMPT_OPEN.len()..]);
    // dirge-e31n.3: the hand-written tool list, appended only when the live
    // projection is OFF. Exactly one of the two describes the tool set —
    // both would state it twice with two different answers.
    if !capability_projection {
        p.push('\n');
        p.push_str(STATIC_TOOL_LIST);
    }
    p.push('\n');
    p.push_str(TODO_TOOLS_PROMPT);
    // dirge-xxun: skills self-improvement nudge (hermes SKILLS_GUIDANCE).
    p.push_str(SKILLS_GUIDANCE);
    // dirge-a6bv: memory + past-session recall guidance (hermes
    // MEMORY_GUIDANCE + SESSION_SEARCH_GUIDANCE). Both tools are always
    // present in dirge's registry, so we inject unconditionally rather
    // than tool-gating like hermes does on `valid_tool_names`.
    p.push_str(MEMORY_GUIDANCE);
    p.push_str(SESSION_SEARCH_GUIDANCE);
    let lean_boundary = lean_enabled.then(|| SYSTEM_PROMPT_OPEN.len() + LEAN_CORE_LINE.len());
    (p, lean_boundary)
}

/// Model-specific steering fragment to append to the preamble, if any.
///
/// Returns the DeepSeek guidance for DeepSeek **chat** models and `None`
/// for everything else (other vendors, and the DeepSeek reasoner, which
/// ignores the system prompt). Appended last by `build_agent_inner` so it
/// sits closest to the conversation / action boundary — research shows
/// rules stated far from the decision point lose influence in long
/// tool-calling loops ("prompt-distance drift").
pub(crate) fn model_steering_fragment(family: ModelFamily) -> Option<&'static str> {
    if family.is_deepseek_chat() {
        Some(DEEPSEEK_GUIDANCE)
    } else {
        None
    }
}

/// Append the "code mode" rubric to `preamble` when `enabled`. Gated on
/// `config.code_mode_rubric` (default off) so the guidance ships only when
/// the operator opts in — the A/B harness flips this per run to isolate
/// the rubric's token effect. No-op when disabled, so the baseline
/// preamble is byte-for-byte unchanged.
pub(crate) fn append_code_mode_guidance(preamble: &mut String, enabled: bool) {
    if enabled {
        preamble.push_str(CODE_MODE_GUIDANCE);
    }
}

/// Append a mode-specific reminder to `preamble` based on the active prompt
/// name. `plan_exists` reports whether `PLAN.md` is present in CWD — only
/// consulted for the `code` mode reminder. Unknown prompt names produce no
/// reminder so custom prompts don't accidentally pick up plan/review semantics.
pub(crate) fn append_mode_reminder(preamble: &mut String, prompt_name: &str, plan_exists: bool) {
    match prompt_name {
        "plan" => {
            preamble.push_str("\n\n---\n\nYou are now in PLAN mode. Create a detailed implementation plan and present it in your chat reply for the user to review — do NOT save it to a file (write/edit/apply_patch are denied in plan mode). Analyze the task, break it into concrete steps, consider edge cases and trade-offs. Do NOT write any code or run any commands until the user reviews and approves the plan.");
        }
        "review" | "review-security" => {
            preamble.push_str("\n\n---\n\nYou are now in REVIEW mode. Review the code or plan carefully. Identify bugs, security issues, performance problems, and design flaws. Be thorough and specific. Provide actionable feedback.");
        }
        "code" if plan_exists => {
            preamble.push_str(
                "\n\n---\n\nA plan file exists at PLAN.md. Execute the plan step by step. Write and test code following the plan. Report progress after each step. The plan is your guide — follow it closely.",
            );
        }
        _ => {}
    }
}
