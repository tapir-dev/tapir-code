// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! Workspace-root confinement and atomic file writes, shared by the file
//! tools. [`resolve_in_root`] maps a model-supplied path to an absolute path
//! proven to stay inside the root — rejecting a `..` escape or a symlink that
//! points out of it. [`atomic_write`] replaces a file in one `rename`, so an
//! interrupted run never leaves a half-written file.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, io};

use tapir::tool::ToolError;

/// Resolve `requested` against the workspace `root`, confined to it.
///
/// The returned path is lexically normalized (so it carries no `.`/`..`) and
/// proven to stay within `root`, both lexically and after resolving symlinks on
/// its existing prefix. A path that escapes — via `..` or a symlink pointing
/// outside — is a model-visible error. The path itself need not exist, so a
/// `write` to a new file resolves fine.
///
/// # Errors
/// Returns a [`ToolError`] when the root cannot be canonicalized, or when the
/// requested path escapes the root.
pub fn resolve_in_root(
    root: &Path,
    requested: &str,
) -> Result<PathBuf, ToolError> {
    let root = root.canonicalize().map_err(|e| {
        ToolError::model(format!("workspace root is unavailable: {e}"))
            .with_operator(e)
    })?;

    let joined = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        root.join(requested)
    };
    let normalized = normalize_lexically(&joined);

    let escapes = !normalized.starts_with(&root)
        || !resolve_existing_prefix(&normalized).starts_with(&root);
    if escapes {
        return Err(ToolError::model(format!(
            "path `{requested}` escapes the workspace root"
        )));
    }
    Ok(normalized)
}

/// Collapse `.` and `..` components without touching the filesystem. A `..`
/// pops the previous component, so a path that climbs above its base loses the
/// base prefix and fails the later `starts_with` check.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Canonicalize the deepest existing ancestor of `path` and rejoin the missing
/// tail, so a symlink anywhere in the existing prefix is resolved. The tail is
/// already `..`-free (the caller normalized it), so it cannot re-escape.
fn resolve_existing_prefix(path: &Path) -> PathBuf {
    let mut ancestor = path;
    loop {
        if let Ok(real) = ancestor.canonicalize() {
            let tail = path.strip_prefix(ancestor).unwrap_or(Path::new(""));
            return real.join(tail);
        }
        match ancestor.parent() {
            Some(parent) => ancestor = parent,
            None => return path.to_path_buf(),
        }
    }
}

/// Write `bytes` to `path` atomically: stage them in a sibling temp file, fsync
/// nothing, then `rename` over the target in one step. Parent directories are
/// created first.
///
/// # Errors
/// Returns an I/O error if a parent cannot be created or the staged write or
/// rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = temp_sibling(path);
    // Best-effort cleanup of the temp file if the rename never happens.
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A unique temp path beside `path`, on the same filesystem so the rename is
/// atomic.
fn temp_sibling(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map_or_else(
        || "tmp".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let tmp = format!(".{name}.tp-{}-{seq}.tmp", std::process::id());
    path.parent()
        .map_or_else(|| PathBuf::from(&tmp), |parent| parent.join(&tmp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        crate::tools::testutil::temp_root("ws", tag)
    }

    #[test]
    fn resolves_a_plain_relative_path() {
        let root = temp_root("plain");
        let path = resolve_in_root(&root, "src/main.rs").unwrap();
        assert_eq!(path, root.join("src/main.rs"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_a_parent_dir_escape() {
        let root = temp_root("escape");
        let err = resolve_in_root(&root, "../secret").unwrap_err();
        assert!(err.model_message.contains("escapes the workspace root"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_an_absolute_path_outside_root() {
        let root = temp_root("abs");
        let err = resolve_in_root(&root, "/etc/passwd").unwrap_err();
        assert!(err.model_message.contains("escapes the workspace root"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_a_symlink_pointing_out_of_root() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-target");
        fs::write(outside.join("secret.txt"), "top secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        let err = resolve_in_root(&root, "link/secret.txt").unwrap_err();
        assert!(err.model_message.contains("escapes the workspace root"));

        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let root = temp_root("atomic");
        let target = root.join("a/b/c.txt");
        atomic_write(&target, b"hello").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello");
        fs::remove_dir_all(&root).ok();
    }
}
