// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! Prompt assembly: splitting positionals into `@file` references and message
//! words, inlining file contents, resolving text-or-path system prompts, and
//! joining piped stdin, files and message into the final prompt.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Positional args split into `@file` references and the message words.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Positionals {
    /// Paths from `@`-prefixed positionals, with the `@` stripped.
    pub files: Vec<String>,
    /// The remaining positionals joined by a single space.
    pub message: String,
}

/// Split positional args: `@`-prefixed ones are file references, the rest join
/// into the message.
#[must_use]
pub fn split_positionals(args: &[String]) -> Positionals {
    let mut files = Vec::new();
    let mut words = Vec::new();
    for arg in args {
        match arg.strip_prefix('@') {
            Some(path) => files.push(path.to_string()),
            None => words.push(arg.as_str()),
        }
    }
    Positionals {
        files,
        message: words.join(" "),
    }
}

/// Strip a leading UTF-8 BOM if present.
fn strip_bom(s: String) -> String {
    s.strip_prefix('\u{feff}').map_or(s.clone(), str::to_string)
}

/// Expand a leading `~/` to the home directory; other paths pass through.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Resolve a prompt input that is either literal text or a path to a file. An
/// existing file is read and returned; anything else is returned verbatim.
///
/// # Errors
/// Returns an error only when the value names an existing file that cannot be
/// read.
pub fn resolve_prompt_input(value: &str) -> io::Result<String> {
    let path = expand_tilde(value);
    if path.is_file() {
        Ok(strip_bom(fs::read_to_string(path)?))
    } else {
        Ok(value.to_string())
    }
}

/// Build the system prompt from an optional base (text or path) and zero or
/// more appends (each text or path), joined by blank lines. `None` when nothing
/// is supplied.
///
/// # Errors
/// Propagates a read error from any input that names an existing file.
pub fn build_system_prompt(
    base: Option<&str>,
    appends: &[String],
) -> io::Result<Option<String>> {
    let mut parts = Vec::new();
    if let Some(base) = base {
        parts.push(resolve_prompt_input(base)?);
    }
    for append in appends {
        parts.push(resolve_prompt_input(append)?);
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("\n\n")))
    }
}

/// Inline `@file` references as `<file name="path">…</file>` blocks. Empty
/// (whitespace-only) files are skipped.
///
/// # Errors
/// Returns an error if any referenced file cannot be read.
pub fn inline_files(paths: &[String]) -> io::Result<String> {
    let mut out = String::new();
    for path in paths {
        let content = fs::read_to_string(expand_tilde(path)).map_err(|e| {
            io::Error::new(e.kind(), format!("cannot read @{path}: {e}"))
        })?;
        let content = strip_bom(content);
        if content.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("<file name=\"{path}\">\n{content}\n</file>\n"));
    }
    Ok(out)
}

/// Join piped stdin, the inlined file block and the message into the final
/// prompt, in that order, separated by blank lines. Empty segments are dropped.
#[must_use]
pub fn assemble_prompt(
    stdin: Option<&str>,
    files_block: &str,
    message: &str,
) -> String {
    let mut parts = Vec::new();
    if let Some(stdin) = stdin {
        let stdin = stdin.trim();
        if !stdin.is_empty() {
            parts.push(stdin);
        }
    }
    let files_block = files_block.trim();
    if !files_block.is_empty() {
        parts.push(files_block);
    }
    if !message.is_empty() {
        parts.push(message);
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_files_from_message_words() {
        let args = [
            "@main.rs".to_string(),
            "where".to_string(),
            "is".to_string(),
            "@lib.rs".to_string(),
            "the bug?".to_string(),
        ];
        let split = split_positionals(&args);
        assert_eq!(split.files, ["main.rs", "lib.rs"]);
        assert_eq!(split.message, "where is the bug?");
    }

    #[test]
    fn split_with_no_files_is_just_the_message() {
        let args = ["explain".to_string(), "this".to_string()];
        let split = split_positionals(&args);
        assert!(split.files.is_empty());
        assert_eq!(split.message, "explain this");
    }

    #[test]
    fn resolve_returns_literal_text_for_non_path() {
        let out = resolve_prompt_input("you are helpful").unwrap();
        assert_eq!(out, "you are helpful");
    }

    #[test]
    fn resolve_reads_an_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "tp-resolve-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sys.txt");
        fs::write(&file, "\u{feff}from file").unwrap();
        let out = resolve_prompt_input(file.to_str().unwrap()).unwrap();
        assert_eq!(out, "from file");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_system_prompt_joins_base_and_appends() {
        let out = build_system_prompt(
            Some("base"),
            &["one".to_string(), "two".to_string()],
        )
        .unwrap();
        assert_eq!(out.as_deref(), Some("base\n\none\n\ntwo"));
    }

    #[test]
    fn build_system_prompt_is_none_when_empty() {
        assert_eq!(build_system_prompt(None, &[]).unwrap(), None);
    }

    #[test]
    fn inline_files_wraps_contents_and_skips_empty() {
        let dir = std::env::temp_dir().join(format!(
            "tp-inline-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir_all(&dir).unwrap();
        let full = dir.join("a.txt");
        let empty = dir.join("b.txt");
        fs::write(&full, "hello").unwrap();
        fs::write(&empty, "   \n").unwrap();
        let paths = [
            full.to_str().unwrap().to_string(),
            empty.to_str().unwrap().to_string(),
        ];
        let out = inline_files(&paths).unwrap();
        assert_eq!(
            out,
            format!("<file name=\"{}\">\nhello\n</file>\n", full.display())
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inline_files_errors_on_missing() {
        let err = inline_files(&["/no/such/file/xyz".to_string()]).unwrap_err();
        assert!(err.to_string().contains("cannot read @/no/such/file/xyz"));
    }

    #[test]
    fn assemble_orders_stdin_then_files_then_message() {
        let out = assemble_prompt(
            Some("piped\n"),
            "<file name=\"a\">\nx\n</file>\n",
            "the question",
        );
        assert_eq!(
            out,
            "piped\n\n<file name=\"a\">\nx\n</file>\n\nthe question"
        );
    }

    #[test]
    fn assemble_drops_empty_segments() {
        assert_eq!(assemble_prompt(None, "", "only message"), "only message");
        assert_eq!(assemble_prompt(Some("  "), "", ""), "");
    }
}
