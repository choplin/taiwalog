# Agentlog

Agentlog is a local browser for session history produced by AI coding agents.
This first durable shell owns a small SQLite catalog and imports Codex, Claude
Code, and OpenCode session history. It never creates, changes, or deletes
provider-owned logs.

## Requirements

- macOS
- Rust 1.85 or newer

## Quickstart

Build and run the shell:

```sh
cargo run -- paths
cargo run -- scan
cargo run -- scan --json
cargo run -- list
cargo run -- show 1
```

By default, Agentlog owns these paths under
`~/Library/Application Support/Agentlog`:

- `config.toml` — optional, minimal local configuration;
- `agentlog.sqlite3` — the Agentlog-owned SQLite catalog.

`agentlog paths` prints these paths and the separate provider-source boundaries.
Use `--home DIR` or `AGENTLOG_HOME` for an isolated Agentlog catalog; these do
not redirect Codex history discovery:

```sh
cargo run -- --home /tmp/agentlog-demo paths
cargo run -- --home /tmp/agentlog-demo scan
```

Codex source discovery uses `providers.codex_root`, then `CODEX_HOME`, then
`~/.codex`. Claude Code source discovery uses `providers.claude_root`, then
`CLAUDE_CONFIG_DIR`, then `~/.claude`; project transcripts are read below its
`projects` directory, including direct subagent JSONL files. OpenCode source
discovery uses `providers.opencode_root`, then `XDG_DATA_HOME/opencode`, then
`~/.local/share/opencode` and the macOS Application Support fallback. OpenCode
is read through a bounded private snapshot of its SQLite database and WAL, so
SQLite never opens the provider-owned files. The optional config can override
these read-only roots. It cannot configure enrichment, ctx, UI settings,
retention, or an external database:

```toml
[providers]
codex_root = "/absolute/path/to/codex/history"
claude_root = "/absolute/path/to/claude/config"
opencode_root = "/absolute/path/to/opencode/data"
```

`scan` creates fresh schema v1 and scans current Codex session JSONL, legacy
`history.jsonl`, Claude Code project/subagent JSONL, and the supported OpenCode
SQLite catalog. Each source is replaced atomically after a successful read. A
source-level failure records a diagnostic while retaining that source's
last-good session snapshot; it does not block other sources. Malformed,
unsupported, or oversized sources are failures and preserve the last-good
snapshot. Valid records that exceed catalog bounds are safely truncated and
stored with a `partial` diagnostic. The collectors store only visible
user/assistant text and tool names/statuses; hidden reasoning, tool
inputs/outputs, raw diffs, images, binary attachments, and recognized injected
user/control records are excluded.

Visible text is bounded to 128 KiB per item and 2 MiB for one selected current
session, one legacy source snapshot, or one Claude source snapshot. Claude
records are structurally limited to 4 MiB: this is the smallest whole-MiB bound
above the observed 3.14 MiB local maximum, while the 64 MiB source limit remains
above the observed 16.9 MiB maximum. These limits keep full scans bounded
without retaining raw provider logs.

OpenCode snapshots are bounded to 128 MiB for the provider database, WAL, and
SHM file set, 64 MiB of joined transcript JSON, and 100,000 joined rows. Its
session metadata and visible items use the same 512-byte identity, 128 KiB
item, 2 MiB session-text, and 2,000-item limits as the normalized catalog.

`list` and `show SESSION_ID` read only the already-cataloged SQLite data; they
never rescan provider files. List and preview responses show each source's
diagnostic status, so an `error` status makes a retained last-good session
explicitly stale. `show` emits at most 80 items and 4 KiB per text snippet.

The rebuildable catalog uses SQLite WAL with `synchronous=NORMAL`. A sudden
power loss can lose recent catalog updates, but does not corrupt the catalog or
affect provider-owned logs; a later `scan` rebuilds it.

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
