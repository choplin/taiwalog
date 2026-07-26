# Provider module layout

Provider collectors live under `src/providers/`, with one module per external
provider. Application orchestration depends on this provider boundary rather
than on collector modules spread across the crate root.

This refactor changes source ownership only. CLI commands, JSON response fields,
catalog schema, scan order, transaction boundaries, retention rules, source
immutability, and provider diagnostics remain unchanged. The crate root
re-exports the original collector module paths for compatibility.

Verification covers formatting, all targets, Clippy with warnings denied, a
release build, and a final diff check. No provider registry, shared parser,
plugin interface, or persistence change is introduced.
