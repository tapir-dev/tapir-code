// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! End-to-end tests for the one-shot run driver, driven by a local scripted
//! [`Provider`] (no network). They assert the split between assistant text on
//! stdout and tool activity on stderr, and the behavior of `--no-tools`,
//! `--quiet`, and `--max-tool-iterations`. A test per coding tool (`read`,
//! `write`, `edit`, `bash`) drives a scripted call against a temp workspace and
//! asserts its filesystem effect.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use tapir::tapir_provider::{
    AssistantMessage, CompletionOptions, Context, Error as ProviderError,
    FinishReason, Provider, StreamAccumulator, StreamEvent, StreamEvents,
    Usage,
};
use tapir_code::cli::{Cli, Effort};
use tapir_code::run::drive;

/// What a scripted completion saw: how many tools it was offered.
#[derive(Debug, Clone)]
struct Seen {
    tools_offered: usize,
}

/// A `Provider` that replays one scripted reply per completion, clamping at the
/// last script, and records what each call was offered.
struct ScriptedProvider {
    scripts: Vec<Vec<StreamEvent>>,
    cursor: AtomicUsize,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            scripts,
            cursor: AtomicUsize::new(0),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen(&self) -> Arc<Mutex<Vec<Seen>>> {
        self.seen.clone()
    }

    fn record(&self, ctx: &Context) -> Vec<StreamEvent> {
        self.seen.lock().unwrap().push(Seen {
            tools_offered: ctx.tools.len(),
        });
        let i = self.cursor.fetch_add(1, Ordering::SeqCst);
        self.scripts[i.min(self.scripts.len() - 1)].clone()
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    async fn complete(
        &self,
        ctx: &Context,
        _opts: &CompletionOptions,
    ) -> Result<AssistantMessage, ProviderError> {
        Ok(StreamAccumulator::fold(&self.record(ctx)))
    }

    async fn complete_stream(
        &self,
        ctx: &Context,
        _opts: &CompletionOptions,
    ) -> Result<StreamEvents, ProviderError> {
        let events = self.record(ctx);
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok::<StreamEvent, ProviderError>),
        )))
    }
}

/// A scripted reply that streams `text` and stops with no tool calls.
fn text_script(text: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart,
        StreamEvent::TextStart { index: 0 },
        StreamEvent::TextDelta {
            index: 0,
            text: text.to_string(),
        },
        StreamEvent::TextEnd { index: 0 },
        StreamEvent::Done {
            finish_reason: FinishReason::Stop,
            usage: Usage::default(),
        },
    ]
}

/// A scripted reply requesting a batch of tool calls, in order.
fn tool_calls_script(
    calls: &[(&str, &str, serde_json::Value)],
) -> Vec<StreamEvent> {
    let mut events = vec![StreamEvent::MessageStart];
    for (index, (id, name, args)) in calls.iter().enumerate() {
        events.push(StreamEvent::ToolCallStart {
            index,
            id: (*id).to_string(),
            name: (*name).to_string(),
        });
        events.push(StreamEvent::ToolCallDelta {
            index,
            partial_json: args.to_string(),
        });
        events.push(StreamEvent::ToolCallEnd { index });
    }
    events.push(StreamEvent::Done {
        finish_reason: FinishReason::ToolUse,
        usage: Usage::default(),
    });
    events
}

/// A `Cli` for the driver, bypassing arg parsing. `model`/`api_key` are unused
/// because the provider is injected.
fn cli(prompt: &str, no_tools: bool, quiet: bool, max: usize) -> Cli {
    Cli {
        model: "scripted".into(),
        effort: Effort::Off,
        system_prompt: None,
        append_system_prompt: Vec::new(),
        api_key: None,
        no_tools,
        quiet,
        max_tool_iterations: max,
        prompt: vec![prompt.into()],
    }
}

/// Make a throwaway directory the process's current directory, so the file
/// tools — rooted at the working directory — operate inside it. Nextest runs
/// each test in its own process, so changing the cwd here disturbs no other
/// test; the process id keeps the directory unique.
fn enter_temp_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("tp-run-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    std::env::set_current_dir(&dir).unwrap();
    dir
}

#[tokio::test]
async fn assistant_text_to_stdout_and_tool_activity_to_stderr() {
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[("call_1", "bash", json!({"command": "echo hi"}))]),
        text_script("all done"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("do it", false, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "all done\n");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("bash(echo hi)"), "stderr was: {err:?}");
}

#[tokio::test]
async fn no_tools_offers_no_tools_and_prints_only_text() {
    let provider = ScriptedProvider::new(vec![text_script("just text")]);
    let seen = provider.seen();
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("hi", true, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "just text\n");
    assert!(String::from_utf8(err).unwrap().is_empty());
    assert_eq!(seen.lock().unwrap()[0].tools_offered, 0);
}

#[tokio::test]
async fn quiet_suppresses_tool_activity() {
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[("call_1", "bash", json!({"command": "echo hi"}))]),
        text_script("ok"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("go", false, true, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "ok\n");
    assert!(String::from_utf8(err).unwrap().is_empty());
}

#[tokio::test]
async fn max_tool_iterations_stops_a_looping_run() {
    // The provider always asks for another tool call, so only the cap ends it.
    let provider = ScriptedProvider::new(vec![tool_calls_script(&[(
        "call_1",
        "bash",
        json!({"command": "echo again"}),
    )])]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = drive(
        &cli("loop", false, true, 2),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await;

    assert!(result.is_err(), "the iteration cap should end the run");
}

#[tokio::test]
async fn a_read_call_reads_a_file_from_the_workspace() {
    let root = enter_temp_workspace("read");
    std::fs::write(root.join("notes.txt"), "first line\nsecond line\n")
        .unwrap();
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[("call_1", "read", json!({"path": "notes.txt"}))]),
        text_script("read it"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("show notes", false, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "read it\n");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("read(notes.txt)"), "stderr was: {err:?}");
    assert!(!err.contains("[error]"), "read should not error: {err:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_write_call_creates_a_file_in_the_workspace() {
    let root = enter_temp_workspace("write");
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[(
            "call_1",
            "write",
            json!({"path": "src/new.rs", "content": "fn main() {}\n"}),
        )]),
        text_script("wrote it"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("create the file", false, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "wrote it\n");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("write(src/new.rs)"), "stderr was: {err:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("src/new.rs")).unwrap(),
        "fn main() {}\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn a_bash_call_runs_a_command_in_the_workspace() {
    let root = enter_temp_workspace("bash");
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[(
            "call_1",
            "bash",
            json!({"command": "echo built > out.txt"}),
        )]),
        text_script("ran it"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("build it", false, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "ran it\n");
    let err = String::from_utf8(err).unwrap();
    assert!(
        err.contains("bash(echo built > out.txt)"),
        "stderr was: {err:?}"
    );
    assert!(!err.contains("[error]"), "bash should not error: {err:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("out.txt")).unwrap(),
        "built\n"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn an_edit_call_replaces_text_in_a_file() {
    let root = enter_temp_workspace("edit");
    std::fs::write(root.join("lib.rs"), "let x = 1;\nlet y = 2;\n").unwrap();
    let provider = ScriptedProvider::new(vec![
        tool_calls_script(&[(
            "call_1",
            "edit",
            json!({
                "path": "lib.rs",
                "edits": [{"old_text": "let x = 1;", "new_text": "let x = 42;"}],
            }),
        )]),
        text_script("edited it"),
    ]);
    let mut out = Vec::new();
    let mut err = Vec::new();
    drive(
        &cli("change x", false, false, 25),
        Arc::new(provider),
        &mut out,
        &mut err,
    )
    .await
    .unwrap();

    assert_eq!(String::from_utf8(out).unwrap(), "edited it\n");
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("edit(lib.rs)"), "stderr was: {err:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("lib.rs")).unwrap(),
        "let x = 42;\nlet y = 2;\n"
    );
    std::fs::remove_dir_all(&root).ok();
}
