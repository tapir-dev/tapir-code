# tapir-code

The `tp` coding-agent CLI: a headless front-end over the tapir SDK. This
glossary fixes the terms the CLI exposes to its users.

## Language

**Effort**:
The reasoning-effort level requested for a run, from `off` through `max`. The
user-facing name for the model's thinking budget.
_Avoid_: thinking, reasoning level, thinking budget

**File reference**:
A positional prompt argument prefixed with `@`, naming a file whose contents
are inlined into the prompt.
_Avoid_: attachment, mention, include

**One-shot run**:
A single non-interactive invocation: one assembled prompt in, the reply
streamed out, then exit. The only run shape the CLI offers.
_Avoid_: batch, print mode, headless mode

**Prompt assembly**:
Combining piped stdin, inlined file references and the message words into the
single prompt sent to the agent, in that fixed order.
_Avoid_: prompt building, input merging

## Tools

**Tool**:
A named capability the agent can invoke during a run. The CLI ships four:
`read`, `edit`, `write` and `bash`.
_Avoid_: function, action, command

**Mutating tool**:
A tool that changes the filesystem or runs a process (`edit`, `write`, `bash`),
run serialized against other tools. A **read-only tool** (`read`) has no side
effects and may run in parallel.
_Avoid_: write tool, unsafe tool

**Workspace root**:
The directory the file tools are confined to — the process working directory.
`read`, `edit` and `write` reject paths that escape it; `bash` is not confined.
_Avoid_: sandbox, jail, project root

**Tool activity**:
The per-call progress lines the CLI writes to stderr while tools run (tool name,
argument summary, error flag). Suppressed with `--quiet`.
_Avoid_: tool log, trace, tool output

**Tool iteration**:
One turn of the model-tool-model loop. A run stops at the first tool-free reply,
capped by `--max-tool-iterations`.
_Avoid_: round, step, turn
