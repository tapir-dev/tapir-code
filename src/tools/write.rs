// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The `write` tool: create or overwrite a file atomically, making any missing
//! parent directories along the way.

use std::path::Path;

use schemars::JsonSchema;
use serde::Deserialize;
use tapir::tool::ToolError;

use crate::tools::workspace::{atomic_write, resolve_in_root};

/// Arguments to the `write` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteArgs {
    /// Path to the file, relative to the workspace root. Parent directories are
    /// created as needed.
    pub path: String,
    /// The full contents to write.
    pub content: String,
}

/// Write `args.content` to `args.path`, confined to `root`.
///
/// # Errors
/// Returns a [`ToolError`] when the path escapes the root or the write fails.
pub fn write_in(root: &Path, args: WriteArgs) -> Result<String, ToolError> {
    let path = resolve_in_root(root, &args.path)?;
    atomic_write(&path, args.content.as_bytes()).map_err(|e| {
        ToolError::model(format!("cannot write `{}`: {e}", args.path))
            .with_operator(e)
    })?;
    Ok(format!(
        "wrote {} bytes to `{}`",
        args.content.len(),
        args.path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        crate::tools::testutil::temp_root("write", tag)
    }

    #[test]
    fn creates_a_new_file() {
        let root = temp_root("new");
        write_in(
            &root,
            WriteArgs {
                path: "hello.txt".into(),
                content: "hi".into(),
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("hello.txt")).unwrap(), "hi");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn creates_missing_parent_directories() {
        let root = temp_root("nested");
        write_in(
            &root,
            WriteArgs {
                path: "a/b/c/deep.txt".into(),
                content: "deep".into(),
            },
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("a/b/c/deep.txt")).unwrap(),
            "deep"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn overwrites_an_existing_file() {
        let root = temp_root("over");
        fs::write(root.join("f.txt"), "old").unwrap();
        write_in(
            &root,
            WriteArgs {
                path: "f.txt".into(),
                content: "new".into(),
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), "new");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_a_path_outside_root() {
        let root = temp_root("escape");
        let err = write_in(
            &root,
            WriteArgs {
                path: "../evil.txt".into(),
                content: "x".into(),
            },
        )
        .unwrap_err();
        assert!(err.model_message.contains("escapes the workspace root"));
        fs::remove_dir_all(&root).ok();
    }
}
