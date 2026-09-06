# tapir-code

tapir coding agent

## Build & test

`just` drives everything; run `just --list` for the full recipe set. Tests run
under `cargo-nextest`, so `cargo test` misses the config — use the recipes.

- `just check` — full local CI gate; run before calling any change done.
- `just test` — fast tests (nextest, all targets/features).
- `just test-doc` — doctests (nextest doesn't run these).
- `cargo nextest run <name>` — a single test.

## Agent skills

### Issue tracker

Issues live in GitHub Issues (`tapir-dev/tapir-code`), via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context (`CONTEXT.md` + `docs/adr/` at the repo root). See `docs/agents/domain.md`.

