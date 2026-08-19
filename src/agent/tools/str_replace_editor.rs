//! Faithful port of the DeepSeek Harness `str_replace_editor` tool.
//!
//! The DSH minimal preset exposes exactly two model-facing tools; this is the
//! second one (reference: `packages/fs/tool-str-replace-editor/src/index.ts`
//! in the DSH repo). The model-facing contract — name, description,
//! parameter schema, and the user-visible result/error strings below — is
//! copied verbatim so a Dirge session reproduces the DSH tool exactly.
//!
//! Semantics: a single tool with four subcommands over the filesystem:
//! `view` (cat -n style numbered view of a file, or a 2-level directory
//! listing), `create` (refuse-if-exists), `str_replace` (exactly-one exact
//! substring match), and `insert` (line-indexed splice). Long outputs
//! truncate at 16000 chars with DSH's `<response clipped>` marker.
//!
//! Execution differences from DSH (deliberate, backed by Dirge's own
//! infrastructure): mutations route through Dirge's permission checker
//! ([`require_and_resolve_rooted`]) plus its atomic-write, snapshot, and
//! tool-cache invalidation layers, instead of DSH's sandbox policy.

use std::future::Future;
use std::pin::Pin;

use serde_json::{Value, json};

use crate::agent::agent_loop::dsh_minimal::{
    DSH_MINIMAL_EDITOR_DESCRIPTION, DSH_MINIMAL_EDITOR_MAX_OUTPUT_CHARS,
    DSH_MINIMAL_EDITOR_TRUNCATED_MESSAGE,
};
use crate::agent::agent_loop::tool::{AbortSignal, LoopToolUpdate};
use crate::agent::agent_loop::{
    LoopTool, LoopToolResult,
};
use crate::agent::agent_loop::types::ToolExecutionMode;
use crate::agent::tools::cache::ToolCache;
use crate::agent::tools::{
    AskSender, PermCheck, ToolError, ToolRoot, require_and_resolve_rooted,
};

/// Tool name as the model sees it (DSH tool `str_replace_editor`).
pub const STR_REPLACE_EDITOR_NAME: &str = "str_replace_editor";

fn err(msg: impl Into<String>) -> String {
    msg.into()
}

fn to_message(e: ToolError) -> String {
    match e {
        ToolError::Msg(m) => m,
    }
}

/// Value handled like a JS number where `Number.isInteger` must hold.
fn as_js_integer(v: &Value) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(f) = v.as_f64()
        && f.fract() == 0.0
        && f.is_finite()
    {
        return Some(f as i64);
    }
    None
}

/// DSH `matchOffsets`: every byte offset where `needle` appears,
/// non-overlapping (JS `indexOf` loop advancing by `match + search.length`).
fn match_offsets(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return (0..=haystack.len()).collect();
    }
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    let mut offsets = Vec::new();
    let mut i = 0;
    while i < hay.len() {
        match hay[i..].windows(ned.len()).position(|w| w == ned) {
            Some(p) => {
                offsets.push(i + p);
                i += p + ned.len();
            }
            None => break,
        }
    }
    offsets
}

/// DSH `lineNumbersAt`: 1-based line number of each byte offset.
fn line_numbers_at(content: &str, offsets: &[usize]) -> Vec<usize> {
    let bytes = content.as_bytes();
    offsets
        .iter()
        .map(|&off| bytes[..off].iter().filter(|&&b| b == b'\n').count() + 1)
        .collect()
}

/// DSH `maybeTruncate`: keep the head, append the clipping marker.
fn maybe_truncate(content: String, max: usize) -> String {
    if content.len() <= max {
        content
    } else {
        let head: String = content.chars().take(max).collect();
        format!("{head}{DSH_MINIMAL_EDITOR_TRUNCATED_MESSAGE}")
    }
}

/// DSH `formatFileView`: numbered cat -n rendering with optional range.
fn format_file_view(path: &str, content: &str, view_range: Option<(i64, i64)>) -> Result<String, String> {
    let all_lines: Vec<&str> = content.split('\n').collect();
    let mut initial_line = 1usize;
    let mut final_line: i64 = all_lines.len() as i64;
    let mut prompt = format!(
        "Here's the content of {path} with line numbers (which has a total of {} lines)",
        all_lines.len()
    );
    if let Some((a, b)) = view_range {
        if a < 1 || a as usize > all_lines.len() {
            return Err(err(format!(
                "Invalid `view_range`: [{a}, {b}]. Its first element `{a}` should be within the range of lines of the file: [1, {}]",
                all_lines.len()
            )));
        }
        final_line = b;
        if final_line > all_lines.len() as i64 {
            return Err(err(format!(
                "Invalid `view_range`: [{a}, {b}]. Its second element `{b}` should be smaller than the number of lines in the file: `{}`",
                all_lines.len()
            )));
        }
        if final_line != -1 && final_line < a {
            return Err(err(format!(
                "Invalid `view_range`: [{a}, {b}]. Its second element `{b}` should be larger or equal than its first `{a}`"
            )));
        }
        initial_line = a as usize;
        prompt.push_str(&format!(" with view_range=[{a}, {b}]"));
    }
    let lines: Vec<&str> = if final_line == -1 {
        all_lines[initial_line - 1..].to_vec()
    } else {
        all_lines[initial_line - 1..final_line as usize].to_vec()
    };
    let numbered = lines
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6}  {}", initial_line + i, line))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(maybe_truncate(
        format!("{prompt}:\n{numbered}\n"),
        DSH_MINIMAL_EDITOR_MAX_OUTPUT_CHARS,
    ))
}

/// The model-facing tool. Registered in the loop registry so both the DSH
/// minimal first request and normal sessions can dispatch it.
pub struct StrReplaceEditorTool {
    permission: Option<PermCheck>,
    ask_tx: Option<AskSender>,
    cache: Option<ToolCache>,
    root: Option<ToolRoot>,
    parameters: Value,
}

impl StrReplaceEditorTool {
    #[allow(dead_code)]
    pub fn new(permission: Option<PermCheck>, ask_tx: Option<AskSender>) -> Self {
        Self {
            permission,
            ask_tx,
            cache: None,
            root: None,
            parameters: crate::agent::agent_loop::dsh_minimal::dsh_minimal_tool_defs()[1]
                .parameters
                .clone(),
        }
    }

    pub fn with_cache(
        permission: Option<PermCheck>,
        ask_tx: Option<AskSender>,
        cache: ToolCache,
    ) -> Self {
        Self {
            permission,
            ask_tx,
            cache: Some(cache),
            root: None,
            parameters: crate::agent::agent_loop::dsh_minimal::dsh_minimal_tool_defs()[1]
                .parameters
                .clone(),
        }
    }

    #[allow(dead_code)]
    pub fn rooted(mut self, root: ToolRoot) -> Self {
        self.root = Some(root);
        self
    }

    async fn resolve(&self, raw: &str) -> Result<String, String> {
        require_and_resolve_rooted(
            self.root.as_ref(),
            &self.permission,
            &self.ask_tx,
            STR_REPLACE_EDITOR_NAME,
            raw,
            "the editor path",
        )
        .await
        .map_err(to_message)
    }

    fn required_arg<'a>(args: &'a Value, key: &str, command: &str) -> Result<&'a str, String> {
        match args.get(key).and_then(Value::as_str) {
            Some(v) => Ok(v),
            None => Err(err(format!(
                "Parameter `{key}` is required for command: {command}"
            ))),
        }
    }

    async fn view(&self, path: &str, view_range: Option<(i64, i64)>) -> Result<String, String> {
        let meta = tokio::fs::metadata(path).await.map_err(|_| {
            err(format!(
                "The path {path} does not exist. Please provide a valid path."
            ))
        })?;
        if meta.is_dir() {
            if view_range.is_some() {
                return Err(err(
                    "The `view_range` parameter is not allowed when `path` points to a directory.",
                ));
            }
            return self.list_directory(path).await;
        }
        if !meta.is_file() {
            return Err(err(format!(
                "cannot view \"{path}\": not a regular file or directory"
            )));
        }
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| err(format!("Failed to read {path}: {e}")))?;
        if let Some(ref cache) = self.cache {
            cache.mark_read(std::path::Path::new(path));
        }
        format_file_view(path, &content, view_range)
    }

    async fn list_directory(&self, path: &str) -> Result<String, String> {
        // Iterative DFS (explicit stack) over the same rules as DSH's
        // `visit`: each entry produces a `{d|f|?}\t{abs-path}` row and
        // directories recurse while depth < 2.
        let mut rows = vec![format!("d\t{path}")];
        let mut stack = vec![(path.to_string(), 1usize)];
        while let Some((dir, depth)) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&dir)
                .await
                .map_err(|e| err(format!("Failed to list {dir}: {e}")))?;
            let mut dirs = Vec::new();
            while let Some(entry) = entries.next_entry().await.map_err(|e| {
                err(format!("Failed to list {dir}: {e}"))
            })? {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name == "node_modules" || name == "__pycache__" {
                    continue;
                }
                let child = entry.path();
                let ft = entry
                    .file_type()
                    .await
                    .map_err(|e| err(format!("Failed to stat {child:?}: {e}")))?;
                let is_dir = ft.is_dir();
                let ty = if is_dir {
                    'd'
                } else if ft.is_file() {
                    'f'
                } else {
                    '?'
                };
                let display = child.to_string_lossy().into_owned();
                rows.push(format!("{ty}\t{display}"));
                if is_dir && depth < 2 {
                    dirs.push(display);
                }
            }
            // LIFO order matches a pre-order traversal; the rows are sorted
            // by path below anyway.
            for d in dirs.into_iter().rev() {
                stack.push((d, depth + 1));
            }
        }
        rows.sort_by(|l, r| {
            let lp = l.split_once('\t').map(|(_, p)| p).unwrap_or(l);
            let rp = r.split_once('\t').map(|(_, p)| p).unwrap_or(r);
            lp.cmp(rp)
        });
        let listing = maybe_truncate(rows.join("\n") + "\n", DSH_MINIMAL_EDITOR_MAX_OUTPUT_CHARS);
        Ok(format!(
            "Here're the files and directories up to 2 levels deep in {path}, excluding hidden items, node_modules, and Python cache directories:\n{listing}\n"
        ))
    }

    async fn create(&self, path: &str, file_text: Option<&str>) -> Result<String, String> {
        let content = match file_text {
            Some(c) => c.to_string(),
            None => String::new(),
        };
        if tokio::fs::metadata(path).await.is_ok() {
            return Err(err(format!(
                "File already exists at: {path}. Cannot overwrite files using command `create`."
            )));
        }
        if let Some(parent) = std::path::Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                err(format!("Failed to create parent directories for {path}: {e}"))
            })?;
        }
        crate::agent::tools::snapshots::capture(std::path::Path::new(path));
        crate::fs_atomic::atomic_write(std::path::Path::new(path), content.as_bytes())
            .await
            .map_err(|e| err(format!("Failed to write {path}: {e}")))?;
        crate::agent::tools::modified::mark_modified(std::path::Path::new(path));
        if let Some(ref cache) = self.cache {
            cache.clear();
            cache.mark_read(std::path::Path::new(path));
        }
        Ok(format!("New file created successfully at: {path}"))
    }

    async fn str_replace(
        &self,
        path: &str,
        old_str: Option<&str>,
        new_str: Option<&str>,
    ) -> Result<String, String> {
        let old_value = match old_str {
            Some(v) => v,
            None => {
                return Err(err(
                    "Parameter `old_str` is required for command: str_replace",
                ))
            }
        };
        if old_value.is_empty() {
            return Err(err(
                "Parameter `old_str` is empty for command: str_replace",
            ));
        }
        let new_value = new_str.unwrap_or("");
        let meta = tokio::fs::metadata(path).await.map_err(|_| {
            err(format!(
                "The path {path} does not exist. Please provide a valid path."
            ))
        })?;
        if meta.is_dir() {
            return Err(err(format!(
                "The path {path} is a directory and only the `view` command can be used on directories"
            )));
        }
        if !meta.is_file() {
            return Err(err(format!("cannot edit \"{path}\": not a regular file")));
        }
        let before = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| err(format!("Failed to read {path}: {e}")))?;
        let offsets = match_offsets(&before, old_value);
        let Some(&offset) = offsets.first() else {
            return Err(err(format!(
                "No replacement was performed, old_str `{old_value}` did not appear verbatim in {path}."
            )));
        };
        if offsets.len() > 1 {
            let lines = line_numbers_at(&before, &offsets);
            return Err(err(format!(
                "No replacement was performed. Multiple occurrences of old_str `{old_value}` in lines [{}]. Please ensure it is unique",
                lines
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let after = format!(
            "{}{}{}",
            &before[..offset],
            new_value,
            &before[offset + old_value.len()..]
        );
        crate::agent::tools::snapshots::capture(std::path::Path::new(path));
        crate::fs_atomic::atomic_write(std::path::Path::new(path), after.as_bytes())
            .await
            .map_err(|e| err(format!("Failed to write {path}: {e}")))?;
        crate::agent::tools::modified::mark_modified(std::path::Path::new(path));
        if let Some(ref cache) = self.cache {
            cache.clear();
            cache.mark_read(std::path::Path::new(path));
        }
        Ok(format!("The file {path} has been edited successfully."))
    }

    async fn insert(
        &self,
        path: &str,
        insert_line: Option<&Value>,
        new_str: Option<&str>,
    ) -> Result<String, String> {
        let value = match new_str {
            Some(v) => v,
            None => {
                return Err(err("Parameter `new_str` is required for command: insert"))
            }
        };
        let insert_line = match insert_line {
            Some(v) => v,
            None => {
                return Err(err(
                    "Parameter `insert_line` is required for command: insert",
                ))
            }
        };
        let meta = tokio::fs::metadata(path).await.map_err(|_| {
            err(format!(
                "The path {path} does not exist. Please provide a valid path."
            ))
        })?;
        if meta.is_dir() {
            return Err(err(format!(
                "The path {path} is a directory and only the `view` command can be used on directories"
            )));
        }
        if !meta.is_file() {
            return Err(err(format!("cannot insert into \"{path}\": not a regular file")));
        }
        let before = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| err(format!("Failed to read {path}: {e}")))?;
        let lines: Vec<&str> = before.split('\n').collect();
        let Some(line) = as_js_integer(insert_line) else {
            return Err(err(format!(
                "Invalid `insert_line` parameter: {insert_line}. It should be within the range of lines of the file: [0, {}]",
                lines.len()
            )));
        };
        if line < 0 || line as usize > lines.len() {
            return Err(err(format!(
                "Invalid `insert_line` parameter: {line}. It should be within the range of lines of the file: [0, {}]",
                lines.len()
            )));
        }
        let at = line as usize;
        let mut after = lines[..at].to_vec();
        after.extend(value.split('\n'));
        after.extend_from_slice(&lines[at..]);
        let after = after.join("\n");
        crate::agent::tools::snapshots::capture(std::path::Path::new(path));
        crate::fs_atomic::atomic_write(std::path::Path::new(path), after.as_bytes())
            .await
            .map_err(|e| err(format!("Failed to write {path}: {e}")))?;
        crate::agent::tools::modified::mark_modified(std::path::Path::new(path));
        if let Some(ref cache) = self.cache {
            cache.clear();
            cache.mark_read(std::path::Path::new(path));
        }
        Ok(format!("The file {path} has been edited successfully."))
    }
}

impl std::fmt::Debug for StrReplaceEditorTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrReplaceEditorTool").finish_non_exhaustive()
    }
}

impl LoopTool for StrReplaceEditorTool {
    fn name(&self) -> &str {
        STR_REPLACE_EDITOR_NAME
    }

    fn description(&self) -> &str {
        DSH_MINIMAL_EDITOR_DESCRIPTION
    }

    fn label(&self) -> &str {
        STR_REPLACE_EDITOR_NAME
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        args: Value,
        _signal: AbortSignal,
        _on_update: LoopToolUpdate,
    ) -> Pin<Box<dyn Future<Output = Result<LoopToolResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let command = Self::required_arg(&args, "command", "?")?;
            let raw_path = match args.get("path").and_then(Value::as_str) {
                Some(p) => p,
                None => return Err(err("Parameter `path` is required for command")),
            };
            let resolved = self.resolve(raw_path).await?;
            let view_range = match args.get("view_range") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let arr = v
                        .as_array()
                        .ok_or_else(|| err("Invalid `view_range`. It should be a list of two integers."))?;
                    if arr.len() != 2 || arr.iter().any(|x| as_js_integer(x).is_none()) {
                        return Err(err(
                            "Invalid `view_range`. It should be a list of two integers.",
                        ));
                    }
                    Some((as_js_integer(&arr[0]).unwrap(), as_js_integer(&arr[1]).unwrap()))
                }
            };
            let output = match command {
                "view" => self.view(&resolved, view_range).await?,
                "create" => self.create(&resolved, args.get("file_text").and_then(Value::as_str)).await?,
                "str_replace" => {
                    self.str_replace(
                        &resolved,
                        args.get("old_str").and_then(Value::as_str),
                        args.get("new_str").and_then(Value::as_str),
                    )
                    .await?
                }
                "insert" => {
                    self.insert(
                        &resolved,
                        args.get("insert_line"),
                        args.get("new_str").and_then(Value::as_str),
                    )
                    .await?
                }
                other => return Err(err(format!("Invalid command: {other}"))),
            };
            Ok(LoopToolResult {
                content: vec![json!({ "type": "text", "text": output })],
                details: Value::Null,
                terminate: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::agent_loop::tool::AbortSignal;
    use crate::agent::agent_loop::LoopToolResult;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dirge_sre_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run a command through the tool with the given args; returns the
    /// tool-result text (or Err message).
    async fn run(tool: &StrReplaceEditorTool, args: Value) -> Result<String, String> {
        match tool.execute("t1", args, AbortSignal::new(), std::sync::Arc::new(|_| {})).await {
            Ok(LoopToolResult { content, .. }) => Ok(content
                .first()
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()),
            Err(e) => Err(e),
        }
    }

    #[tokio::test]
    async fn create_then_view_shows_cat_n_numbering() {
        let dir = tmpdir("create_view");
        let file = dir.join("a.txt");
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);

        let out = run(
            &tool,
            json!({"command": "create", "path": file_s, "file_text": "hello\nworld"}),
        )
        .await
        .unwrap();
        assert_eq!(out, format!("New file created successfully at: {file_s}"));

        let out = run(&tool, json!({"command": "view", "path": file_s})).await.unwrap();
        assert_eq!(
            out,
            format!(
                "Here's the content of {file_s} with line numbers (which has a total of 2 lines):\n     1  hello\n     2  world\n"
            )
        );
    }

    #[tokio::test]
    async fn create_refuses_when_file_exists() {
        let dir = tmpdir("create_exists");
        let file = dir.join("x.rs");
        std::fs::write(&file, "old").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(
            &tool,
            json!({"command": "create", "path": file_s, "file_text": "new"}),
        )
        .await
        .unwrap_err();
        assert!(out.contains("File already exists at:"));
        assert!(out.contains("Cannot overwrite files using command `create`."));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
    }

    #[tokio::test]
    async fn str_replace_success_unique_and_ambiguous() {
        let dir = tmpdir("replace");
        let file = dir.join("r.txt");
        std::fs::write(&file, "one two one").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);

        let out = run(
            &tool,
            json!({"command": "str_replace", "path": file_s, "old_str": "two", "new_str": "three"}),
        )
        .await
        .unwrap();
        assert_eq!(out, format!("The file {file_s} has been edited successfully."));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one three one");

        // Now "one" occurs twice -> ambiguous.
        let out = run(
            &tool,
            json!({"command": "str_replace", "path": file_s, "old_str": "one", "new_str": "x"}),
        )
        .await
        .unwrap_err();
        assert!(out.contains("Multiple occurrences of old_str `one` in lines [1, 1]"), "{out}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one three one");
    }

    #[tokio::test]
    async fn str_replace_empty_old_str_is_rejected() {
        let dir = tmpdir("replace_empty");
        let file = dir.join("e.txt");
        std::fs::write(&file, "abc").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(
            &tool,
            json!({"command": "str_replace", "path": file_s, "old_str": "", "new_str": "n"}),
        )
        .await
        .unwrap_err();
        assert_eq!(out, "Parameter `old_str` is empty for command: str_replace");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "abc");
    }

    #[tokio::test]
    async fn str_replace_matches_are_non_overlapping() {
        // DSH's `matchOffsets` advances by the match length (no overlaps):
        // in "aaa", "aa" occurs once, so the replacement is unambiguous.
        let dir = tmpdir("replace_overlap");
        let file = dir.join("o.txt");
        std::fs::write(&file, "aaa").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(
            &tool,
            json!({"command": "str_replace", "path": file_s, "old_str": "aa", "new_str": "b"}),
        )
        .await
        .unwrap();
        assert_eq!(out, format!("The file {file_s} has been edited successfully."));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ba");
    }

    #[tokio::test]
    async fn str_replace_on_directory_is_refused() {
        let dir = tmpdir("replace_dir");
        let dir_s = dir.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(
            &tool,
            json!({"command": "str_replace", "path": dir_s, "old_str": "a", "new_str": "b"}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            out,
            format!("The path {dir_s} is a directory and only the `view` command can be used on directories")
        );
    }

    #[tokio::test]
    async fn str_replace_missing_old_str_reports_not_found() {
        let dir = tmpdir("replace_missing");
        let file = dir.join("m.txt");
        std::fs::write(&file, "abc").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(
            &tool,
            json!({"command": "str_replace", "path": file_s, "old_str": "zzz", "new_str": "n"}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            out,
            format!("No replacement was performed, old_str `zzz` did not appear verbatim in {file_s}.")
        );
    }

    #[tokio::test]
    async fn insert_splices_at_line_index() {
        let dir = tmpdir("insert");
        let file = dir.join("i.txt");
        std::fs::write(&file, "a\nc").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);

        let out = run(
            &tool,
            json!({"command": "insert", "path": file_s, "insert_line": 1, "new_str": "b"}),
        )
        .await
        .unwrap();
        assert_eq!(out, format!("The file {file_s} has been edited successfully."));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a\nb\nc");

        // Out of range.
        let out = run(
            &tool,
            json!({"command": "insert", "path": file_s, "insert_line": 99, "new_str": "x"}),
        )
        .await
        .unwrap_err();
        assert!(out.contains("Invalid `insert_line` parameter: 99"), "{out}");
        assert!(out.contains("[0, 3]"), "{out}");
    }

    #[tokio::test]
    async fn view_range_validates_bounds() {
        let dir = tmpdir("view_range");
        let file = dir.join("v.txt");
        std::fs::write(&file, "a\nb\nc\nd").unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);

        let out = run(
            &tool,
            json!({"command": "view", "path": file_s, "view_range": [2, 3]}),
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            format!(
                "Here's the content of {file_s} with line numbers (which has a total of 4 lines) with view_range=[2, 3]:\n     2  b\n     3  c\n"
            )
        );

        let out = run(
            &tool,
            json!({"command": "view", "path": file_s, "view_range": [0, 3]}),
        )
        .await
        .unwrap_err();
        assert!(out.contains("Its first element `0` should be within the range of lines"), "{out}");
    }

    #[tokio::test]
    async fn view_directory_lists_two_levels() {
        let dir = tmpdir("dir_list");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("top.txt"), "t").unwrap();
        std::fs::write(dir.join("sub").join("inner.txt"), "i").unwrap();
        std::fs::write(dir.join(".hidden"), "h").unwrap();
        let dir_s = dir.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(&tool, json!({"command": "view", "path": dir_s})).await.unwrap();
        assert!(out.starts_with(&format!(
            "Here're the files and directories up to 2 levels deep in {dir_s}, excluding hidden items, node_modules, and Python cache directories:\n"
        )), "{out}");
        assert!(out.contains(&format!("d\t{dir_s}\n")));
        assert!(out.contains("d\t") && out.contains("/sub"));
        assert!(out.contains("f\t") && out.contains("/top.txt"));
        assert!(out.contains("/inner.txt"));
        assert!(!out.contains(".hidden"), "hidden entries must be excluded: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn long_output_is_truncated_with_marker() {
        let dir = tmpdir("truncate");
        let file = dir.join("big.txt");
        let big = "x".repeat(DSH_MINIMAL_EDITOR_MAX_OUTPUT_CHARS + 100);
        std::fs::write(&file, &big).unwrap();
        let file_s = file.to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(&tool, json!({"command": "view", "path": file_s})).await.unwrap();
        assert!(out.contains(DSH_MINIMAL_EDITOR_TRUNCATED_MESSAGE), "must clip with the DSH marker");
        assert!(!out.ends_with("x"), "must not carry the tail after the marker");
    }

    #[tokio::test]
    async fn view_missing_path_errors() {
        let dir = tmpdir("missing");
        let file_s = dir.join("nope.txt").to_string_lossy().into_owned();
        let tool = StrReplaceEditorTool::new(None, None);
        let out = run(&tool, json!({"command": "view", "path": file_s})).await.unwrap_err();
        assert_eq!(out, format!("The path {file_s} does not exist. Please provide a valid path."));
    }
}