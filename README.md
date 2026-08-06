# Agentlog

Agentlog is a local browser for session history produced by AI coding agents.
This first durable shell owns a small SQLite catalog and imports Codex, Claude
Code, OpenCode, Gemini CLI, Cursor, and Kimi Code session history. It never
creates, changes, or deletes provider-owned logs.

## Requirements

- macOS or Linux
- Rust 1.85 or newer when building from source

## Install on macOS

Build a native release bundle without Developer ID signing from a source checkout:

```sh
release_archive=$(./scripts/build-macos-release.sh)
release_dir=$(mktemp -d)
trap 'rm -rf "$release_dir"' EXIT HUP INT TERM
tar -xzf "$release_archive" -C "$release_dir"
"$release_dir"/agentlog-*/install.sh "$HOME/.local"
```

Add `$HOME/.local/bin` to `PATH` if it is not already present. Omit the prefix
to install into `/usr/local`; that location may require elevated permissions.
The bundle is built for the active macOS Rust host architecture. It is not
Developer ID signed, notarized, checksummed, or a deterministic bundle.

To build the bundle and verify an isolated installation in one command:

```sh
./scripts/smoke-macos-release.sh
```

## Quickstart

After installation:

```sh
agentlog paths
agentlog sync
agentlog sync --json
agentlog list
agentlog show 1
agentlog purge
agentlog purge --yes
agentlog browse
```

From a source checkout, replace `agentlog` with `cargo run --`.

`browse` first shows the existing Agentlog catalog, then starts a read-only
sync in the background. Its status line names the current provider and reports
processed/candidate source counts without exposing source paths or log content.
Press `r` to request another sync; repeated requests
while a sync is active do not start another one. The list remains available
during sync, keeps its selection by provider/source/session key when rows are
replaced, and reports bounded provider-failure and
refreshed/partial/failed/missing/session counts when sync completes.
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
values make every interaction observable. Run `cargo run -- sync` first, then
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

On a normal exit, Agentlog restores the terminal. It reports an error only if
that restoration fails; on an unexpected terminal error or panic it still makes
a best-effort restoration.

By default, Agentlog owns these paths under the platform XDG data directory,
falling back to `~/.local/share/agentlog` when `XDG_DATA_HOME` is unset or
invalid:

- `config.toml` — optional, minimal local configuration;
- `agentlog.sqlite3` — the Agentlog-owned SQLite catalog.

`agentlog paths` prints these paths and the separate provider-source boundaries.
Use `--home DIR` or `AGENTLOG_HOME` for an isolated Agentlog catalog; these do
not redirect Codex history discovery:

```sh
cargo run -- --home /tmp/agentlog-demo paths
cargo run -- --home /tmp/agentlog-demo sync
```

### Data handling documentation

Agentlog keeps provider-owned inputs separate from its local derived catalog.

| Question | Reference |
| --- | --- |
| What is read, stored, or excluded? | [Data flow](docs/data-architecture.md#data-ownership-and-flow), [stored data](docs/data-architecture.md#stored-catalog-data), [excluded data](docs/data-architecture.md#excluded-data) |
| Does data leave the machine? | [Local-only operation](docs/data-architecture.md#local-only-operation) |
| What does purge remove? | [Purging Agentlog data](docs/data-architecture.md#purging-agentlog-data) |
| Where are provider sources found? | [Source discovery and root precedence](docs/providers.md#source-discovery-and-root-precedence) |
| What happens on failure? | [Failure isolation and last-good data](docs/providers.md#failure-isolation-and-last-good-data) |

`sync` reports provider discovery, candidate counts, and completion to stderr.
In an interactive terminal, its current provider and processed/candidate source
counts update in place; redirected runs keep a bounded per-provider record.
`sync --json` keeps its machine-readable result on stdout.

### Purging the local catalog

`agentlog purge` previews the Agentlog-owned database path, row counts, and
SQLite file-set size. In an eligible terminal it asks for default-No
confirmation; redirected or background use stays preview-only. `--yes` skips
the prompt and removes catalog rows while preserving the schema and files.
See [Purging Agentlog data](docs/data-architecture.md#purging-agentlog-data) for
the exact safety checks, retained data, and rebuild limitation.

## Supported providers

All provider inputs are read-only. An explicit `providers.*_root` value in
`config.toml` has the highest precedence.

| Provider | Imported source | Default root precedence after config |
| --- | --- | --- |
| Codex | Current `sessions/**/*.jsonl` and legacy `history.jsonl` | `CODEX_HOME`, then `~/.codex` |
| Claude Code | Parent and subagent JSONL below `projects/` | `CLAUDE_CONFIG_DIR`, then `~/.claude` |
| OpenCode | `opencode.db` through a bounded private SQLite/WAL snapshot | `XDG_DATA_HOME/opencode`, then the OS-native default (macOS Application Support or Linux `~/.local/share/opencode`) |
| Gemini CLI | Session JSONL below `tmp/` | `$GEMINI_CLI_HOME/.gemini`, then `~/.gemini` |
| Cursor | Transcript JSONL below `projects/` | `~/.cursor` |
| Kimi Code | `sessions/*/*/agents/*/wire.jsonl`, validated against the session index and state | `KIMI_CODE_HOME`, then `~/.kimi-code` |

A discovered source can legitimately produce no session when it contains no
supported visible conversation text. Provider and source failures are isolated:
other providers continue, and a failed or missing source retains its last-good
catalog snapshot with a diagnostic.

See [provider projections](docs/providers.md#provider-projections),
[failure isolation](docs/providers.md#failure-isolation-and-last-good-data),
and [safety bounds](docs/providers.md#safety-bounds) for collector details.
Catalog fields and exclusions are documented under
[Stored catalog data](docs/data-architecture.md#stored-catalog-data) and
[Excluded data](docs/data-architecture.md#excluded-data).
