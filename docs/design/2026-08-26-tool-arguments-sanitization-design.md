# Design: providers-boundary tool-arguments sanitization

Date: 2026-08-26
Branch: `deepseek`
Status: Approved for implementation

## Problem

One malformed `write_todo_list` tool call in session history has `arguments` as
`Value::String` whose content is a JSON-encoded string (TOON-text double-encoded:
`"\"{\\\"todos\\\"...\""`). On replay, `value_to_assistant_content` passes this string to
rig's `ToolFunction { arguments }`, and rig's OpenAI `stringified_json::serialize` emits
`"arguments": "\"{\\\"todos\\\"...\""` on the wire. DeepSeek's validator does
`json.loads(arguments).items()` — the string has no `.items()` — and crashes with 400001.

## Root cause (verified against source)

- `src/agent/agent_loop/rig_stream_factory.rs:834` — in `value_to_assistant_content`, the
  `"toolCall"` arm passes `obj.get("arguments").cloned().unwrap_or(Value::Null)` straight into
  `ToolFunction { name, arguments }`.
- rig-core 0.41 `providers/openai/completion/mod.rs` serializes `Function.arguments` with
  `json_utils::stringified_json::serialize` = `value.to_string()` as a string literal.
- For a valid call, rig's streaming parse (`deserialize_maybe_stringified` →
  `parse_tool_arguments` = `serde_json::from_str`) turns the raw string into `Value::Object`,
  which is what lands in history (`rig_stream.rs:488-491`).
- For the poisoned call, `serde_json::from_str` on `"\"{\\\"todos\\\"...\""` *succeeds* and
  returns `Value::String` — so history holds a `Value::String`, and replay double-encodes it
  on the wire. `json.loads(arguments)` then yields a `str`, and `.items()` raises.

## Why not caught elsewhere

- `compressing_http::structural_validation_failure` checks message-level structure (roles,
  content types) but not tool-call `arguments` depth.
- The auto-dump diagnostics capture the failure but do not prevent it.

## Fix location

`src/agent/agent_loop/rig_stream_factory.rs:834`, the `"toolCall"` arm of
`value_to_assistant_content`. This is the single funnel every provider-bound assistant
message passes through (`value_to_rig_message_for_provider` → `build_rig_request`), covering
all code paths and all providers. No other path converts loop messages to wire format.

## Change

```rust
/// Sanitize a tool-call `arguments` value at the provider boundary.
/// A well-formed JSON object passes through unchanged; a string that
/// decodes to an object is normalized to that object (the well-formed
/// single-encoded case); anything else — double-encoded string, bare
/// string, invalid JSON, null, array, scalar — becomes `{}` so a
/// structurally invalid tool call can never reach the wire and crash
/// the provider's message validator (`json.loads(arguments).items()`).
fn sanitize_tool_arguments(arguments: Value) -> Value {
    match arguments {
        Value::String(s) => serde_json::from_str::<Value>(&s)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({})),
        value @ Value::Object(_) => value,
        _ => serde_json::json!({}),
    }
}
```

Applied at line 834:

```rust
let arguments = sanitize_tool_arguments(obj.get("arguments").cloned().unwrap_or(Value::Null));
```

## Behavior matrix

| Input `arguments` Value | Final Value | Wire output | Notes |
|---|---|---|---|
| `Value::Object({"limit":30})` | unchanged `{"limit":30}` | `"arguments": "{\"limit\":30}"` | common valid case, no change |
| `Value::String("{\"limit\":30}")` | `{"limit":30}` | `"arguments": "{\"limit\":30}"` | normalized, same wire bytes as before |
| `Value::String("\"{\\\"todos\\\"...\"")` | `{}` | `"arguments": "{}"` | **the bug — fixed** |
| `Value::String("bad json")` | `{}` | `"arguments": "{}"` | parse failure → empty |
| `Value::Null` | `{}` | `"arguments": "{}"` | missing → empty |

## Unit test

Add to `rig_stream_factory.rs` `#[cfg(test)] mod tests`, mirroring
`assistant_tool_call_block_converts`:

- Input: assistant message with a toolCall block whose `arguments` is the double-encoded
  string shape (e.g. `Value::String("\"hi\"")`). Assert the resulting
  `AssistantContent::ToolCall.function.arguments` is `serde_json::json!({})`.
- Input: toolCall with `arguments: Value::String("{\"valid\": true}")`. Assert the result is
  `Value::Object({"valid": true})`.
- Input: toolCall with `arguments: Value::Object({"valid": true})`. Assert unchanged (guard
  against regression).

## Verification

1. `cargo test --bin dirge` — existing tests pass, new test passes.
2. Replay the dumped `wire_body` (`/workspace/dirge-request-1787711511122-22764-0.json`) with
   `messages[232].tool_calls[0].function.arguments` replaced by `"{}"` against the CLIProxyAPI
   endpoint; the sanitized body must not return 400001.

## Scope

Included: the sanitization function + application at line 834 + unit tests.

Excluded:
- Session history is not mutated (malformed args stay in storage).
- Tool dispatch is not affected (the "Tool input rejected" result was already produced).
- No new dependencies.
- No change to `recovery::classify_error` 400001 handling (tracked as a separate secondary
  observation in `invalid_tool.md`).
