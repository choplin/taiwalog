# Provider scan boundaries

Provider-native models and Agentlog scanning adapters live under
`src/providers/`, with one module per external provider.

Each provider model owns knowledge that remains true without Agentlog:

- provider root precedence and validation;
- native source directories and files;
- source discovery and provider-owned representations.

Each provider scanner applies Agentlog policy to that model:

- selection of visible transcript content;
- bounds, partial diagnostics, and source-local failures;
- projection into normalized source snapshots.

`Catalog` invokes the common `ProviderScanner` capability and consumes one
`ProviderScan` incrementally. It owns snapshot persistence, diagnostic writes,
aggregate reporting, scanner-identity validation, and the provider-wide
transaction. Catalog code does not branch on individual providers, and scanners
do not receive a database connection or catalog operation.

The provider registry in `providers::installed` is composition code. Adding a
provider changes that registry and the new provider module, but does not change
Catalog scan coordination.

The original `collect_*` functions and crate-root module re-exports remain as
compatibility entry points. They construct the corresponding provider model and
scanner, then delegate to the same Catalog scan operation.

This refactor deliberately preserves CLI commands, JSON response fields,
catalog schema, provider order, provider-wide transaction boundaries, retention
rules, source immutability, and diagnostic behavior. It does not introduce a
storage port or plugin loading mechanism; the concrete Catalog continues to own
the shared `SQLite` pool.

Verification covers formatting, all targets, the full test suite, Clippy with
warnings denied, a release build, and a final diff check. A scanner-identity
mismatch is now rejected before persistence as an explicit Catalog invariant.
