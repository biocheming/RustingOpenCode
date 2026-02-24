use std::collections::HashMap;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;

use crate::theme::Theme;

#[derive(Clone, Copy)]
pub enum ToolState {
    Pending,
    Running,
    Completed,
    Failed,
}

/// Threshold: tool results longer than this are "block" tools with expandable output
const BLOCK_RESULT_THRESHOLD: usize = 3;

/// Map tool name to a semantic glyph
pub fn tool_glyph(name: &str) -> &'static str {
    match name {
        "bash" | "shell" => "$",
        "read" | "readFile" | "read_file" => "→",
        "write" | "writeFile" | "write_file" => "←",
        "edit" | "editFile" | "edit_file" => "←",
        "glob" | "grep" | "search" | "ripgrep" => "✱",
        "list" | "ls" | "listDir" | "list_dir" => "→",
        "webfetch" | "web_fetch" | "fetch" => "%",
        "codesearch" | "code_search" => "◇",
        "websearch" | "web_search" => "◈",
        "task" | "subagent" => "#",
        "apply_patch" | "applyPatch" => "%",
        "todowrite" | "todo_write" | "todoRead" | "todo_read" => "☐",
        _ => "⚙",
    }
}

/// Returns true if this tool typically produces block-level output
fn is_block_tool(name: &str, result: Option<&(String, bool)>) -> bool {
    let normalized = normalize_tool_name(name);
    // Tools that always produce block output
    match normalized.as_str() {
        "bash" | "shell" | "apply_patch" => return true,
        _ => {}
    }
    // Otherwise, check result length
    if let Some((result_text, _)) = result {
        result_text.lines().count() > BLOCK_RESULT_THRESHOLD
    } else {
        false
    }
}

/// Render a single tool call as lines (inline or block style)
pub fn render_tool_call(
    id: &str,
    name: &str,
    arguments: &str,
    state: ToolState,
    tool_results: &HashMap<String, (String, bool)>,
    show_tool_details: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if matches!(state, ToolState::Completed) && !show_tool_details {
        return Vec::new();
    }

    let result = tool_results.get(id);
    let block_mode = is_block_tool(name, result);
    let normalized = normalize_tool_name(name);

    let glyph = tool_glyph(name);
    let is_denied =
        result.is_some_and(|(result_text, is_error)| *is_error && is_denied_result(result_text));

    let (state_icon, icon_style, name_style) = styles_for_state(state, is_denied, theme);

    let mut lines = Vec::new();

    if block_mode {
        let bg = theme.background_panel;
        let mut main_spans = vec![
            block_prefix(theme, bg),
            Span::styled(format!("{} ", state_icon), icon_style.bg(bg)),
            Span::styled(format!("{} ", glyph), icon_style.bg(bg)),
            Span::styled(name.to_string(), name_style.bg(bg)),
        ];

        if let Some(argument_preview) = tool_argument_preview(&normalized, arguments) {
            main_spans.push(Span::styled(
                format!("  {}", argument_preview),
                Style::default().fg(theme.text_muted).bg(bg),
            ));
        }

        if is_denied {
            main_spans.push(Span::styled(
                "  denied",
                Style::default()
                    .fg(theme.error)
                    .add_modifier(Modifier::BOLD)
                    .bg(bg),
            ));
        }

        lines.push(Line::from(main_spans));

        if let Some((result_text, is_error)) = result {
            if *is_error {
                let mut iter = result_text.lines();
                if let Some(first_line) = iter.next() {
                    lines.push(block_content_line(
                        format!("Error: {}", format_preview_line(first_line, 96)),
                        Style::default().fg(theme.error),
                        theme,
                        bg,
                    ));
                }

                if show_tool_details {
                    for line in iter.take(2) {
                        lines.push(block_content_line(
                            format_preview_line(line, 96),
                            Style::default().fg(theme.error),
                            theme,
                            bg,
                        ));
                    }
                }
            } else if show_tool_details {
                let output_lines = result_text.lines().collect::<Vec<_>>();
                let line_count = output_lines.len();
                let preview_limit = if normalized == "bash" || normalized == "shell" {
                    10usize
                } else {
                    6usize
                };

                lines.push(block_content_line(
                    format!("({} lines of output)", line_count),
                    Style::default().fg(theme.text_muted),
                    theme,
                    bg,
                ));

                for line in output_lines.iter().take(preview_limit) {
                    lines.push(block_content_line(
                        format_preview_line(line, 96),
                        Style::default().fg(theme.text),
                        theme,
                        bg,
                    ));
                }

                if line_count > preview_limit {
                    lines.push(block_content_line(
                        format!("… ({} more lines)", line_count - preview_limit),
                        Style::default().fg(theme.text_muted),
                        theme,
                        bg,
                    ));
                }
            }
        }

        return lines;
    }

    // Inline mode
    let mut main_spans = vec![
        Span::styled(format!("{} ", state_icon), icon_style),
        Span::styled(format!("{} ", glyph), Style::default().fg(theme.tool_icon)),
        Span::styled(name.to_string(), name_style),
    ];

    // Inline result summary for completed non-block tools
    if let Some((result_text, is_error)) = result {
        if *is_error {
            let first_line = result_text.lines().next().unwrap_or(result_text).trim();
            main_spans.push(Span::styled(
                format!(" — {}", format_preview_line(first_line, 96)),
                Style::default().fg(theme.error),
            ));
            if is_denied {
                main_spans.push(Span::styled(
                    " (denied)",
                    Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                ));
            }
        } else {
            let line_count = result_text.lines().count();
            if line_count <= 1 {
                let summary = result_text.trim();
                if !summary.is_empty() && summary.len() <= 80 {
                    main_spans.push(Span::styled(
                        format!(" — {}", summary),
                        Style::default().fg(theme.text_muted),
                    ));
                }
            } else if let Some(first_line) =
                result_text.lines().find(|line| !line.trim().is_empty())
            {
                main_spans.push(Span::styled(
                    format!(
                        " — {} (+{} lines)",
                        format_preview_line(first_line, 72),
                        line_count.saturating_sub(1)
                    ),
                    Style::default().fg(theme.text_muted),
                ));
            }
        }
    }

    lines.push(Line::from(main_spans));

    if show_tool_details {
        if let Some(argument_preview) = tool_argument_preview(&normalized, arguments) {
            lines.push(Line::from(Span::styled(
                format!("    {}", argument_preview),
                Style::default().fg(theme.text_muted),
            )));
        }
    }

    lines
}

fn block_prefix(theme: &Theme, background: ratatui::style::Color) -> Span<'static> {
    Span::styled(
        "│ ",
        Style::default().fg(theme.border_subtle).bg(background),
    )
}

fn block_content_line(
    content: impl Into<String>,
    style: Style,
    theme: &Theme,
    background: ratatui::style::Color,
) -> Line<'static> {
    Line::from(vec![
        block_prefix(theme, background),
        Span::styled(format!("  {}", content.into()), style.bg(background)),
    ])
}

fn styles_for_state(
    state: ToolState,
    is_denied: bool,
    theme: &Theme,
) -> (&'static str, Style, Style) {
    match state {
        ToolState::Pending => (
            "◯",
            Style::default().fg(theme.warning),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Running => (
            "◐",
            Style::default().fg(theme.warning),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Completed => (
            "●",
            Style::default().fg(theme.success),
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        ToolState::Failed => {
            let mut name_style = Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD);
            if is_denied {
                name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
            }
            ("✗", Style::default().fg(theme.error), name_style)
        }
    }
}

fn normalize_tool_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

fn tool_argument_preview(normalized_name: &str, arguments: &str) -> Option<String> {
    let raw = arguments.trim();
    let parsed = serde_json::from_str::<Value>(raw).ok();
    let object = parsed.as_ref().and_then(|v| v.as_object());

    if normalized_name == "bash" || normalized_name == "shell" {
        let command = parsed
            .as_ref()
            .and_then(extract_shell_command)
            .or_else(|| (!raw.is_empty()).then_some(raw.to_string()))?;
        return Some(format!("$ {}", command.trim()));
    }

    if matches!(normalized_name, "read" | "readfile" | "read_file") {
        if let Some(path) = parsed.as_ref().and_then(extract_path) {
            return Some(format!("→ {}", path));
        }
    }

    if matches!(
        normalized_name,
        "list" | "ls" | "listdir" | "list_dir" | "list_directory"
    ) {
        if let Some(path) = parsed.as_ref().and_then(extract_path) {
            return Some(format!("→ {}", path));
        }
        return Some("→ .".to_string());
    }

    if matches!(
        normalized_name,
        "write" | "writefile" | "write_file" | "edit" | "editfile" | "edit_file"
    ) {
        if let Some(path) = parsed.as_ref().and_then(extract_path) {
            return Some(format!("← {}", path));
        }
    }

    if normalized_name == "glob" {
        if let Some(pattern) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["pattern"]))
        {
            let target = parsed.as_ref().and_then(extract_path);
            return Some(match target {
                Some(path) => format!("\"{}\" in {}", pattern, path),
                None => format!("\"{}\"", pattern),
            });
        }
    }

    if normalized_name == "grep" {
        if let Some(pattern) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["pattern", "query"]))
        {
            let target = parsed.as_ref().and_then(extract_path);
            return Some(match target {
                Some(path) => format!("\"{}\" in {}", pattern, path),
                None => format!("\"{}\"", pattern),
            });
        }
    }

    if matches!(normalized_name, "webfetch" | "web_fetch") {
        if let Some(url) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["url"]))
        {
            return Some(url);
        }
    }

    if matches!(
        normalized_name,
        "codesearch" | "code_search" | "websearch" | "web_search"
    ) {
        if let Some(query) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["query"]))
        {
            return Some(format!("\"{}\"", query));
        }
    }

    if normalized_name == "task" {
        let kind = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["subagent_type"]));
        let description = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["description"]));

        return match (kind, description) {
            (Some(kind), Some(description)) => Some(format!("{kind} task {description}")),
            (Some(kind), None) => Some(format!("{kind} task")),
            (None, Some(description)) => Some(description),
            (None, None) => None,
        };
    }

    if normalized_name == "question" {
        if let Some(count) = object
            .and_then(|value| value.get("questions"))
            .and_then(|value| value.as_array())
            .map(Vec::len)
        {
            return Some(format!(
                "Asked {} question{}",
                count,
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    if matches!(normalized_name, "todowrite" | "todo_write") {
        if let Some(count) = object
            .and_then(|value| value.get("todos"))
            .and_then(|value| value.as_array())
            .map(Vec::len)
        {
            return Some(format!(
                "Update {} todo{}",
                count,
                if count == 1 { "" } else { "s" }
            ));
        }
        return Some("Update todos".to_string());
    }

    if normalized_name == "skill" {
        if let Some(name) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["name"]))
        {
            return Some(format!("\"{}\"", name));
        }
    }

    if matches!(normalized_name, "apply_patch" | "applypatch") {
        return Some("Patch".to_string());
    }

    if normalized_name == "lsp" {
        if let Some(operation) = parsed
            .as_ref()
            .and_then(|value| extract_string_key(value, &["operation"]))
        {
            let target = parsed
                .as_ref()
                .and_then(|value| extract_string_key(value, &["filePath", "file_path", "path"]));
            return Some(match target {
                Some(path) => format!("{} {}", operation, path),
                None => operation,
            });
        }
    }

    if raw.is_empty() {
        return None;
    }

    if let Some(preview) = object.and_then(|value| {
        format_primitive_arguments(
            value,
            &[
                "content",
                "new_string",
                "old_string",
                "patch",
                "prompt",
                "questions",
                "todos",
            ],
        )
    }) {
        return Some(preview);
    }

    let first = raw.lines().next().unwrap_or(raw).trim();
    if first.is_empty() {
        None
    } else {
        Some(format_preview_line(first, 84))
    }
}

fn extract_shell_command(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in ["command", "cmd", "script", "input", "text"] {
        if let Some(command) = object.get(key).and_then(|v| v.as_str()) {
            let trimmed = command.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_path(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    for key in [
        "path",
        "file_path",
        "filePath",
        "file",
        "filename",
        "filepath",
        "absolute_path",
        "absolutePath",
        "target",
        "destination",
        "to",
        "from",
    ] {
        if let Some(path) = object.get(key).and_then(|v| v.as_str()) {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn extract_string_key(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        if let Some(content) = object.get(*key).and_then(|value| value.as_str()) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn format_primitive_arguments(
    object: &serde_json::Map<String, Value>,
    omit: &[&str],
) -> Option<String> {
    let mut parts = Vec::new();

    for (key, value) in object {
        if omit.contains(&key.as_str()) {
            continue;
        }

        let rendered = match value {
            Value::String(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    continue;
                }
                format_preview_line(trimmed, 28)
            }
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            _ => continue,
        };

        parts.push(format!("{key}={rendered}"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(format!("[{}]", parts.join(", ")))
    }
}

fn is_denied_result(result_text: &str) -> bool {
    let lower = result_text.to_ascii_lowercase();
    lower.contains("permission denied")
        || lower.contains("denied")
        || lower.contains("not permitted")
        || lower.contains("forbidden")
}

fn format_preview_line(line: &str, max_chars: usize) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated)
}

#[cfg(test)]
mod tests {
    use super::tool_argument_preview;

    #[test]
    fn list_tool_preview_shows_path() {
        let preview = tool_argument_preview("ls", r#"{"path":"."}"#);
        assert_eq!(preview.as_deref(), Some("→ ."));
    }

    #[test]
    fn read_tool_preview_supports_file_path_keys() {
        let preview = tool_argument_preview("read", r#"{"file_path":"/tmp/a.txt"}"#);
        assert_eq!(preview.as_deref(), Some("→ /tmp/a.txt"));
    }

    #[test]
    fn generic_preview_compacts_json_to_key_values() {
        let preview = tool_argument_preview("unknown", r#"{"path":".","recursive":true}"#);
        assert_eq!(preview.as_deref(), Some("[path=., recursive=true]"));
    }

    #[test]
    fn apply_patch_preview_hides_patch_body() {
        let preview = tool_argument_preview("apply_patch", "*** Begin Patch\n...");
        assert_eq!(preview.as_deref(), Some("Patch"));
    }
}
