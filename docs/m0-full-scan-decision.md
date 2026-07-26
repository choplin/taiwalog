# M0 sequential full-scan feasibility record

Status: accepted. Scan-time measurements below are feasibility evidence and
diagnostic baselines, not MVP acceptance targets.

## Method

The private tool at tools/full-scan-feasibility performs one provider at a
time, in the fixed order Codex, Claude, OpenCode, Gemini, Cursor, and Kimi.
It reads only provider-owned inputs and emits only aggregate numbers.

Before and after each provider pass it re-discovers candidates and compares
the source set and in-memory SHA-256 audit. The audit includes file content,
size, modification time, mode, inode, relevant directory metadata, and source
presence. OpenCode additionally covers database, WAL, and SHM inputs. Kimi
additionally covers session index, session state, and wire inputs.

Hash-audit time is reported separately and is not included in scan time.
OpenCode is opened through SQLx with read-only access and disabled creation.
The tool does not issue checkpoint, immutable-mode, or write PRAGMAs.

This M0 harness isolates malformed or incompatible discovered sources, but a
provider-root enumeration permission error aborts that measurement run. Such a
run is invalid for the five-run performance record and must be repeated after
the root is readable.

## Stable provider detail run

One stable read-only observation gave the following aggregate values. All six
provider audits reported unchanged sources before and after their scan.

| Provider | Candidates | Accepted | Catalog sessions | Partial/rejected | Largest candidate bytes | Source unchanged |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Codex | 896 | 895 | 894 | 1 | 13,821,779 | yes |
| Claude | 2,919 | 2,555 | 2,555 | 364 | 16,855,559 | yes |
| OpenCode | 1 | 1 | 9 | 0 | 7,073,464 | yes |
| Gemini | 1 | 1 | 0 | 0 | 2,474 | yes |
| Cursor | 3 | 3 | 3 | 0 | 18,307 | yes |
| Kimi | 7 | 7 | 5 | 0 | 1,509,302 | yes |

Codex legacy remained out of catalog totals. Its reference-only counts in this
observation were 226 distinct session IDs and 1,824 prompt records.

The pass measured 4,697 ms of sequential scan work and 11,983 ms of separate
hash-audit work.

Gemini's native source passed the format probe but contained no visible user
or assistant conversation, so it is accepted without creating a browseable
catalog session. Kimi accepted all seven valid composite sources; only five
contained the visible wire conversation required for catalog inclusion.

Catalog identities are scoped to their provider source as well as their
native ID, so repeated native IDs in distinct sources remain distinct
sessions.

## Stable five fresh-process observations

The harness release binary was launched five times through macOS
time-with-limits. All five samples in this batch reported immutable sources.
Peak RSS below is maximum resident-set size in bytes.

| Run | Scan ms | Peak RSS bytes | Stable source set |
| --- | ---: | ---: | --- |
| 1 | 4,913 | 16,498,688 | yes |
| 2 | 4,815 | 16,138,240 | yes |
| 3 | 4,617 | 15,581,184 | yes |
| 4 | 4,655 | 15,876,096 | yes |
| 5 | 4,786 | 15,499,264 | yes |

The stable median scan time is 4,786 ms, stable worst scan time is 4,913 ms,
and stable median peak RSS is 15,876,096 bytes (about 15.1 MiB). The largest
observed peak RSS was 16,498,688 bytes (about 15.7 MiB). These values are
retained as reference measurements for detecting regressions. Scan time is not
a release gate; the 512 MiB peak-memory ceiling remains a resource-safety
guard.

## Initial unstable observations

An earlier five-run batch occurred while orchestration activity was still
writing provider logs. Those samples detected concurrent source changes and
were excluded from the decision statistics. They were not interpreted as
scanner writes; the fixture immutability test separately proves that the
harness itself does not mutate provider sources.

## Recommendation

Adopt sequential full scan for MVP startup and manual refresh, with no
checkpoint framework, parallel provider lanes, or generic incremental layer.
The MVP has no scan-latency target. Timing measurements remain useful
diagnostics, but do not determine milestone acceptance while the product goal
is cross-agent session browsing.

Provider-owned sources remaining immutable and peak memory remaining below
512 MiB are still required correctness and resource-safety properties.

Gemini's fixture coverage proves the supported visible-conversation format,
but the stable real-data source produced zero browseable sessions. Reconfirm
real-data Gemini coverage at the M2 gate before treating it as proven end-user
coverage.

Revisit source-level change detection or parallel provider lanes only when
observed user experience makes that work valuable, not merely because a
diagnostic measurement changes.
