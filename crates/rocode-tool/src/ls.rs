use async_trait::async_trait;
use glob::Pattern;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::{Metadata, PermissionRequest, Tool, ToolContext, ToolError, ToolResult};

const IGNORE_PATTERNS: &[&str] = &[
    "node_modules/",
    "__pycache__/",
    ".git/",
    "dist/",
    "build/",
    "target/",
    "vendor/",
    "bin/",
    "obj/",
    ".idea/",
    ".vscode/",
    ".zig-cache/",
    "zig-out",
    ".coverage",
    "coverage/",
    "vendor/",
    "tmp/",
    "temp/",
    ".cache/",
    "cache/",
    "logs/",
    ".venv/",
    "venv/",
    "env/",
];

const LIMIT: usize = 100;

pub struct LsTool {}

impl LsTool {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for LsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Deserialize)]
struct LsInput {
    path: Option<String>,
    ignore: Option<Vec<String>>,
}

fn has_glob_meta(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
}

#[async_trait]
impl Tool for LsTool {
    fn id(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "Lists files and directories in a given path."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The absolute path to the directory to list (must be absolute, not relative)"
                },
                "ignore": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of glob patterns to ignore"
                }
            },
            "required": []
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let input: LsInput = serde_json::from_value(args).unwrap_or(LsInput {
            path: None,
            ignore: None,
        });

        let requested_path = input.path.unwrap_or_else(|| ".".to_string());
        let mut base_dir = if Path::new(&requested_path).is_absolute() {
            PathBuf::from(&requested_path)
        } else {
            PathBuf::from(&ctx.directory).join(&requested_path)
        };
        if let Ok(canonical) = base_dir.canonicalize() {
            base_dir = canonical;
        }
        let base_dir_str = base_dir.to_string_lossy().to_string();

        ctx.ask_permission(
            PermissionRequest::new("list")
                .with_pattern(&base_dir_str)
                .with_metadata("path", serde_json::json!(&base_dir_str))
                .always_allow(),
        )
        .await?;

        if !base_dir.exists() {
            return Err(ToolError::FileNotFound(base_dir.display().to_string()));
        }

        if !base_dir.is_dir() {
            return Err(ToolError::ExecutionError(format!(
                "{} is not a directory",
                base_dir.display()
            )));
        }

        let mut ignore_set: HashSet<String> = IGNORE_PATTERNS
            .iter()
            .map(|s| s.trim_end_matches('/').to_string())
            .collect();
        let mut ignore_globs: Vec<Pattern> = Vec::new();

        if let Some(custom_ignore) = input.ignore {
            for pattern in custom_ignore {
                let normalized = pattern.trim_start_matches('!').trim();
                if normalized.is_empty() {
                    continue;
                }

                if has_glob_meta(normalized) {
                    if let Ok(glob) = Pattern::new(normalized) {
                        ignore_globs.push(glob);
                    }
                } else {
                    ignore_set.insert(normalized.trim_end_matches('/').to_string());
                }
            }
        }

        let mut files: Vec<String> = Vec::new();
        for entry in WalkDir::new(&base_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let Ok(rel_path) = entry.path().strip_prefix(&base_dir) else {
                // Skip entries outside the requested directory (e.g. followed symlinks).
                continue;
            };
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");

            if rel_str.is_empty() {
                continue;
            }

            let should_skip = rel_str.split('/').any(|part| ignore_set.contains(part))
                || ignore_globs.iter().any(|glob| glob.matches(&rel_str));

            if should_skip {
                continue;
            }

            if entry.file_type().is_file() {
                files.push(rel_str);
                if files.len() >= LIMIT {
                    break;
                }
            }
        }

        files.sort();

        let output = format!("{}/\n{}", base_dir.display(), files.join("\n"));

        let title = match base_dir.strip_prefix(Path::new(&ctx.worktree)) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.to_string_lossy().to_string(),
            Err(_) => base_dir.display().to_string(),
        };

        Ok(ToolResult {
            title,
            output,
            metadata: {
                let mut m = Metadata::new();
                m.insert("count".into(), serde_json::json!(files.len()));
                m.insert("truncated".into(), serde_json::json!(files.len() >= LIMIT));
                m
            },
            truncated: files.len() >= LIMIT,
        })
    }
}
