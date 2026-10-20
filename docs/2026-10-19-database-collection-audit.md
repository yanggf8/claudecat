# 2026-10-19: database collection audit

This inventory is based on the Rust writers and readers, not older architecture notes.

| Data | Writer | Local store | Shared `harnessuse` table | Meaning |
| --- | --- | --- | --- | --- |
| Code index and call graph | `cort index` | one cache DB per local checkout | none | Rebuildable cache tied to a path and tree; not a measurement stream. |
| `cort` commands and hook verdicts | `rust/src/usage.rs` | `cort` `usage.db`, `command_log` | `telemetry_events`, `source=cort_command` | Structured command, outcome, available cost and staleness fields. Schema-v2 rows have per-row machine stamps. |
| ClaudeCat navigation | `src/usage.rs` | ClaudeCat `usage.db`, `navigation_log` | `telemetry_events`, `source=claudecat_navigation` | Hit and route metrics, no raw query. |
| Historical project-memory recalls | `src/usage.rs` | ClaudeCat `usage.db`, `memory_recall_log` | `telemetry_events`, `source=claudecat_memory_recall` | Pre-delegation exposure and source freshness, not answer correctness. Current recalls are cort command rows. |
| Claude Code daily cost | `cort-evals cost-collect` | `cort-evals/claude-cost/*.json` | `telemetry_events`, `source=cost_daily` | Daily descriptive aggregate from surviving local transcripts. |
| Curated project memory | `cort memory record/resolve` | Cortexyoung-owned Turso sync replica | `project_memory` | Reviewed claims with Git source fingerprints, pulled by both harnesses. ClaudeCat forwards memory verbs to cort. |

`claudecat telemetry sync` is the only cloud uploader for structured usage. It reads local
sources without changing the writers, inserts each source row using a stable event ID, and
explicitly pushes and pulls the Rust Turso sync replica. It uses `BEGIN CONCURRENT` for the
upload batch. The telemetry replica and lock are separate from the memory replica and lock,
so a long upload does not hold a local memory recall lock. This is a shared cloud database,
not a shared local SQLite file and not network I/O in an agent hook.

The sync report exposes scanned and inserted totals, counts by source, missing local sources,
legacy `cort` rows, unknown machine rows, unresolved project IDs, and rows without a usable
project alias. A project alias is based on the indexed checkout's Git origin **at upload time**.
It cannot reconstruct an older origin after a remote change. Schema-v1 `cort` rows use their
database-wide machine stamp; if that DB crossed machines, their row origins cannot be recovered.
Schema-v2 `cort` rows and new ClaudeCat rows have per-row stamps. Historical untagged ClaudeCat
navigation rows remain `legacy_unknown`.

This collection is not yet a causal measure of memory help. It does not capture questions on
which memory should have been used but was never queried, whether an answer was correct, or
whether a source was actually read. `memory eval-report` can grade reviewed paired trials,
but no complete paired run has yet been produced. Raw Claude Code and Codex transcripts,
prompts, tool results, and full source files are outside this structured uploader. Cost
backfill sees only transcripts that still exist locally; a zero on an old day can mean the
raw sessions expired. The local index DBs are derived caches and are not uploaded.

2026-10-19, machine `44a8a38f0b05b9a8`: first end-to-end verification of this path.
`claudecat telemetry sync` scanned 13,851 events (13,756 `cort_command`, 90 `cost_daily`,
4 `claudecat_memory_recall`, 1 `claudecat_navigation`) and inserted 246; 13,605 were already
present from uploads earlier the same night. A rerun inserted only the 9 events recorded in
between, so dedup held for every one of the 13,851 previously scanned rows. Remote counts per
source matched the local scan exactly. 13,716 of the 13,756 `cort` rows are schema-v1
(`legacy_unknown`); 40 carry per-row stamps. 28 rows had no project alias, reported as
`unmatched_project_rows` rather than silently dropped. Both user timers
(`cort-cost-collect` 01:30 UTC, `claudecat-telemetry-sync` 01:45 UTC) are enabled on this
machine.

2026-10-20, machine `2eb02d46ec5c4319`: this machine had never synced, because no command
could create its `turso-memory.json` after `memory configure` began forwarding to cort.
`claudecat telemetry configure` (0e6cda6) wrote it from cort's own `harnessuse` credentials.
The first sync inserted all 16,510 scanned rows; a rerun inserted only the 2 recorded in
between. No `cost_daily` source exists here (`cost-collect` has never run on this machine).
72 rows were unmatched, all from 12 project IDs absent from every local cort index. Hashing
existing directories recovered 6: four live repos whose indexes were gone (20 rows; a fresh
`cort index` in each mapped them, leaving 52) and `cortexyoung/rust` / `cortexyoung/evals`
subdirectory runs (4 rows, left unmapped). The other 6 IDs (48 rows) match no existing path.

## Interpretation after collection, 2026-10-20

A read-only `claudecat cort-audit --json` on the same machine reported 4,272 search-hook
probes, 5,223 edit-hook refreshes, seven `impact` calls, and 11 `memory` commands in the
local 30-day usage window. The seven-day window held no `impact` or `context` call. These
totals come from one local cort usage database; running the audit from either repository
shows the same command totals, not per-repository adoption. The recent memory commands
include deployment and manual checks. Search-hook delivery is silent by default. None of
these event counts is an eligible-task denominator or an answer-quality result.

The immediate evaluation gate is paired Claude Code and Codex work on frozen past decisions,
superseded claims, changed sources, and a false pasted report. ClaudeCat can check trial
artifact integrity with `memory eval-report` and measure navigation on separately labeled
long tasks. Cortexyoung owns the memory claim logic and any later change to capture or
recall timing. The full priority and measurement plan is in the sibling cortexyoung
repository's `docs/2026-10-20-direction-after-collection.md`.
