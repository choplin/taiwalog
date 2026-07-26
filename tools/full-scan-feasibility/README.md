# M0 full-scan feasibility harness

This private tool measures one sequential, read-only full pass over the six
MVP provider inputs. It is deliberately outside the root package so M1 can
bootstrap the production application from an empty root.

It reports only aggregate counts, byte sizes, timings, and an in-memory
before/after immutability result. It does not print or write source paths,
session IDs, source hashes, transcript text, prompts, or parser errors.

The explicit format probes are intentionally smaller than production
collectors:

- Codex current requires a session metadata ID; legacy history is partial and
  is reported only as reference counts.
- Claude requires a nonempty session ID.
- Gemini requires a nonempty session ID in its first JSONL record.
- Cursor requires a nonempty transcript-parent directory plus a user/assistant
  role and object message.
- Kimi requires the documented session, agent, and wire-file path shape.
- OpenCode uses one SQLx SQLite connection in read-only mode with creation
  disabled. It does not run write PRAGMAs, checkpoints, or immutable mode.

Candidate sources, accepted sources, catalog sessions, and rejected/partial
sources are separate metrics. Codex legacy session-ID and prompt-record counts
are reference values only and never contribute catalog sessions.

## Run

    cd tools/full-scan-feasibility
    cargo test --all-targets
    ./scripts/run-five-times.sh

Run the script while the six clients are idle. It starts five fresh release
processes and prints sequential scan time, separate hash-audit time, and the
macOS time utility's maximum resident-set size for each run.

For M0, propose sequential full scan only if all are true:

- median scan time is at most 5 seconds;
- worst scan time is at most 10 seconds;
- every run's peak RSS is at most 512 MiB; and
- every provider reports immutable=true.

The decision record must exclude a run in which the source set changed during
measurement. A changed run is evidence for the immutability audit, not a
performance sample.
