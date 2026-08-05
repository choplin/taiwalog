# Agentlog MVP scope

## Shipped outcome

Agentlog is a local, single-user macOS CLI and TUI for browsing recent session
history across Codex, Claude Code, OpenCode, Gemini CLI, Cursor, and Kimi Code.
It imports bounded visible conversation data into an Agentlog-owned SQLite
catalog without changing provider-owned files.

The MVP includes:

- full read-only synchronization at startup and through `agentlog sync` or `r`;
- a reverse-chronological browse list and bounded transcript preview;
- provider, repository, CWD, model, execution-kind, and start-date refinement;
- repository or provider grouping;
- source diagnostics, last-good retention, and missing-source reporting;
- preview and explicit purge of Agentlog-owned catalog rows;
- a native macOS release bundle without Developer ID signing, with a
  prefix-based installer.

## Roadmap, not MVP

The following are deliberately excluded from this release:

- AI-generated titles, summaries, or tags;
- ctx integration or full-text search;
- user-authored or generated tag features;
- incremental or continuously running collection;
- multi-machine synchronization or team sharing;
- provider-log archival and configurable retention policies;
- Nix profiles, deterministic bundles, checksums, code signing, or notarization;
- a schema migration framework for older or unknown catalogs.

These are possible follow-up outcomes, not compatibility promises for the MVP.

## Known limitations

- macOS is the only supported operating system. A release archive is native to
  the architecture of the Rust toolchain that built it.
- Release archives are not Developer ID signed or notarized. macOS security
  policy may require the user to approve locally built software.
- Synchronization scans all currently discoverable provider sources. It is
  bounded for safety but has no MVP latency target.
- The catalog is durable derived data, but it can only be rebuilt from provider
  logs that still exist and remain readable.
- Provider formats are external interfaces. Unsupported, malformed, changing,
  or oversized sources are isolated and reported as `error` or `partial` rather
  than guessed through.
- A source with no supported visible user or assistant text can be recognized
  successfully without producing a browsable session.
- Preview is limited to 80 catalog items and 4 KiB per text snippet. Source and
  transcript bounds can intentionally produce a `partial` diagnostic.
- Missing repository, CWD, model, or execution-kind metadata is displayed where
  possible but is not offered as a refinement value.
- The TUI requires at least 40 columns by 10 rows. The passive split preview is
  available at 100 columns or wider; narrower supported terminals use the
  compact list and explicit Preview mode.
- An existing database with an unknown version or a non-Agentlog schema is
  refused without modification; automatic migration is outside the MVP.
