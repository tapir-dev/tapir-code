// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The one-shot driver: assemble the prompt, build the agent, and stream the
//! reply's text to stdout.

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
    assemble_prompt, build_system_prompt, inline_files, split_positionals,
};

/// Run one prompt to completion, streaming the assistant's text to stdout.
///
/// # Errors
/// Returns an error when no prompt is supplied, a credential is missing, a
/// referenced file cannot be read, or the agent run fails.
pub async fn run(cli: Cli) -> Result<()> {
    let provider = resolve_provider(&cli.model, cli.api_key.as_deref())?;

    let prompt = assemble(&cli)?;
    if prompt.trim().is_empty() {
        bail!("no prompt provided");
    }
    let system = build_system_prompt(
        cli.system_prompt.as_deref(),
        &cli.append_system_prompt,
    )
    .context("reading system prompt")?;

    let mut builder = Agent::builder().provider(provider);
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
