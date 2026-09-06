// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The `edit` tool: apply one or more exact, unique text replacements to a
//! file in a single atomic write. Each `old_text` must match the original
//! contents exactly once; zero or many matches is an error, so an edit is never
//! silently applied in the wrong place (ADR 0002).

use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use tapir::tool::ToolError;

use crate::tools::workspace::{atomic_write, resolve_in_root};

/// One search-and-replace block.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditBlock {
    /// Exact text to find. Must occur exactly once in the file.
    pub old_text: String,
    /// Text to replace it with.
    pub new_text: String,
}

/// Arguments to the `edit` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditArgs {
    /// Path to the file, relative to the workspace root.
    pub path: String,
    /// The replacements to apply. All match against the original contents.
    pub edits: Vec<EditBlock>,
}

/// Apply `args.edits` to the file at `args.path`, confined to `root`.
///
/// # Errors
/// Returns a [`ToolError`] when the path escapes the root, the file cannot be
/// read as text, no edits are supplied, an `old_text` is empty, matches zero or
/// more than once, or two edits overlap.
pub fn edit_in(root: &Path, args: EditArgs) -> Result<String, ToolError> {
    if args.edits.is_empty() {
        return Err(ToolError::model("no edits supplied"));
    }
    let path = resolve_in_root(root, &args.path)?;
    let original = std::fs::read_to_string(&path).map_err(|e| {
        ToolError::model(format!("cannot read `{}`: {e}", args.path))
            .with_operator(e)
    })?;

    let (bom, body) = split_bom(&original);
    let crlf = body.contains("\r\n");

    // Locate every block's unique match against the original body, translating
    // the model's `\n` to the file's line ending first.
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for (i, block) in args.edits.iter().enumerate() {
        if block.old_text.is_empty() {
            return Err(ToolError::model(format!(
                "edit {}: old_text is empty",
                i + 1
            )));
        }
        let old = translate(&block.old_text, crlf);
        let new = translate(&block.new_text, crlf);
        let start = unique_match(body, &old, i, &args.path)?;
        spans.push((start, start + old.len(), new));
    }

    spans.sort_by_key(|(start, _, _)| *start);
    reject_overlaps(&spans, &args.path)?;

    // Splice the replacements into the body in order.
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    for (start, end, new) in &spans {
        out.push_str(&body[cursor..*start]);
        out.push_str(new);
        cursor = *end;
    }
    out.push_str(&body[cursor..]);

    let mut bytes = Vec::with_capacity(bom.len() + out.len());
    bytes.extend_from_slice(bom.as_bytes());
    bytes.extend_from_slice(out.as_bytes());
    atomic_write(&path, &bytes).map_err(|e| {
        ToolError::model(format!("cannot write `{}`: {e}", args.path))
            .with_operator(e)
    })?;

    let n = args.edits.len();
    let plural = if n == 1 { "edit" } else { "edits" };
    Ok(format!("applied {n} {plural} to `{}`", args.path))
}

/// Find the single byte offset where `needle` occurs in `haystack`, erroring on
/// zero or multiple matches with a message asking for more context.
fn unique_match(
    haystack: &str,
    needle: &str,
    index: usize,
    path: &str,
) -> Result<usize, ToolError> {
    let mut matches = haystack.match_indices(needle);
    let first = matches.next().map(|(i, _)| i);
    let extra = matches.next().is_some();
    match (first, extra) {
        (None, _) => Err(ToolError::model(format!(
            "edit {}: old_text not found in `{path}`; provide more surrounding context",
            index + 1
        ))),
        (Some(_), true) => Err(ToolError::model(format!(
            "edit {}: old_text matches more than once in `{path}`; provide more surrounding context to make it unique",
            index + 1
        ))),
        (Some(start), false) => Ok(start),
    }
}

/// Reject any two edits whose matched ranges overlap or nest.
fn reject_overlaps(
    spans: &[(usize, usize, String)],
    path: &str,
) -> Result<(), ToolError> {
    for pair in spans.windows(2) {
        let (_, prev_end, _) = &pair[0];
        let (next_start, _, _) = &pair[1];
        if next_start < prev_end {
            return Err(ToolError::model(format!(
                "edits overlap in `{path}`; overlapping or nested edits are not allowed"
            )));
        }
    }
    Ok(())
}

/// Translate model-supplied `\n` line endings to `\r\n` when the file uses
/// them, so an `old_text` written with `\n` still matches a CRLF file.
fn translate(text: &str, crlf: bool) -> String {
    if crlf {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// Split a leading UTF-8 BOM off the content, returning `(bom, rest)`.
fn split_bom(content: &str) -> (&str, &str) {
    match content.strip_prefix('\u{feff}') {
        Some(rest) => (&content[..'\u{feff}'.len_utf8()], rest),
        None => ("", content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        crate::tools::testutil::temp_root("edit", tag)
    }

    fn block(old: &str, new: &str) -> EditBlock {
        EditBlock {
            old_text: old.to_string(),
            new_text: new.to_string(),
        }
    }

    #[test]
    fn applies_a_unique_edit() {
        let root = temp_root("unique");
        fs::write(root.join("f.txt"), "hello world\n").unwrap();
        let out = edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("world", "there")],
            },
        )
        .unwrap();
        assert!(out.contains("applied 1 edit"));
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            "hello there\n"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn applies_several_separate_edits_in_one_call() {
        let root = temp_root("multi");
        fs::write(root.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let out = edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("alpha", "ALPHA"), block("gamma", "GAMMA")],
            },
        )
        .unwrap();
        assert!(out.contains("applied 2 edits"));
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            "ALPHA\nbeta\nGAMMA\n"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_an_ambiguous_match() {
        let root = temp_root("ambig");
        fs::write(root.join("f.txt"), "x\nx\n").unwrap();
        let err = edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("x", "y")],
            },
        )
        .unwrap_err();
        assert!(err.model_message.contains("more than once"));
        // The file is untouched on error.
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), "x\nx\n");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_a_missing_match() {
        let root = temp_root("missing");
        fs::write(root.join("f.txt"), "hello\n").unwrap();
        let err = edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("goodbye", "hi")],
            },
        )
        .unwrap_err();
        assert!(err.model_message.contains("not found"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_overlapping_edits() {
        let root = temp_root("overlap");
        fs::write(root.join("f.txt"), "abcdef\n").unwrap();
        let err = edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("abcd", "X"), block("cdef", "Y")],
            },
        )
        .unwrap_err();
        assert!(err.model_message.contains("overlap"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let root = temp_root("crlf");
        fs::write(root.join("f.txt"), "one\r\ntwo\r\n").unwrap();
        edit_in(
            &root,
            EditArgs {
                path: "f.txt".into(),
                edits: vec![block("two", "TWO")],
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            "one\r\nTWO\r\n"
        );
        fs::remove_dir_all(&root).ok();
    }
}
