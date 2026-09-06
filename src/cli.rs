// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! Command-line surface: the parsed [`Cli`] and the [`Effort`] level that maps
//! onto the SDK's reasoning-effort budget.

use clap::{Parser, ValueEnum};
use tapir::prelude::ThinkingLevel;

/// tapir coding agent — headless, one-shot.
///
/// The prompt is assembled from piped stdin, any `@file` references and the
/// remaining positional words, in that order.
#[derive(Debug, Parser)]
#[command(name = "tp", version, about, long_about = None)]
pub struct Cli {
    /// Model id passed straight to the SDK (e.g. `claude-haiku-4-5`, `gpt-5`).
    #[arg(long, default_value = "claude-haiku-4-5")]
    pub model: String,

    /// Reasoning effort applied to every turn.
    #[arg(long, value_enum, default_value_t = Effort::Off)]
    pub effort: Effort,

    /// System prompt: literal text, or a path to a file to read.
    #[arg(long)]
    pub system_prompt: Option<String>,

    /// Text (or file path) appended to the system prompt. Repeatable.
    #[arg(long)]
    pub append_system_prompt: Vec<String>,

    /// API key for the model's provider. Falls back to the provider env var
    /// (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`) when unset.
    #[arg(long)]
    pub api_key: Option<String>,

    /// Disable the coding tools and run a pure text-only one-shot.
    #[arg(long)]
    pub no_tools: bool,

    /// Suppress tool-activity lines on stderr.
    #[arg(long)]
    pub quiet: bool,

    /// Cap on the number of tool iterations before the run stops.
    #[arg(long, default_value_t = 25)]
    pub max_tool_iterations: usize,

    /// Prompt words and `@file` references. A positional starting with `@` is
    /// inlined as a file; the rest join into the message.
    #[arg(value_name = "PROMPT")]
    pub prompt: Vec<String>,
}

/// Reasoning-effort level, mirroring the SDK's `ThinkingLevel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Effort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// The SDK level to apply, or `None` for `Off` (leave the provider default,
    /// no thinking budget).
    #[must_use]
    pub fn to_thinking(self) -> Option<ThinkingLevel> {
        match self {
            Effort::Off => None,
            Effort::Minimal => Some(ThinkingLevel::Minimal),
            Effort::Low => Some(ThinkingLevel::Low),
            Effort::Medium => Some(ThinkingLevel::Medium),
            Effort::High => Some(ThinkingLevel::High),
            Effort::Xhigh => Some(ThinkingLevel::XHigh),
            Effort::Max => Some(ThinkingLevel::Max),
        }
    }
}
