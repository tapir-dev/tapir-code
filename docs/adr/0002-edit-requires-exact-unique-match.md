# The `edit` tool requires an exact, unique text match

`edit` replaces text by matching each `old_text` block exactly once against the
original file contents. Zero matches or more than one match is an error that
asks the model for more surrounding context. Multiple blocks are matched against
the original file, not incrementally after earlier blocks apply.

## Considered options

- **Fuzzy / similarity matching** (whitespace- and Unicode-normalized, or
  levenshtein-based SEARCH/REPLACE): rejected because it can silently apply an
  edit to the wrong location, which is worse than a clear failure the model can
  recover from.

## Consequences

Edits are predictable and auditable. If exact matching proves too brittle in
practice (models struggling to reproduce whitespace), a normalized-match
fallback can be revisited under a new ADR.
