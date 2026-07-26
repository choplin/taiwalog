# Agentlog

Agentlog is a local browser for session history produced by AI coding agents.
This first durable shell owns a small SQLite catalog but has no provider
collectors yet. It never creates, changes, or deletes provider-owned logs.

## Requirements

- macOS
- Rust 1.85 or newer

## Quickstart

Build and run the shell:

```sh
cargo run -- paths
cargo run -- scan
cargo run -- scan --json
```

By default, Agentlog owns these paths under
`~/Library/Application Support/Agentlog`:

- `config.toml` — optional, minimal local configuration;
- `agentlog.sqlite3` — the Agentlog-owned SQLite catalog.

`agentlog paths` prints these paths and separately states that provider-owned
sources are not inspected in this build. Use `--home DIR` or `AGENTLOG_HOME`
for an isolated install or test:

```sh
cargo run -- --home /tmp/agentlog-demo paths
cargo run -- --home /tmp/agentlog-demo scan
```

The optional config reserves explicit provider-root overrides for later
collectors. It cannot configure enrichment, ctx, UI settings, retention, or an
external database:

```toml
[providers]
codex_root = "/absolute/path/to/codex/history"
```

`scan` creates fresh schema v1 and explicitly reports that it refreshed zero
sources because no collector is installed yet. It does not claim that provider
history was imported.

## Schema v1

The fresh catalog contains only `sources`, `sessions`, and `transcript_items`.
A source is unique by provider, source format, and canonical locator; its
latest diagnostic and last-success time live on the same source row. A session
is unique within its source. The schema stores only fields needed for the MVP
catalog filters and bounded visible transcript later. It contains no legacy
compatibility, migration framework, raw archive, tags, AI enrichment, or ctx
search.

An existing database with a version other than v1, or with tables but no
Agentlog version, is refused without modification.
