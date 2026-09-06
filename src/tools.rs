// SPDX-License-Identifier: ISC
// SPDX-FileCopyrightText: 2026 Murilo Ijanc' <murilo@ijanc.org>

//! The four coding tools — `read`, `edit`, `write`, `bash` — authored on the
//! tapir SDK's `#[tool]` attribute. Each `#[tool]` wrapper is a thin shell that
//! resolves the workspace root (the process working directory) and delegates to
//! a root-taking core function in the submodule of the same name; the core
//! functions hold all the logic and are unit-tested directly against a
//! temp-directory workspace.
//!
//! The `///` on each `#[tool]` wrapper is the model-facing tool description
//! (the macro reads it into the schema), so those wrappers carry usage guidance
//! rather than the usual `# Errors` section.

pub mod bash;
pub mod edit;
pub mod read;
pub mod workspace;
pub mod write;

use std::path::PathBuf;
use std::sync::Arc;

use tapir::prelude::tool;
use tapir::tool::{ErasedTool, ToolError};

use bash::{BashArgs, bash_in};
use edit::{EditArgs, edit_in};
use read::{ReadArgs, read_in};
use write::{WriteArgs, write_in};

/// The workspace root: the process working directory the file tools are
/// confined to.
fn cwd() -> Result<PathBuf, ToolError> {
    std::env::current_dir().map_err(|e| {
        ToolError::model(format!("cannot determine the working directory: {e}"))
            .with_operator(e)
    })
}

// The wrapper fns are named `*_tool` so the generated tool struct does not
// collide with the submodule of the same name; `name = "..."` keeps the
// model-facing tool name short.

#[tool(read_only, name = "read")]
/// Read a slice of a UTF-8 text file from the workspace. Optional 1-indexed
/// `offset` and line `limit` point at a slice of a large file; output is
/// head-truncated with a notice when it is too long.
pub async fn read_tool(args: ReadArgs) -> Result<String, ToolError> {
    read_in(&cwd()?, args)
}

#[tool(name = "edit")]
/// Edit a file in place by exact, unique text replacement. Each `old_text` must
/// appear exactly once; supply several blocks to change several spots at once.
/// Read the file first so the text matches exactly.
pub async fn edit_tool(args: EditArgs) -> Result<String, ToolError> {
    edit_in(&cwd()?, args)
}

#[tool(name = "write")]
/// Create or overwrite a file with the given contents, making any missing
/// parent directories. Prefer `edit` for changing existing files.
pub async fn write_tool(args: WriteArgs) -> Result<String, ToolError> {
    write_in(&cwd()?, args)
}

#[tool(name = "bash")]
/// Run a shell command with `sh -c` from the workspace root, returning its
/// combined output. Use it to build, test, and search (`rg`, `grep`, `find`).
/// Defaults to a 120s timeout; set `timeout_seconds` for a longer command.
pub async fn bash_tool(
    args: BashArgs,
    ctx: &ToolCtx,
) -> Result<String, ToolError> {
    bash_in(&cwd()?, args, ctx).await
}

/// The four coding tools, erased for bulk registration on the agent builder.
#[must_use]
pub fn coding_tools() -> Vec<Arc<dyn ErasedTool>> {
    vec![
        Arc::new(read_tool),
        Arc::new(edit_tool),
        Arc::new(write_tool),
        Arc::new(bash_tool),
    ]
}

/// Shared test helpers for the tool submodules.
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    /// A throwaway workspace directory for a test, canonicalized so a
    /// confinement check compares like paths. `prefix` names the tool and `tag`
    /// distinguishes tests within it; the process id keeps it unique under the
    /// process-per-test runner.
    pub(crate) fn temp_root(prefix: &str, tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("tp-{prefix}-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapir::tool::Concurrency;

    #[test]
    fn registers_four_tools_with_the_right_classes() {
        let tools = coding_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["read", "edit", "write", "bash"]);

        // `read` is parallel-safe; the mutating tools are exclusive.
        assert_eq!(tools[0].concurrency(), Concurrency::Safe);
        for mutating in &tools[1..] {
            assert_eq!(mutating.concurrency(), Concurrency::Exclusive);
        }
    }
}
