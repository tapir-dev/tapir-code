# tapir-code

tapir coding agent

## Build & test

`just` drives everything; run `just --list` for the full recipe set. Tests run
under `cargo-nextest`, so `cargo test` misses the config — use the recipes.

- `just check` — full local CI gate; run before calling any change done.
- `just test` — fast tests (nextest, all targets/features).
- `just test-doc` — doctests (nextest doesn't run these).
- `cargo nextest run <name>` — a single test.

