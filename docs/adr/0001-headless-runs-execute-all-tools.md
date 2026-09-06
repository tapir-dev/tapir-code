# Headless runs execute all tool calls without an approval gate

`tp` is a non-interactive one-shot CLI, so no human is present to approve tool
calls mid-run. We run every tool the model requests, with no `before_tool_call`
gate, trusting the caller who launched `tp`. The only guardrail is confining the
file tools (`read`/`edit`/`write`) to the workspace root; `bash` runs unconfined
because a shell escapes any jail trivially.

## Considered options

- **Read-only by default, mutations behind a flag** (`--allow-writes`): rejected
  as friction for the primary use case, where `tp` is driven as trusted
  automation and is expected to edit and run commands.
- **Graduated modes** (ask/write/yolo): the `ask` mode is impossible headless,
  leaving only always-allow, which is what we do.

## Consequences

A prompt such as "clean up the project" can delete or overwrite files inside the
workspace root. Callers must treat `tp` as trusted automation and scope its
working directory accordingly.
