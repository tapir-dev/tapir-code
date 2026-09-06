// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The one-shot driver: assemble the prompt, build the agent (with the coding
//! tools by default), run it, and split the output — assistant text to stdout,
//! tool activity to stderr.

use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use tapir::prelude::*;
use tapir::tapir_provider::http::ReqwestClient;
use tapir::tapir_provider::{
    ModelEntry, ModelRegistry, Registry, StreamEvent, create_provider,
};

use crate::cli::Cli;
use crate::prompt::{
    assemble_prompt, build_system_prompt, coding_system_prompt, inline_files,
    split_positionals,
};
use crate::tools::coding_tools;

/// Run one prompt to completion against the provider resolved from `--model`,
/// writing assistant text to stdout and tool activity to stderr.
///
/// # Errors
/// Returns an error when no prompt is supplied, a credential is missing, a
/// referenced file cannot be read, or the agent run fails.
pub async fn run(cli: Cli) -> Result<()> {
    let provider = resolve_provider(&cli.model, cli.api_key.as_deref())?;
    let stdout = io::stdout();
    let stderr = io::stderr();
    drive(&cli, provider, &mut stdout.lock(), &mut stderr.lock()).await
}

/// The provider-injectable core of [`run`]: build the agent from `cli` over the
/// supplied `provider` and render its events to `out` (assistant text) and
/// `err` (tool activity). A scripted provider drives this in tests.
///
/// # Errors
/// Returns an error when no prompt is supplied, a referenced file cannot be
/// read, the agent cannot be built, or the run fails.
pub async fn drive<O: Write, E: Write>(
    cli: &Cli,
    provider: Arc<dyn Provider>,
    out: &mut O,
    err: &mut E,
) -> Result<()> {
    let prompt = assemble(cli)?;
    if prompt.trim().is_empty() {
        bail!("no prompt provided");
    }

    let tools_active = !cli.no_tools;
    let system = build_system(cli, tools_active)?;

    let mut builder = Agent::builder()
        .provider(provider)
        .max_tool_iterations(cli.max_tool_iterations);
    if tools_active {
        builder = builder.tools(coding_tools());
    }
    if let Some(system) = system {
        builder = builder.system(system);
    }
    if let Some(level) = cli.effort.to_thinking() {
        builder = builder.thinking(level);
    }
    let agent = builder
        .build()
        .map_err(|e| anyhow!("building agent: {e}"))?;

    render_run(agent.prompt(prompt), out, err, cli.quiet).await
}

/// Resolve the system prompt: the user's `--system-prompt` replaces the
/// default; otherwise, when tools are active, the coding-agent prompt is used.
/// `--append-system-prompt` inputs are appended to whichever base applies.
fn build_system(cli: &Cli, tools_active: bool) -> Result<Option<String>> {
    let mut base = cli.system_prompt.clone();
    if tools_active && base.is_none() {
        let workspace = std::env::current_dir()
            .context("resolving the working directory")?;
        base = Some(coding_system_prompt(&workspace.display().to_string()));
    }
    build_system_prompt(base.as_deref(), &cli.append_system_prompt)
        .context("reading system prompt")
}

/// Assemble the prompt from piped stdin, `@file` inlines and the message.
fn assemble(cli: &Cli) -> Result<String> {
    let stdin = read_piped_stdin()?;
    let split = split_positionals(&cli.prompt);
    let files_block =
        inline_files(&split.files).context("inlining @file references")?;
    Ok(assemble_prompt(
        stdin.as_deref(),
        &files_block,
        &split.message,
    ))
}

/// Read all of stdin when it is piped; `None` when attached to a terminal or
/// empty.
fn read_piped_stdin() -> Result<Option<String>> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(None);
    }
    let mut buf = String::new();
    stdin.read_to_string(&mut buf).context("reading stdin")?;
    if buf.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(buf))
    }
}

/// Resolve `model` to a concrete provider carrying its credential.
///
/// The id is looked up in the SDK's model catalog, not guessed from its prefix,
/// so provider selection follows the registry. When `--api-key` is supplied it
/// is passed to the provider explicitly; otherwise the catalog's env-resolved
/// key is used. Either way the provider is constructed directly, so no
/// environment mutation happens.
///
/// # Errors
/// Returns an error when the catalog fails to load, the id is unknown, no
/// credential is available, or the provider cannot be constructed.
fn resolve_provider(
    model: &str,
    api_key: Option<&str>,
) -> Result<Arc<dyn Provider>> {
    let entry = resolve_entry(model, api_key)?;
    create_provider(&entry, ReqwestClient::new())
        .map_err(|e| anyhow!("building provider for model {model}: {e}"))
}

/// Look up `model` in the catalog and attach the credential to use.
///
/// `--api-key`, when given, overrides the catalog's env-resolved key. An
/// unknown id is a clear error; a model with no key from either source reports
/// which provider env var to set.
fn resolve_entry(model: &str, api_key: Option<&str>) -> Result<ModelEntry> {
    let registry = ModelRegistry::load(None, None)
        .map_err(|e| anyhow!("loading model catalog: {e}"))?;
    let mut entry = registry
        .find_by_id(model)
        .ok_or_else(|| anyhow!("unknown model id `{model}`"))?
        .clone();
    if let Some(key) = api_key {
        entry.api_key = Some(key.to_owned());
    }
    if entry.api_key.is_none() {
        let provider = entry.model.provider.as_str();
        let env_var = Registry::resolve(provider)
            .map_or("its API-key environment variable", |info| {
                info.api_key_env
            });
        bail!(
            "no credential for model {model} (provider {provider}); pass --api-key or set {env_var}"
        );
    }
    Ok(entry)
}

/// Consume the run as an event stream: assistant text deltas go to `out`, and
/// one tool-activity line per completed call goes to `err` (unless `quiet`).
async fn render_run<O: Write, E: Write>(
    mut run: Run,
    out: &mut O,
    err: &mut E,
    quiet: bool,
) -> Result<()> {
    // The argument summary for each pending call, keyed by call id and captured
    // from the assistant message before the call runs.
    let mut summaries: HashMap<String, String> = HashMap::new();

    while let Some(event) = run.next().await {
        match event {
            AgentEvent::MessageUpdate {
                delta: StreamEvent::TextDelta { text, .. },
                ..
            } => {
                out.write_all(text.as_bytes())?;
                out.flush()?;
            }
            AgentEvent::MessageEnd { message, .. } if !quiet => {
                capture_summaries(&message, &mut summaries);
            }
            AgentEvent::ToolExecutionEnd { result, .. } if !quiet => {
                let summary = summaries
                    .get(&result.tool_call_id)
                    .map_or("", String::as_str);
                write_activity(
                    err,
                    &result.tool_name,
                    summary,
                    result.is_error,
                )?;
            }
            AgentEvent::Error { error, .. } => return Err(anyhow!("{error}")),
            _ => {}
        }
    }
    writeln!(out)?;
    Ok(())
}

/// Record an argument summary for every tool call in `message`, so its
/// activity line can name what the tool ran on.
fn capture_summaries(
    message: &AssistantMessage,
    summaries: &mut HashMap<String, String>,
) {
    for part in &message.content {
        if let ContentPart::ToolCall { id, arguments, .. } = part {
            summaries.insert(id.clone(), summarize(arguments));
        }
    }
}

/// A short argument summary for a tool call: the `path` for a file tool, or the
/// first line of the `command` for `bash`, truncated.
fn summarize(arguments: &serde_json::Value) -> String {
    if let Some(path) = arguments.get("path").and_then(|v| v.as_str()) {
        return path.to_string();
    }
    if let Some(command) = arguments.get("command").and_then(|v| v.as_str()) {
        let first = command.lines().next().unwrap_or_default();
        return truncate_summary(first);
    }
    String::new()
}

/// Clip a summary to a single terminal-friendly line.
fn truncate_summary(text: &str) -> String {
    const MAX: usize = 60;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let clipped: String = text.chars().take(MAX).collect();
    format!("{clipped}…")
}

/// Write one tool-activity line: the tool name, its argument summary, and an
/// error flag when the call failed.
fn write_activity<E: Write>(
    err: &mut E,
    name: &str,
    summary: &str,
    is_error: bool,
) -> io::Result<()> {
    let status = if is_error { " [error]" } else { "" };
    if summary.is_empty() {
        writeln!(err, "{name}{status}")
    } else {
        writeln!(err, "{name}({summary}){status}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An explicit key is passed so the check is credential-independent: the
    // provider is picked from the catalog, never the env.
    #[test]
    fn resolve_entry_selects_anthropic_for_a_claude_id() {
        let entry = resolve_entry("claude-haiku-4-5", Some("sk-test")).unwrap();
        assert_eq!(entry.model.provider.as_str(), "anthropic");
        assert_eq!(entry.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn resolve_entry_selects_openai_for_a_gpt_id() {
        let entry = resolve_entry("gpt-5", Some("sk-test")).unwrap();
        assert_eq!(entry.model.provider.as_str(), "openai");
        assert_eq!(entry.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn resolve_entry_rejects_an_unknown_id() {
        let err =
            resolve_entry("no-such-model-xyz", Some("sk-test")).unwrap_err();
        assert!(err.to_string().contains("unknown model id"));
    }
}
