# Agentlog MVP acceptance

Status: release candidate evidence is complete; final human acceptance is
pending.

This record separates repeatable automated evidence from the actual-terminal
review that must be performed by a person. Final acceptance must not be inferred
from passing automation alone.

## Automated release checks

Run from a clean source checkout on macOS:

```sh
cargo fmt --all -- --check
TMPDIR=/private/tmp cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --release --locked
./scripts/smoke-macos-release.sh
git diff --check
```

Verification on 2026-08-05:

- format: passed;
- tests: 124 library tests, 2 process integration tests, and doc tests passed;
- Clippy with warnings denied: passed;
- native release build: passed;
- isolated native bundle install, `--version`, `--help`, and `paths --json`:
  passed for `agentlog-0.1.0-aarch64-apple-darwin.tar.gz`;
- fresh target build and Mach-O architecture match: passed for `arm64` /
  `aarch64-apple-darwin`;
- empty and relative install prefixes, a non-runnable payload, and an existing
  symlink installation were rejected without replacing a valid install;
- shell syntax check for all release scripts: passed;
- Nix flake evaluation: passed;
- diff check: passed.

The process test `one_sync_accepts_one_representative_source_from_each_provider`
is the reproducible fallback acceptance path: it exercises one representative
source fixture for every supported provider in one catalog synchronization and
verifies that the fixture inputs remain unchanged.

## Six-provider source evidence

An isolated catalog was synchronized from the locally available provider roots
on 2026-08-05 with `agentlog --home <temporary-directory> sync --json`. The
summary JSON and a provider-grouped read of the isolated SQLite catalog produced
the counts below. No provider-wide discovery failed. This table is supplementary
environment evidence; provider immutability and the six-provider parser path are
enforced by the reproducible fixture test above.

| Provider | Candidate sources | Refreshed | Partial | Failed | Visible sessions |
| --- | ---: | ---: | ---: | ---: | ---: |
| Codex | 1,350 | 1,349 | 2 | 1 | 1,663 |
| Claude Code | 1,882 | 1,881 | 0 | 1 | 1,861 |
| OpenCode | 1 | 1 | 0 | 0 | 10 |
| Gemini CLI | 1 | 1 | 0 | 0 | 0 |
| Cursor | 23 | 23 | 0 | 0 | 23 |
| Kimi Code | 7 | 7 | 0 | 0 | 5 |

Totals: 6 collectors, 3,264 candidate sources, 3,262 refreshed sources, 2
partial sources, 2 failed sources, 0 missing sources, and 3,562 visible
sessions. Gemini CLI source discovery and parsing succeeded but the available
source contained no supported visible session; its browseable-session behavior
is therefore covered by the approved representative fixture rather than claimed
from this local source.

## Human actual-terminal checklist

Use an isolated Agentlog home so purge review cannot affect the normal catalog:

```sh
review_home=/private/tmp/agentlog-mvp-review
agentlog --home "$review_home" sync
agentlog --home "$review_home" browse
```

Confirm each item and record any failure before approving the MVP:

- [ ] The six providers appear in sync completion or source diagnostics. For
  each provider with a visible session, filter to it, select a session, and
  inspect Preview. Accept the representative fixture evidence for a provider
  only when the actual source contains no supported visible session.
- [ ] At 100x24 or wider, Browse shows Started, Title, Repository, Provider, and
  a passive preview. Selection and preview remain stable during refresh.
- [ ] At 80x24, Browse uses the compact Started, Title, and Provider list;
  `Enter` opens Preview and `Esc` restores the same list context.
- [ ] Below 40x10, `Terminal too small` is shown without a panic; resizing back
  restores the normal view.
- [ ] Refine supports OR within one field and AND across fields, preserves
  choices on `Esc`, groups by Provider or Repository, and reports an empty
  result distinctly.
- [ ] `r` starts one refresh, repeated requests do not start another, existing
  sessions remain usable, and completion counts are bounded and visible.
- [ ] `!` shows retained source diagnostics; list and preview navigation work,
  and `Esc` returns without changing the selected session.
- [ ] `agentlog --home "$review_home" purge` previews only the isolated
  Agentlog-owned catalog and makes no change without confirmation.
- [ ] `agentlog --home "$review_home" purge --yes` empties catalog rows while
  preserving the database and schema; a subsequent `sync` rebuilds available
  provider data.
- [ ] `q` and `Ctrl-C` restore the normal terminal, including alternate screen
  and cursor state, from wide, narrow, Preview, Refine, and Diagnostics views.

Final acceptance requires an explicit human approval after this checklist. The
release candidate must remain pending if any item is unverified or rejected.
