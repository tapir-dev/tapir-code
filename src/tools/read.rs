// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The `read` tool: return a slice of a text file, head-truncated so a huge
//! file never blows up a run.

use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use tapir::tool::ToolError;

use crate::tools::workspace::resolve_in_root;

/// Head-truncation ceilings: whichever is hit first ends the output.
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 256 * 1024;

/// Arguments to the `read` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// Path to the file, relative to the workspace root.
    pub path: String,
    /// 1-indexed line to start reading from. Defaults to the first line.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return from the offset.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Read a slice of the text file at `args.path`, confined to `root`.
///
/// # Errors
/// Returns a [`ToolError`] when the path escapes the root, the file cannot be
/// read, its bytes are not UTF-8 text, or `offset` points past the end.
pub fn read_in(root: &Path, args: ReadArgs) -> Result<String, ToolError> {
    let path = resolve_in_root(root, &args.path)?;
    let bytes = std::fs::read(&path).map_err(|e| {
        ToolError::model(format!("cannot read `{}`: {e}", args.path))
            .with_operator(e)
    })?;

    if looks_binary(&bytes) {
        return Ok(format!(
            "`{}` is not a UTF-8 text file ({} bytes); not shown.",
            args.path,
            bytes.len()
        ));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            return Ok(format!(
                "`{}` is not UTF-8 text; not shown.",
                args.path
            ));
        }
    };

    let lines: Vec<&str> = text.lines().collect();
    let start = args.offset.map_or(0, |o| o.saturating_sub(1));
    if start > lines.len() {
        return Err(ToolError::model(format!(
            "offset {} is past the end of `{}` ({} lines)",
            args.offset.unwrap_or(0),
            args.path,
            lines.len()
        )));
    }

    let after_offset = &lines[start..];
    let limit = args.limit.unwrap_or(after_offset.len());
    let window = &after_offset[..limit.min(after_offset.len())];

    Ok(render(window, start))
}

/// Join `window` (line-slice starting at 0-indexed `start`) back into text,
/// stopping at the first ceiling and appending a continuation notice when the
/// window is cut short.
fn render(window: &[&str], start: usize) -> String {
    let mut out = String::new();
    let mut shown = 0;
    let mut truncated = false;
    for line in window {
        if shown == MAX_LINES {
            truncated = true;
            break;
        }
        if out.len() + line.len() + 1 > MAX_BYTES {
            // A single line that alone overflows the byte budget: emit as much
            // of it as fits so the read still advances, then stop. Counting it
            // as shown keeps the continuation offset moving past it.
            if shown == 0 {
                let mut cut = MAX_BYTES.saturating_sub(1).min(line.len());
                while cut > 0 && !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&line[..cut]);
                out.push('\n');
                shown += 1;
            }
            truncated = true;
            break;
        }
        out.push_str(line);
        out.push('\n');
        shown += 1;
    }

    if truncated {
        let next = start + shown + 1;
        out.push_str(&format!(
            "\n[truncated after {shown} lines; continue with offset {next}]\n"
        ));
    }
    out
}

/// A file is treated as binary if a NUL byte appears in its head — the cheap,
/// conventional text/binary sniff.
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        crate::tools::testutil::temp_root("read", tag)
    }

    fn args(
        path: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> ReadArgs {
        ReadArgs {
            path: path.to_string(),
            offset,
            limit,
        }
    }

    #[test]
    fn reads_a_whole_small_file() {
        let root = temp_root("whole");
        fs::write(root.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        let out = read_in(&root, args("f.txt", None, None)).unwrap();
        assert_eq!(out, "one\ntwo\nthree\n");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn applies_offset_and_limit() {
        let root = temp_root("slice");
        fs::write(root.join("f.txt"), "a\nb\nc\nd\ne\n").unwrap();
        let out = read_in(&root, args("f.txt", Some(2), Some(2))).unwrap();
        assert_eq!(out, "b\nc\n");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn truncates_with_a_continuation_notice() {
        let root = temp_root("trunc");
        let body: String =
            (0..MAX_LINES + 500).map(|i| format!("line{i}\n")).collect();
        fs::write(root.join("big.txt"), &body).unwrap();

        let out = read_in(&root, args("big.txt", None, None)).unwrap();
        assert!(out.contains(&format!("truncated after {MAX_LINES} lines")));
        assert!(
            out.contains(&format!("continue with offset {}", MAX_LINES + 1))
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn errors_when_offset_past_end() {
        let root = temp_root("past");
        fs::write(root.join("f.txt"), "a\nb\n").unwrap();
        let err = read_in(&root, args("f.txt", Some(99), None)).unwrap_err();
        assert!(err.model_message.contains("past the end"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reports_binary_content_without_bytes() {
        let root = temp_root("bin");
        fs::write(root.join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let out = read_in(&root, args("b.bin", None, None)).unwrap();
        assert!(out.contains("not a UTF-8 text file"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reports_non_utf8_text_as_a_note() {
        let root = temp_root("latin1");
        // 0xFF is invalid UTF-8 but carries no NUL, so it slips past the sniff.
        fs::write(root.join("l.txt"), [b'h', b'i', 0xFF]).unwrap();
        let out = read_in(&root, args("l.txt", None, None)).unwrap();
        assert!(out.contains("not UTF-8 text"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn truncates_a_single_oversized_line_and_advances() {
        let root = temp_root("hugeline");
        let giant = "a".repeat(MAX_BYTES * 2);
        fs::write(root.join("one.txt"), &giant).unwrap();

        let out = read_in(&root, args("one.txt", None, None)).unwrap();
        // Progress is made — the notice points at the next line, not the same
        // offset, so a follow-up read does not loop.
        assert!(out.contains("truncated after 1 lines"));
        assert!(out.contains("continue with offset 2"));
        assert!(out.len() < giant.len());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_a_path_outside_root() {
        let root = temp_root("escape");
        let err = read_in(&root, args("../x", None, None)).unwrap_err();
        assert!(err.model_message.contains("escapes the workspace root"));
        fs::remove_dir_all(&root).ok();
    }
}
