# 2026-10-19: database collection audit

This inventory is based on the Rust writers and readers, not older architecture notes.

| Data | Writer | Local store | Shared `harnessuse` table | Meaning |
| --- | --- | --- | --- | --- |
| Code index and call graph | `cort index` | one cache DB per local checkout | none | Rebuildable cache tied to a path and tree; not a measurement stream. |
| `cort` commands and hook verdicts | `rust/src/usage.rs` | `cort` `usage.db`, `command_log` | `telemetry_events`, `source=cort_command` | Structured command, outcome, available cost and staleness fields. Schema-v2 rows have per-row machine stamps. |
| ClaudeCat navigation | `src/usage.rs` | ClaudeCat `usage.db`, `navigation_log` | `telemetry_events`, `source=claudecat_navigation` | Hit and route metrics, no raw query. |
| Project-memory recalls | `src/usage.rs` | ClaudeCat `usage.db`, `memory_recall_log` | `telemetry_events`, `source=claudecat_memory_recall` | Exposure and source freshness, not answer correctness. |
| Claude Code daily cost | `cort-evals cost-collect` | `cort-evals/claude-cost/*.json` | `telemetry_events`, `source=cost_daily` | Daily descriptive aggregate from surviving local transcripts. |
| Curated project memory | `claudecat memory record/resolve` | Turso sync replica | `project_memory` | Reviewed claims with Git source fingerprints, pulled by both harnesses. |

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
