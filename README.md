# Agentlog

Agentlog is a local browser for session history produced by AI coding agents.
This first durable shell owns a small SQLite catalog and imports Codex, Claude
Code, OpenCode, Gemini CLI, Cursor, and Kimi Code session history. It never
creates, changes, or deletes provider-owned logs.

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
cargo run -- browse
```

`browse` reads the existing Agentlog catalog without scanning provider sources.
Use `j`/`k` or the arrow keys to choose a recent session, `Enter` for its full
preview, `Esc` to return, and `q` to quit. Press `f` to refine by provider,
repository, CWD, model, execution kind, and start date. Within each metadata
field, `Space` toggles exact values with OR semantics; fields combine with AND.
Blank or missing metadata is not offered as a filter value. Start-date choices
also toggle with OR semantics. `Tab`/`Enter` moves forward, `BackTab` moves
back, `x` clears the current step, and `C` clears every choice and returns to
Recent grouping. Press `?` for the key reference and `!` for retained source
diagnostics. The diagnostics list deliberately shows each retained source's
local locator for source attribution; locators are never emitted by the JSON
catalog interfaces.

### Manual browse review

Run this only with an actual local catalog: at least two provider values, a
repository or CWD value, and several sessions with different `started_at`
values make every interaction observable. Run `cargo run -- scan` first, then
`cargo run -- browse`; if the catalog lacks one of those values, verify the
available layout, help, diagnostics, selection, and terminal restoration steps
below rather than inventing a fixture.

1. At **at least 100x24**, expect a chronological list with Started, Title,
   Repository, and Provider plus a passive preview. At **80x24**, expect the
   narrow list with Started, Title, and Provider. Below **40x10**, expect
   `Terminal too small` and no panic.
2. Press `f`. Toggle two provider values with `Space` (OR), advance with
   `Tab`, select a repository and/or CWD (AND), then select multiple date
   buckets (OR). Advance to Group and choose Provider or Repository. Expect
   fixed recent order inside each first-seen group; missing repositories are
   displayed as `(no repository)`.
3. Press `Esc`: the choices remain active. Press `?` to confirm the key
   reference. Press `!`, use `j`/`k`, `PgUp`/`PgDn`, and `g`/`G` to scroll the
   diagnostics, then `Esc` to close it.
4. In Browse, move selection with `j`/`k`, press `Enter`, scroll the preview,
   and press `Esc`. Expect the same list selection and list context. Finally,
   press `q`; the normal terminal must be restored.

On a normal exit, Agentlog reports a terminal-restoration failure; on an
unexpected terminal error or panic it still makes a best-effort restoration.

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
SQLite never opens the provider-owned files. Gemini CLI discovery uses
`providers.gemini_root`, then `$GEMINI_CLI_HOME/.gemini`, then `~/.gemini`; it
scans JSONL files below `tmp` and uses the native `sessionId` as the session
identity.
Cursor discovery uses `providers.cursor_root`, then `~/.cursor`; it scans JSONL
files below `projects`, including UUID- or otherwise arbitrarily named files,
and uses the containing directory as the native session identity. Kimi Code
discovery uses `providers.kimi_root`, then `KIMI_CODE_HOME`, then
`~/.kimi-code`. It verifies each `sessions/*/*/agents/*/wire.jsonl` source
against the provider session index and session state before importing its
provider-native session ID. The optional config can override these read-only
roots. It cannot configure enrichment, ctx, UI settings, retention, or an
external database:

```toml
[providers]
codex_root = "/absolute/path/to/codex/history"
claude_root = "/absolute/path/to/claude/config"
opencode_root = "/absolute/path/to/opencode/data"
gemini_root = "/absolute/path/to/gemini/config"
cursor_root = "/absolute/path/to/cursor/config"
kimi_root = "/absolute/path/to/kimi-code/config"
```

`scan` creates fresh schema v1 and scans current Codex session JSONL, legacy
`history.jsonl`, Claude Code project/subagent JSONL, the supported OpenCode
SQLite catalog, Gemini CLI session JSONL, Cursor transcript JSONL, and Kimi
Code session-indexed agent wire JSONL. Each source is replaced atomically after
a successful read. A
source-level failure records a diagnostic while retaining that source's
last-good session snapshot; it does not block other sources. Malformed,
unsupported, or oversized sources are failures and preserve the last-good
snapshot. Valid records that exceed catalog bounds are safely truncated and
stored with a `partial` diagnostic. The collectors store only visible
user/assistant text and tool names/statuses; hidden reasoning, tool
inputs/outputs, raw diffs, images, binary attachments, and recognized injected
user/control records are excluded.

Every JSONL collector has a 64 MiB source bound. Gemini, Claude, Cursor, and
Kimi have a 4 MiB record bound; Codex has a 1 MiB record bound. Gemini, Claude,
Cursor, and Kimi bound visible text to 128 KiB per item, 2 MiB per source, and
2,000 catalog items per source. Valid sources exceeding a selection bound are
marked `partial`; the limits are safety boundaries and do not express a
scan-speed target. Claude's 4 MiB record ceiling is the smallest whole-MiB bound
above the observed 3.14 MiB local maximum, while its 64 MiB source limit remains
above the observed 16.9 MiB maximum.

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
