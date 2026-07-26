# Agentlog

Agentlog is a local browser for session history produced by AI coding agents.
This first durable shell owns a small SQLite catalog and imports Codex session
history. It never creates, changes, or deletes provider-owned logs.

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

`agentlog paths` prints these paths and the read-only Codex source boundary.
Use `--home DIR` or `AGENTLOG_HOME` for an isolated Agentlog catalog; these do
not redirect Codex history discovery:

```sh
cargo run -- --home /tmp/agentlog-demo paths
cargo run -- --home /tmp/agentlog-demo scan
```

Codex source discovery uses `providers.codex_root`, then `CODEX_HOME`, then
`~/.codex`. The optional config can override that read-only root. It cannot
configure enrichment, ctx, UI settings, retention, or an external database:

```toml
[providers]
codex_root = "/absolute/path/to/codex/history"
```

`scan` creates fresh schema v1 and scans current Codex session JSONL plus
legacy `history.jsonl`. Each source is replaced atomically after a successful
read. A source-level failure records a diagnostic while retaining that source's
last-good session snapshot; it does not block other files. Malformed or
oversized records are failures and preserve the last-good snapshot. Valid
records that exceed catalog bounds are safely truncated and stored with a
`partial` diagnostic. The collector stores only visible
user/assistant text and tool names/statuses—never hidden reasoning,
instructions, tool inputs/outputs, raw diffs, or images.

Visible text is bounded to 128 KiB per item and 2 MiB for one selected current
session or one legacy source snapshot. These limits keep full scans bounded
without retaining raw provider logs.

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
