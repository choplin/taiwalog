# Shell module boundaries

Status: accepted.

Agentlog's shell keeps each change driver at an explicit boundary:

- `main` is the executable entry point and delegates startup immediately;
- `cli` owns command-line parsing, human-readable output, and JSON formatting;
- `app` coordinates scan, list, show, and path-report workflows;
- `storage` owns the Agentlog SQLite catalog, mappings, and transactions;
- each provider module owns its native source representation and test fixtures.

The OpenCode fixture schema therefore lives with the OpenCode adapter rather
than in the application workflow module. The fixture remains test-only.

This refactor preserves CLI commands, flags, defaults, help text, output text,
JSON shapes, provider order, schema v1, transaction boundaries, and provider
source immutability. It deliberately adds no storage port, plugin mechanism,
schema migration, or user-visible feature.

Verification covers formatting, all targets, the full test suite, Clippy with
warnings denied, a release build, CLI help and representative command output,
and a final diff review.
