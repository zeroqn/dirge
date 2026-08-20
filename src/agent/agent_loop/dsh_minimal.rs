//! The DeepSeek Harness `minimal` preset contract, frozen as Dirge constants.
//!
//! DeepSeek Harness ships the `minimal` agent preset
//! (`apps/cli/config/agent-presets/minimal/agent.cordis.yml` in the DSH
//! repo): the system prompt is exactly the one-line complete persona
//! `You are a helpful software engineer assistant.`, runtime-context
//! snapshots are suppressed, and exactly two model-facing tools are exposed —
//! `bash` and `str_replace_editor` — with the descriptions and parameter
//! schemas authored there.
//!
//! Dirge reproduces that contract on the FIRST request of a fresh
//! DeepSeek-chat session: the system text is exactly
//! [`DSH_MINIMAL_SYSTEM_PROMPT`] and the request ships exactly
//! [`dsh_minimal_tool_defs`]. Request 2+ GROWS the contract instead of
//! swapping it ([`dsh_minimal_full_prompt`]): the one-line persona stays as a
//! strict byte-prefix and Dirge's normal preamble + full tool set are
//! appended, so the provider's prefix cache carries request 1's tokens into
//! request 2 and the model's identity/tool surface never re-anchors.

use rig::completion::ToolDefinition;

/// The complete system prompt of the DSH minimal preset, byte-exact:
/// `You are a helpful software engineer assistant.`
pub const DSH_MINIMAL_SYSTEM_PROMPT: &str = "You are a helpful software engineer assistant.";

/// The minimal preset's `bash` tool description, verbatim from the
/// `description: |-` block of the preset composition.
pub const DSH_MINIMAL_BASH_DESCRIPTION: &str = "\
Run commands in a bash shell
* When invoking this tool, the contents of the \"command\" parameter does NOT need to be XML-escaped.
* You don't have access to the internet via this tool.
* You do have access to a mirror of common linux and python packages via apt and pip.
* State is persistent across command calls and discussions with the user.
* To inspect a particular line range of a file, e.g. lines 10-25, try 'sed -n 10,25p /path/to/the/file'.
* Please avoid commands that may produce a very large amount of output.
* Please run long lived commands in the background, e.g. 'sleep 10 &' or start a server in the background.";

/// The minimal preset's `str_replace_editor` tool description, verbatim from
/// the tool's `DEFAULT_DESCRIPTION` (the preset only overrides
/// `maxOutputChars`).
pub const DSH_MINIMAL_EDITOR_DESCRIPTION: &str = "\
Custom editing tool for viewing, creating and editing files
* State is persistent across command calls and discussions with the user
* If `path` is a file, `view` displays the result of applying `cat -n`. If `path` is a directory, `view` lists non-hidden files and directories up to 2 levels deep
* The `create` command cannot be used if the specified `path` already exists as a file
* If a `command` generates a long output, it will be truncated and marked with `<response clipped>`

Notes for using the `str_replace` command:
* The `old_str` parameter should match EXACTLY one or more consecutive lines from the original file. Be mindful of whitespaces!
* If the `old_str` parameter is not unique in the file, the replacement will not be performed. Make sure to include enough context in `old_str` to make it unique
* The `new_str` parameter should contain the edited lines that should replace the `old_str`";

/// The `str_replace_editor` output-truncation marker (DSH
/// `TRUNCATED_MESSAGE`).
pub const DSH_MINIMAL_EDITOR_TRUNCATED_MESSAGE: &str = "<response clipped><NOTE>To save on context only part of this file has been shown to you. You should retry this tool after you have searched inside the file with `grep -n` in order to find the line numbers of what you are looking for.</NOTE>";

/// The minimal preset's view/edit output cap (`maxOutputChars: 16000`).
pub const DSH_MINIMAL_EDITOR_MAX_OUTPUT_CHARS: usize = 16_000;

/// The exact model-facing tool definitions of the DSH minimal preset: `bash`
/// then `str_replace_editor` (DSH ships tools in lexicographic order; the
/// request also carries them in this order).
pub fn dsh_minimal_tool_defs() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "bash".to_string(),
            description: DSH_MINIMAL_BASH_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to run. Relative path is preferred in the command."
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDefinition {
            name: "str_replace_editor".to_string(),
            description: DSH_MINIMAL_EDITOR_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["view", "create", "str_replace", "insert"],
                        "description": "The commands to run. Allowed options are: `view`, `create`, `str_replace`, `insert`."
                    },
                    "path": {
                        "type": "string",
                        "description": "Absolute path to file or directory, e.g. `/repo/file.py` or `/repo`."
                    },
                    "file_text": {
                        "type": "string",
                        "description": "Required parameter of `create` command, with the content of the file to be created."
                    },
                    "insert_line": {
                        "type": "integer",
                        "description": "Required parameter of `insert` command. The `new_str` will be inserted AFTER the line `insert_line` of `path`."
                    },
                    "new_str": {
                        "type": "string",
                        "description": "Optional parameter of `str_replace` command containing the new string (if not given, no string will be added). Required parameter of `insert` command containing the string to insert."
                    },
                    "old_str": {
                        "type": "string",
                        "description": "Required parameter of `str_replace` command containing the string in `path` to replace."
                    },
                    "view_range": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "description": "Optional parameter of `view` command when `path` points to a file. If none is given, the full file is shown. If provided, the file will be shown in the indicated line number range, e.g. [11, 12] will show lines 11 and 12. Indexing at 1 to start. Setting `[start_line, -1]` shows all lines from `start_line` to the end of the file."
                    }
                },
                "required": ["command", "path"]
            }),
        },
    ]
}

/// The request-2+ system prompt: the minimal one line first, then Dirge's
/// full preamble appended after a blank line. Request 1's exact system bytes
/// are a strict prefix of this, which is what lets the provider's prefix
/// cache carry the minimal block into every later request.
pub fn dsh_minimal_full_prompt(full_preamble: &str) -> String {
    format!("{DSH_MINIMAL_SYSTEM_PROMPT}\n\n{full_preamble}")
}

/// The placeholder Dirge substitutes for the user's real first message on
/// request 1 of a fresh DeepSeek-chat session. The real message stays in the
/// transcript and reaches the model on request 2 — the same request that
/// first ships the full preamble and full tool set.
pub const DSH_MINIMAL_HI_PLACEHOLDER: &str = "hi";

/// Rewrite the FIRST user-role message's content to the `"hi"` placeholder,
/// in place. A no-op when no user message is present; only the first one
/// changes and later messages are untouched.
///
/// This is a WIRE-only rewrite applied to the per-request serialized message
/// list in `stream_assistant_response` while the first-request slot is armed —
/// `context.messages` (the transcript) is never touched, so the real first
/// user message survives to request 2.
pub(crate) fn rewrite_first_user_message(msgs: &mut [serde_json::Value]) -> bool {
    if let Some(first) = msgs.iter_mut().find(|m| role(m) == "user") {
        first["content"] = serde_json::Value::String(DSH_MINIMAL_HI_PLACEHOLDER.to_string());
        true
    } else {
        false
    }
}

fn role(m: &serde_json::Value) -> &str {
    m.get("role").and_then(serde_json::Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_is_the_exact_dsh_line() {
        assert_eq!(DSH_MINIMAL_SYSTEM_PROMPT, "You are a helpful software engineer assistant.");
        assert!(!DSH_MINIMAL_SYSTEM_PROMPT.ends_with('\n'));
    }

    #[test]
    fn tool_defs_are_exactly_the_two_dsh_tools_in_order() {
        let defs = dsh_minimal_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "str_replace_editor"]);
    }

    #[test]
    fn bash_description_is_verbatim_from_the_preset() {
        let defs = dsh_minimal_tool_defs();
        let bash = &defs[0];
        assert_eq!(bash.description, DSH_MINIMAL_BASH_DESCRIPTION);
        assert!(bash.description.starts_with("Run commands in a bash shell"));
        assert!(bash.description.contains("XML-escaped"));
        assert!(bash.description.ends_with("start a server in the background."));
        assert!(!bash.description.ends_with('\n'));
    }

    #[test]
    fn bash_parameters_are_command_only() {
        let defs = dsh_minimal_tool_defs();
        let params = &defs[0].parameters["properties"];
        let keys: Vec<&str> = params.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["command"]);
        assert_eq!(
            defs[0].parameters["required"],
            serde_json::json!(["command"])
        );
        assert_eq!(
            defs[0].parameters["properties"]["command"]["description"],
            "The bash command to run. Relative path is preferred in the command."
        );
    }

    #[test]
    fn editor_description_matches_dsh_default() {
        let defs = dsh_minimal_tool_defs();
        let editor = &defs[1];
        assert_eq!(editor.description, DSH_MINIMAL_EDITOR_DESCRIPTION);
        assert!(editor.description.starts_with("Custom editing tool for viewing, creating and editing files"));
        assert!(editor.description.contains("Notes for using the `str_replace` command:"));
        assert!(editor.description.ends_with("The `new_str` parameter should contain the edited lines that should replace the `old_str`"));
    }

    #[test]
    fn editor_parameters_match_dsh_schema() {
        let defs = dsh_minimal_tool_defs();
        let params = &defs[1].parameters;
        assert_eq!(params["type"], "object");
        assert_eq!(params["required"], serde_json::json!(["command", "path"]));
        let props = params["properties"].as_object().unwrap();
        let names: Vec<&str> = props.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec!["command", "path", "file_text", "insert_line", "new_str", "old_str", "view_range"]
        );
        assert_eq!(
            props["command"]["enum"],
            serde_json::json!(["view", "create", "str_replace", "insert"])
        );
        assert_eq!(props["insert_line"]["type"], "integer");
        assert_eq!(props["view_range"]["items"]["type"], "integer");
        assert!(params["properties"].get("timeout").is_none());
        assert!(params["properties"].get("background").is_none());
    }

    #[test]
    fn full_prompt_keeps_the_minimal_line_as_a_byte_prefix() {
        let full = dsh_minimal_full_prompt("You are an expert coding assistant.");
        assert!(full.starts_with(DSH_MINIMAL_SYSTEM_PROMPT));
        assert!(full.len() > DSH_MINIMAL_SYSTEM_PROMPT.len());
        assert_eq!(full, "You are a helpful software engineer assistant.\n\nYou are an expert coding assistant.");
        // The minimal line itself is never used as a standalone suffix —
        // the growth is strictly append-after-the-prefix, so a swap never
        // happens and the cache prefix is untouched.
        let second = dsh_minimal_full_prompt("more");
        assert!(second.starts_with(full.split_once('\n').unwrap().0));
        assert_eq!(second, "You are a helpful software engineer assistant.\n\nmore");
    }

    #[test]
    fn rewrite_first_user_message_replaces_only_the_first_user_message() {
        let mut msgs = vec![
            serde_json::json!({"role": "user", "content": "real first message"}),
            serde_json::json!({"role": "assistant", "content": "Hi! How can I help?"}),
            serde_json::json!({"role": "user", "content": "a later injected note"}),
        ];
        let changed = rewrite_first_user_message(&mut msgs);
        assert!(changed);
        assert_eq!(msgs[0]["content"], serde_json::json!(DSH_MINIMAL_HI_PLACEHOLDER));
        // Only the first user message changes; the rest stay untouched.
        assert_eq!(msgs[1]["content"], serde_json::json!("Hi! How can I help?"));
        assert_eq!(msgs[2]["content"], serde_json::json!("a later injected note"));
    }

    #[test]
    fn rewrite_first_user_message_handles_multipart_content() {
        // A multipart (e.g. image) first user message also becomes "hi" on the
        // wire; the real part array lives only in the transcript.
        let mut msgs = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": "look at this"},
                        {"type": "image", "source": {"type": "base64", "data": "..."}}]
        })];
        let changed = rewrite_first_user_message(&mut msgs);
        assert!(changed);
        assert_eq!(msgs[0]["content"], serde_json::json!(DSH_MINIMAL_HI_PLACEHOLDER));
    }

    #[test]
    fn rewrite_first_user_message_is_noop_without_a_user_message() {
        let mut msgs = vec![serde_json::json!({"role": "assistant", "content": "hello"})];
        assert!(!rewrite_first_user_message(&mut msgs));
        assert_eq!(msgs[0]["content"], serde_json::json!("hello"));
    }
}