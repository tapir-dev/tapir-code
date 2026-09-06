// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The `bash` tool: run a shell command from the workspace root in its own
//! process group, with a default timeout and tail-truncated combined output. On
//! timeout or cancellation the whole process group is killed, so no orphaned
//! child is left behind.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;
use tapir::tool::{ToolCtx, ToolError};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Default seconds a command may run before it is killed.
const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// Tail-truncation ceilings for the combined output.
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 256 * 1024;

/// Arguments to the `bash` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BashArgs {
    /// The shell command, run with `sh -c` from the workspace root.
    pub command: String,
    /// Seconds before the command is killed. Defaults to 120.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// Run `args.command` with the working directory set to `root`.
///
/// Combines stdout and stderr, tail-truncates, and returns the output. A
/// non-zero exit, a timeout, or cancellation is a [`ToolError`] carrying the
/// captured output.
///
/// # Errors
/// Returns a [`ToolError`] when the command cannot be spawned, exits non-zero,
/// times out, or is cancelled.
pub async fn bash_in(
    root: &Path,
    args: BashArgs,
    ctx: &ToolCtx,
) -> Result<String, ToolError> {
    let timeout_secs = args.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS);

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&args.command)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|e| {
        ToolError::model(format!("cannot start command: {e}")).with_operator(e)
    })?;
    // `process_group(0)` set the child's group id to its own pid.
    let pgid = child.id().expect("spawned child has a pid") as i32;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    // Drain both pipes concurrently so a chatty command never deadlocks on a
    // full pipe while we wait for it to exit.
    let out_task = tokio::spawn(read_all(stdout));
    let err_task = tokio::spawn(read_all(stderr));

    let outcome = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => Outcome::Exited(status.code()),
            Err(e) => {
                return Err(ToolError::model(format!(
                    "command could not be waited on: {e}"
                ))
                .with_operator(e));
            }
        },
        () = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            kill_group(pgid);
            let _ = child.wait().await;
            Outcome::TimedOut
        }
        () = ctx.cancelled() => {
            kill_group(pgid);
            let _ = child.wait().await;
            Outcome::Cancelled
        }
    };

    let out = out_task.await.unwrap_or_default();
    let err = err_task.await.unwrap_or_default();
    let body = truncate_tail(&combine(&out, &err));

    match outcome {
        Outcome::Exited(Some(0)) => Ok(or_no_output_note(body)),
        Outcome::Exited(code) => Err(ToolError::model(format!(
            "command exited with status {}\n{body}",
            code.map_or_else(|| "signal".to_string(), |c| c.to_string()),
        ))),
        Outcome::TimedOut => Err(ToolError::model(format!(
            "command timed out after {timeout_secs}s and its process group was killed\n{body}"
        ))),
        Outcome::Cancelled => {
            Err(ToolError::model(format!("command was cancelled\n{body}")))
        }
    }
}

/// How the command run ended.
enum Outcome {
    /// Exited with this status code (`None` if killed by a signal).
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

/// SIGKILL the whole process group `pgid`, tearing down the command's child
/// tree in one call.
fn kill_group(pgid: i32) {
    // SAFETY: `killpg` on our own spawned child's process group id; SIGKILL
    // takes no argument and the call has no memory effects.
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

/// Read a pipe to EOF, discarding a read error (the output so far is still
/// returned).
async fn read_all<R: AsyncRead + Unpin>(mut reader: R) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf).await;
    buf
}

/// Combine captured stdout then stderr into one lossy-UTF-8 string.
fn combine(out: &[u8], err: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(out).into_owned();
    let err = String::from_utf8_lossy(err);
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    text
}

/// Keep the tail of `text` within the line and byte ceilings, prepending a
/// notice when anything is dropped — so the error at the end of a noisy command
/// survives.
fn truncate_tail(text: &str) -> String {
    let mut truncated = false;
    let lines: Vec<&str> = text.lines().collect();
    let kept = if lines.len() > MAX_LINES {
        truncated = true;
        &lines[lines.len() - MAX_LINES..]
    } else {
        &lines[..]
    };
    let mut body = kept.join("\n");
    if text.ends_with('\n') && !body.is_empty() {
        body.push('\n');
    }

    if body.len() > MAX_BYTES {
        truncated = true;
        let mut cut = body.len() - MAX_BYTES;
        while cut < body.len() && !body.is_char_boundary(cut) {
            cut += 1;
        }
        body = body[cut..].to_string();
    }

    if truncated {
        format!("[truncated: showing the tail of the output]\n{body}")
    } else {
        body
    }
}

/// Substitute a note for empty output, so a successful command still returns
/// something legible.
fn or_no_output_note(body: String) -> String {
    if body.is_empty() {
        "(command produced no output)".to_string()
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        crate::tools::testutil::temp_root("bash", tag)
    }

    fn args(command: &str, timeout_seconds: Option<u64>) -> BashArgs {
        BashArgs {
            command: command.to_string(),
            timeout_seconds,
        }
    }

    #[tokio::test]
    async fn runs_a_command_and_returns_output() {
        let root = temp_root("ok");
        let ctx = ToolCtx::new("call");
        let out = bash_in(&root, args("echo hello", None), &ctx)
            .await
            .unwrap();
        assert_eq!(out, "hello\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn runs_from_the_workspace_root() {
        let root = temp_root("cwd");
        std::fs::write(root.join("marker.txt"), "x").unwrap();
        let ctx = ToolCtx::new("call");
        let out = bash_in(&root, args("ls", None), &ctx).await.unwrap();
        assert!(out.contains("marker.txt"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_nonzero_exit_is_an_error_carrying_output() {
        let root = temp_root("fail");
        let ctx = ToolCtx::new("call");
        let err = bash_in(&root, args("echo oops >&2; exit 3", None), &ctx)
            .await
            .unwrap_err();
        assert!(err.model_message.contains("status 3"));
        assert!(err.model_message.contains("oops"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_timeout_kills_the_process_tree() {
        let root = temp_root("timeout");
        let ctx = ToolCtx::new("call");
        // A child that would create `marker` after the timeout fires.
        let err =
            bash_in(&root, args("sleep 3 && touch marker", Some(1)), &ctx)
                .await
                .unwrap_err();
        assert!(err.model_message.contains("timed out"));

        // Give the killed grandchild well past its sleep; the marker must never
        // appear, proving the whole group was torn down.
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(
            !root.join("marker").exists(),
            "the process tree survived the timeout"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn oversized_output_is_tail_truncated_with_a_notice() {
        let root = temp_root("truncate");
        let ctx = ToolCtx::new("call");
        // Emit well over MAX_LINES lines; the tail must survive under a notice
        // and the head must be dropped. `awk` (POSIX) keeps the test portable
        // where `seq` is absent.
        let out = bash_in(
            &root,
            args("awk 'BEGIN{for(i=1;i<=3000;i++)print i}'", None),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            out.starts_with("[truncated: showing the tail of the output]\n"),
            "missing truncation notice: {out:.80?}"
        );
        assert!(out.contains("\n3000\n"), "the tail line was dropped");
        assert!(!out.contains("\n1\n"), "the head line survived truncation");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn output_over_the_byte_ceiling_is_tail_truncated() {
        let root = temp_root("bytes");
        let ctx = ToolCtx::new("call");
        // Few lines (under MAX_LINES) but well over MAX_BYTES, so only the byte
        // ceiling trips. Each line is ~2 KB; 300 of them clear 256 KB.
        let out = bash_in(
            &root,
            args(
                "awk 'BEGIN{for(i=1;i<=300;i++){s=\"\";for(j=0;j<2000;j++)s=s\"x\";print i\" \"s}}'",
                None,
            ),
            &ctx,
        )
        .await
        .unwrap();
        assert!(
            out.starts_with("[truncated: showing the tail of the output]\n"),
            "missing truncation notice: {out:.80?}"
        );
        assert!(out.len() <= MAX_BYTES + 64, "body exceeds the byte ceiling");
        assert!(out.contains("300 "), "the tail line was dropped");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn stdout_and_stderr_are_combined_on_success() {
        let root = temp_root("combine");
        let ctx = ToolCtx::new("call");
        let out = bash_in(&root, args("echo out; echo err >&2", None), &ctx)
            .await
            .unwrap();
        assert!(out.contains("out"), "stdout missing: {out:?}");
        assert!(out.contains("err"), "stderr missing: {out:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_explicit_timeout_overrides_the_default() {
        let root = temp_root("override");
        let ctx = ToolCtx::new("call");
        // 2s of sleep under a 5s cap completes normally.
        let out = bash_in(&root, args("sleep 2 && echo done", Some(5)), &ctx)
            .await
            .unwrap();
        assert!(out.contains("done"));
        std::fs::remove_dir_all(&root).ok();
    }
}
