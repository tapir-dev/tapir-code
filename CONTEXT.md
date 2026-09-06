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
