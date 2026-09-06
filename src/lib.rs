// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The `tp` coding-agent CLI. This first slice is a headless, one-shot driver
//! over the `tapir` SDK: assemble a prompt (positional words, `@file` inlines
//! and piped stdin), pick a model and reasoning effort, and stream the reply.

pub mod cli;
pub mod prompt;
pub mod run;

pub use cli::Cli;
pub use run::run;
