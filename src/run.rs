// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The one-shot driver: assemble the prompt, build the agent, and stream the
//! reply's text to stdout.

use std::io::{self, IsTerminal, Read, Write};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use tapir::prelude::*;
use tapir::tapir_provider::StreamEvent;

use crate::cli::Cli;
use crate::prompt::{
    assemble_prompt, build_system_prompt, inline_files, split_positionals,
};

/// Run one prompt to completion, streaming the assistant's text to stdout.
///
/// # Errors
/// Returns an error when no prompt is supplied, a credential is missing, a
/// referenced file cannot be read, or the agent run fails.
pub async fn run(cli: Cli) -> Result<()> {
    let (provider, env_var) = provider_for(&cli.model);
    apply_api_key(cli.api_key.as_deref(), env_var);
    if cli.api_key.is_none() && std::env::var_os(env_var).is_none() {
        bail!(
            "{env_var} unset (model {}, provider {provider}); pass --api-key or export it",
            cli.model
        );
    }

    let prompt = assemble(&cli)?;
    if prompt.trim().is_empty() {
        bail!("no prompt provided");
    }
    let system = build_system_prompt(
        cli.system_prompt.as_deref(),
        &cli.append_system_prompt,
    )
    .context("reading system prompt")?;

    let mut builder = Agent::builder().model(&cli.model);
    if let Some(system) = system {
        builder = builder.system(system);
    }
    if let Some(level) = cli.effort.to_thinking() {
        builder = builder.thinking(level);
    }
    let agent = builder
        .build()
        .map_err(|e| anyhow!("building agent: {e}"))?;

    stream_reply(agent.prompt(prompt)).await
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

/// The provider name and credential env var for a model id, inferred from its
/// prefix (`claude*` is Anthropic, everything else OpenAI).
fn provider_for(model: &str) -> (&'static str, &'static str) {
    if model.starts_with("claude") {
        ("anthropic", "ANTHROPIC_API_KEY")
    } else {
        ("openai", "OPENAI_API_KEY")
    }
}

/// Set the provider credential env var from `--api-key` so the SDK's offline
/// `model` path picks it up. The SDK reads the key from the environment at
/// build time; this is done before the agent is built.
fn apply_api_key(api_key: Option<&str>, env_var: &str) {
    if let Some(key) = api_key {
        // SAFETY: called at startup before any agent turn spawns provider
        // work, so no other thread is reading the environment concurrently.
        unsafe { std::env::set_var(env_var, key) };
    }
}

/// Consume the run as an event stream, writing each text delta to stdout.
async fn stream_reply(mut run: Run) -> Result<()> {
    let mut stdout = io::stdout();
    while let Some(event) = run.next().await {
        match event {
            AgentEvent::MessageUpdate {
                delta: StreamEvent::TextDelta { text, .. },
                ..
            } => {
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
            }
            AgentEvent::Error { error, .. } => return Err(anyhow!("{error}")),
            _ => {}
        }
    }
    writeln!(stdout)?;
    Ok(())
}
