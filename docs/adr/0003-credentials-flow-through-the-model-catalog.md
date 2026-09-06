# Credentials reach the provider through the model catalog, not the environment

`--api-key` (and the fall-back provider env var) reaches the model's provider by
constructing the provider explicitly. `tp` resolves the model id against the
SDK's `ModelRegistry`, sets the key on the resolved catalog entry, and builds the
provider with `create_provider(entry, ReqwestClient::new())`, passing it to
`Agent::builder().provider(...)`. Provider selection follows the catalog entry's
provider id; an unknown id fails before any credential work.

Both credential sources take the same path. When `--api-key` is absent, the key
the catalog resolved from the provider's env var is used as-is; when it is
present, it overrides that entry key. The CLI never mutates the process
environment.

## Considered options

- **Env-var shim** (the first slice): infer the provider from the model-id
  prefix and `std::env::set_var` the key before the offline `.model(id)` path
  reads it. Rejected: `set_var` is `unsafe` in edition 2024 (process-global, not
  thread-safe), and the `claude*`/else prefix split misroutes any other
  provider, aliased id, or future catalog entry.
- **Split paths**: keep `.model(id)` for the env case and construct a provider
  only for an explicit `--api-key`. Rejected: two resolution paths and two
  places for the missing-credential error, for no gain — `.model(id)` already
  resolves through the same `ModelRegistry` and `create_provider` internally, so
  routing both cases through it directly is one path, not a new dependency.

## Consequences

Provider selection is catalog-driven and prefix guessing is gone: a model id the
catalog does not hold is a clear `unknown model id` error rather than a key sent
to the wrong provider. The credential path holds no `unsafe` and no env
mutation. `tp` now always loads the catalog at startup (the `.model(id)`
convenience did this internally anyway), and a new provider is reachable the
moment it is in the catalog, with no CLI change.
