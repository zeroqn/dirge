//! Lean first request: ship a minimal system prompt + core tool set on the
//! first LLM request of a session, then restore the full original prompt and
//! full tool surface from request 2 on.
//!
//! Ported idea from `pi-deepseek-route` ("first-turn anchoring"): the first
//! request is often a cheap exploration turn, and most of the system preamble
//! is cold-cache overhead there. Gating (DeepSeek chat family, config, fresh
//! session, subagent guards) lives at the call sites; this module holds the
//! per-run state and the core tool set.
//!
//! The mechanism is truncate-then-grow, not swap: the full system prompt is
//! assembled once and never mutated; the lean text is a strict byte-prefix of
//! it (see `prompt::SYSTEM_PROMPT_OPEN` / `prompt::LEAN_CORE_LINE`). Request 1
//! ships the prefix; later requests ship the whole string — the head bytes
//! never change, only grow, so the provider's prefix cache carries the lean
//! block across the upgrade instead of being invalidated at a swap point.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Tools the lean first request keeps visible. Everything else in the
/// registry unlocks on request 2. Chosen so the model can explore the repo
/// (read) and run commands (bash) without the full menu; the capability
/// projection on request 2+ shows the complete list below the core line.
pub const LEAN_CORE_TOOLS: &[&str] = &["read", "bash"];

/// Per-run lean-first state, shared between the loop (which clears it after
/// the first request) and `stream_assistant_response` (which, while armed,
/// selects the lean system prompt and the core-only stream fn).
#[derive(Clone)]
pub struct LeanFirst {
    /// System prompt for the lean request. For the main agent this is a
    /// strict byte-prefix of the full preamble (`SYSTEM_PROMPT_OPEN` +
    /// `LEAN_CORE_LINE`). For a subagent it is `None` — a subagent's system
    /// prompt is already the small persona text, so only the tool surface
    /// narrows and the normal loop prompt stays.
    pub system_prompt: Option<String>,
    /// Stream fn for the lean request — built from the core tool definitions
    /// only. Requests 2+ use the loop's regular stream fn (full tool set).
    pub stream_fn: super::stream::StreamFn,
    /// Armed until the first LLM request completes; always cleared from then
    /// on (the upgrade to the full prompt/tool set is permanent for the
    /// session). Shared across clones via Arc.
    armed: Arc<AtomicBool>,
}

impl LeanFirst {
    pub fn new(system_prompt: Option<String>, stream_fn: super::stream::StreamFn) -> Self {
        Self {
            system_prompt,
            stream_fn,
            armed: Arc::new(AtomicBool::new(true)),
        }
    }

    /// `true` while the NEXT request should be the lean one.
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Disarm: from the next request on, ship the full system prompt and the
    /// full tool set. Called once, right after the first request completes.
    pub fn clear(&self) {
        self.armed.store(false, Ordering::Relaxed);
    }
}

/// The subset of [`LEAN_CORE_TOOLS`] present in `allowed`, if any. Used by the
/// subagent gate: a tooled subagent's lean core can never exceed its resolved
/// allow-list, and an empty result means "skip lean entirely" (a first request
/// with zero tools is worse than none).
pub(crate) fn core_tools_in(allowed: &[String]) -> Vec<String> {
    let allowed: HashSet<&str> = allowed.iter().map(String::as_str).collect();
    LEAN_CORE_TOOLS
        .iter()
        .filter(|t| allowed.contains(**t))
        .map(|s| s.to_string())
        .collect()
}

/// Filter tool definitions down to `core`. Input order is preserved — the
/// result is a stable subset of the input, in registry order.
///
/// Application point (dirge-lean): the lean request's stream fn is built from
/// this subset only, while the regular stream fn keeps the full defs. Deny and
/// allow filtering happens BEFORE this helper — the registry handed in is
/// already deny-filtered, and the subagent gate intersects `core` with the
/// allowed set in [`core_tools_in`] — so this function only ever narrows
/// within the already-legal set. An empty registry (e.g. `--no-tools`) or an
/// empty `core` yields an empty result: the lean prompt still applies, the
/// tool-narrowing half is a no-op.
pub(crate) fn retain_core_tools(
    defs: &[rig::completion::ToolDefinition],
    core: &[&str],
) -> Vec<rig::completion::ToolDefinition> {
    let core: HashSet<&str> = core.iter().copied().collect();
    defs.iter()
        .filter(|d| core.contains(d.name.as_str()))
        .cloned()
        .collect()
}

/// Subagent lean gate (dirge-lean). Returns the lean core tool set (⊆
/// `allowed`) for a tooled subagent, or `None` to run the pre-lean path.
///
/// Guards:
/// - `max_turns >= 2` — a single-turn subagent has no request 2 to upgrade
///   to, so lean would permanently strip its prompt/tools.
/// - model family: the profile-pinned `family` when `Some`, else the live
///   agent's lean eligibility (`live_agent_eligible` — same family gate, from
///   `AnyAgent::lean_eligible`).
/// - `core ∩ allowed` non-empty — otherwise a lean request would ship with
///   zero tools.
pub(crate) fn resolving_lean_core(
    family: Option<crate::agent::model_family::ModelFamily>,
    live_agent_eligible: bool,
    max_turns: usize,
    allowed: &[String],
) -> Option<Vec<String>> {
    if max_turns < 2 {
        return None;
    }
    let eligible = match family {
        Some(f) => f.is_deepseek_chat(),
        None => live_agent_eligible,
    };
    if !eligible {
        return None;
    }
    let core = core_tools_in(allowed);
    if core.is_empty() {
        None
    } else {
        Some(core)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_tools_are_read_and_bash_only() {
        assert_eq!(LEAN_CORE_TOOLS, &["read", "bash"]);
    }

    #[test]
    fn core_tools_in_filters_by_allowed() {
        let allowed = vec!["read".to_string(), "grep".to_string()];
        assert_eq!(core_tools_in(&allowed), vec!["read"]);

        let none = vec!["write".to_string(), "edit".to_string()];
        assert!(core_tools_in(&none).is_empty());
    }

    fn tool_def(name: &str) -> rig::completion::ToolDefinition {
        rig::completion::ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn retain_core_tools_keeps_only_core_and_preserves_order() {
        let defs = vec![
            tool_def("list_dir"),
            tool_def("read"),
            tool_def("grep"),
            tool_def("bash"),
            tool_def("write"),
        ];
        let kept = retain_core_tools(&defs, LEAN_CORE_TOOLS);
        let names: Vec<&str> = kept.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "bash"]);
    }

    #[test]
    fn retain_core_tools_handles_empty_registry_and_empty_core() {
        // `--no-tools` / empty registry: the tool-narrowing half is a no-op.
        assert!(retain_core_tools(&[], LEAN_CORE_TOOLS).is_empty());
        // Empty core (never produced by the gates, but defensive): empty out.
        let defs = vec![tool_def("read"), tool_def("bash")];
        assert!(retain_core_tools(&defs, &[]).is_empty());
    }

    #[test]
    fn resolving_lean_core_guards() {
        use crate::agent::model_family::resolve_family;
        let deepseek = resolve_family("deepseek", "deepseek-v4-pro");
        let openai = resolve_family("openai", "gpt-4o");
        let allowed = vec!["read".to_string(), "bash".to_string()];

        // Single-turn subagent: no request 2 to upgrade to → skip lean.
        assert!(resolving_lean_core(Some(deepseek), false, 1, &allowed).is_none());

        // Empty `{read, bash} ∩ allowed` (readonly tier) → skip lean.
        let readonly = vec!["grep".to_string(), "list_dir".to_string()];
        assert!(resolving_lean_core(Some(deepseek), false, 2, &readonly).is_none());

        // Non-DeepSeek profile-pinned family → skip lean even if live agent is eligible.
        assert!(resolving_lean_core(Some(openai), true, 2, &allowed).is_none());

        // DeepSeek family + max_turns >= 2 + non-empty core → armed with the
        // intersection of the core set and `allowed`.
        let core = resolving_lean_core(Some(deepseek), false, 2, &allowed);
        assert_eq!(core.as_deref(), Some(&["read".to_string(), "bash".to_string()][..]));

        // No pinned family → falls back to the live agent's eligibility.
        assert!(resolving_lean_core(None, true, 2, &allowed).is_some());
        assert!(resolving_lean_core(None, false, 2, &allowed).is_none());
    }

    #[test]
    fn arm_clear_round_trip() {
        use super::super::stream::{LlmContext, StreamFn, StreamOptions};
        use futures::Stream;
        fn noop_stream(
            _ctx: LlmContext,
            _opts: StreamOptions,
        ) -> std::pin::Pin<Box<dyn Stream<Item = super::super::message::StreamEvent> + Send>>
        {
            Box::pin(futures::stream::empty())
        }
        let noop: StreamFn = Arc::new(noop_stream);
        let lean = LeanFirst::new(Some("prefix".to_string()), noop);
        assert!(lean.is_armed());
        lean.clear();
        assert!(!lean.is_armed());
    }
}