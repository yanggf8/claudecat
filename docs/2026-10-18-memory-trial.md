# 2026-10-18: cross-agent project memory trial

## Why this exists

On this machine, a no-tool headless probe found a prior deployment correction in Claude Code's
auto-memory while Codex answered that it did not know. The same project files and instruction
files do not imply the two agents receive the same past decisions. The baseline notes are in
`cortexyoung/docs/2026-10-18-cross-agent-memory-baseline.md` in the sibling repository. This is
one local observation, not a measured recall rate.

Hindsight's coding-agent integration suggested three useful mechanisms: retain a specific fact,
retrieve it when needed, and reconcile later corrections. Its implementation and service are not
used here. This trial is a pure Rust local CLI with a Git-reviewable ledger.

## Storage update on 2026-10-18

The first trial used a Git-tracked JSON ledger. The current CLI uses the Rust `turso` sync SDK
with a TursoDB cloud database and a local sync replica. The JSON file is now an import source
only (`memory import-legacy`). The record/source/supersession contract below remains, but references
to Git synchronization describe the original trial. A recall discloses `sync_status: synced` only
when both push and pull succeeded; otherwise `offline_or_pending` makes its local-only basis clear.
The local usage DB and paired eval report remain separate. A TursoDB organization must enable the
new engine; an existing libSQL database is not a substitute. No cross-machine outcome measurement
has yet been claimed from the storage change.

The `yanggf8` organization enabled the Turso engine later on 2026-10-18. A `harnessuse` database
was created with `--tursodb` and `turso db show` reported `Type: Turso`. The first live sync
exposed a bootstrap bug: it created `project_memory` locally after the push, so the remote still
had only Turso's internal tables. The sync command now creates the table before its push. This
observation prompted the independent-replica check below.
After the fix, a fresh `memory sync` returned success and `turso db shell harnessuse` listed
`project_memory` on the remote. A reviewed claim (`README.md:51`, key `memory database`) was
recorded. After a README correction, a second entry explicitly superseded the first, retaining
both in the remote table. A fresh local data directory, configured with the same token, pulled
only the active entry in recall and reported `sync_status: synced` and
`evidence_status: source_unchanged`. This verifies the cloud path and supersession across
independent local replicas on one machine; it does not yet measure behavior on a second physical
machine or agent uptake.
The remote also accepted `BEGIN CONCURRENT; SELECT COUNT(*) FROM project_memory; COMMIT;`.

## Trial contract

- One project ledger lives at `.claudecat/memory.json`. Agents on either harness read the same
  bytes after Git synchronization; no harness-private memory is treated as this ledger.
- `record` requires an explicit claim, stable key, source file and nonblank line within the repo.
  It stores the whole source file's SHA-256 and requires `--supersedes` for a correction.
- `recall` returns the active claim and a source freshness status. A missing ledger is distinct
  from a present ledger with no match. Concurrent active claims for one key are a conflict.
- A hash match checks source bytes only. A person or agent must inspect the cited line to decide
  whether it supports the claim. Neither arbitrary transcript text nor pasted agent output is
  automatically retained.
- The first version is query-on-demand. It does not promise recall for questions whose agent
  never invokes the skill, nor semantic verification or complete capture of past decisions.

## What would count as useful

Re-run the two baseline questions in fresh Claude Code and Codex sessions after recording and
reviewing the relevant decisions in a test repo. Each should cite the same active ledger entry
and source line. Then change the source and confirm both agents report the memory as unresolved;
supersede it and confirm both choose the newer entry. A useful result requires correct retrieval
in both agents, not merely fewer context tokens.

## Local paired probe

A temporary Git repo had one source line and a shared ledger. Both fresh agents were explicitly
asked to use the project-memory workflow; Claude Code was permitted the recall command and file
read, and Codex ran read-only. With the ledger saying A and the source edited to B, both reported
`source_changed`, read the source, and refused to treat A as current. After recording B with
`--supersedes`, both reported B with `source_unchanged` and checked the cited line. This is 2/2
agents in each of two prompted states, not a recall-rate estimate or proof that agents will use
the ledger without an instruction. Codex's rollout showed the recall and source-read commands;
Claude Code's returned answer named both the status and source content.

## Collection and its limits

The original `claudecat usage.db` recorded only navigation shape and hits. On this machine,
`claudecat usage --json` reported one navigation event in the last 30 days when the trial began;
that event is unrelated to memory. The new `claudecat memory recall` logs one local row per CLI
invocation: project ID, machine stamp and its source, declared harness, success/error, ledger
state, hit counts by evidence state, and elapsed milliseconds. On Linux the machine stamp is a
SHA-256 of `/etc/machine-id`; elsewhere it falls back to a hashed hostname, or `unknown` if
neither exists. The stamp belongs to each row, so a copied database does not relabel old events;
older unstamped rows remain `legacy_unknown`. `claudecat memory usage --json` reports these rows. No query,
claim, source path or conversation text enters this table. `CLAUDECAT_NO_USAGE=1` disables it.
The harness label is supplied by the caller and is not independently verified. An error in
writing telemetry is visible on stderr while recall still answers. Existing sessions before this
change cannot be reconstructed from the new table.

These rows show exposure, source freshness, and (after the storage update) sync outcome. They do not count questions that should have used
memory but never called it, prove that an agent read the source, or grade its answer. `cort` usage
counts code-intelligence commands and cannot fill those gaps. In the original trial there was no
Turso/libSQL service; project memory moved between machines through reviewed Git commits, while
usage data stayed local. The current TursoDB deployment status is described above.

`claudecat memory eval-report <eval.json> --json` adds a second, explicit evidence path. It reads
schema 1 JSON with a `rubric_id`, frozen `cases`, and one `memory_on` and one `memory_off` trial per
`pair_id`. Every case points to a retained answer-key artifact and SHA-256; every trial points to
its agent-output artifact and SHA-256. The report refuses changed artifacts, duplicate arms,
missing cases, duplicate sessions, or a pair whose task, scenario, machine, harness, model,
prompt fingerprint, source snapshot or tool policy differs. It lists
incomplete pairs and excludes them from outcome totals. Scores remain reviewer judgments; the
hashes make the evidence inspectable, not the judgment automatically correct.

Each case has `task_id`, `scenario`, `answer_key_artifact`, and `answer_key_sha256`. Each trial has
`pair_id`, `task_id`, `scenario`, `harness`, `model`, `machine_id`, `machine_source`, `session_id`,
`prompt_sha256`, `source_snapshot` (excluding the ledger), `tool_policy_id`, `condition`,
`artifact`, `artifact_sha256`, five Boolean review fields (`lookup_observed`,
`source_read_observed`, `answer_correct`, `citation_correct`, `uncertainty_correct`), and optional
`input_tokens`, `output_tokens`, `elapsed_ms`. Artifact paths are relative to the eval JSON file.
Keep raw transcripts in a private local dataset unless they have been reviewed for publication.

The next evidence gate is a frozen, reviewed task set with past decisions, corrected decisions,
stale sources, and a false pasted agent report. Freeze the answer key and rubric before running
either arm. For each task, use the same prompt, source snapshot, model settings and tool
permissions in fresh Claude Code and Codex sessions; vary only whether the shared ledger is
available. Preserve full outputs, then score both arms against the same answer key. Report the
eligible-task denominator, complete pairs, improvements, regressions, source checks, and token
cost separately by machine, harness, model and scenario. Record whether other agent memory was
present, since it could contaminate the no-ledger arm. A successful recall count alone is not
evidence of improved correctness, and the two prompted states above are not a controlled trial.
