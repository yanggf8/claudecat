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

## Can current collection validate the benefit?

No. `claudecat usage.db` records navigation shape and hits, not `memory record`, `memory recall`,
whether an agent read the cited line, or whether its answer was correct. On this machine,
`claudecat usage --json` reported one navigation event in the last 30 days at the time of this
note; that event is unrelated to memory. `cort` usage counts code-intelligence commands, so it
cannot supply the missing memory denominator or grade answers. The paired probe above establishes
that the workflow can work when explicitly requested; it cannot show spontaneous adoption or a
benefit over reading the source directly.

The next evidence gate is a frozen, reviewed task set with past decisions, corrected decisions,
stale sources, and a false pasted agent report. Run each task with and without the shared ledger
in fresh Claude Code and Codex sessions using the same prompt, repo state, model settings, and
tool permissions. Grade (1) whether a memory lookup happened, (2) whether the cited source was
read, (3) answer correctness, and (4) whether uncertainty was disclosed when evidence was stale
or absent. Record harness, model, machine, task ID, token use and elapsed time separately. Report
the eligible-task denominator and paired differences; a count of successful recalls alone would
only measure availability, not improvement.
