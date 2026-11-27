---
name: claudecat
description: Run ClaudeCat (pure Rust CLI v2) to map and navigate a project, use cortexyoung/cort for explicitly recorded project memory, and quantify exploration cost. Use when the user asks about a project map, a symbol location, cort index health, or invokes /claudecat. Also gate heavy work on load and memory (`claudecat gate`) and pick a machine with room across the fleet (`claudecat fleet`), which is ClaudeCat's main job.
---

# ClaudeCat — Gate, Fleet, Project Map & cort Integration

Pure Rust CLI. Run via Bash; binary is on PATH as `claudecat`. All subcommands default to cwd (`--root` to override).

## Gate and fleet (ClaudeCat's main job)

ClaudeCat gates heavy work on resources only: load (with the load reserved for runs still warming up), memory pressure, and available memory (with its reservations). There are **no slots and no compiler lock**: any number of runs may start while load and memory allow.

```bash
claudecat gate check [--kind build|test|e2e|other]   # exit 0 = may start; exit 75 = wait (reason on stderr)
claudecat gate run [--kind …] -- <cmd>               # runs <cmd> under nice once it fits; the command's exit code, 75 when denied
claudecat status [--json]                            # this machine: load, memory, `capacity`, who holds a gate run
claudecat fleet                                      # every machine: CPUS, MEM_GB, LOAD, HEADROOM, FREE_GB, BLOCKED, WARM, RUN, COMP, SESS
claudecat fleet pick --need-mb 3072 --need-load 1    # machines with that much memory and load headroom, best first; exit 1 when none
```

- **Rules.** `max_load` = CPUs × 1.0 (`CLAUDECAT_MAX_LOAD`). Memory: PSI `some avg10` > 10 or MemAvailable < 1536 MB (`CLAUDECAT_MAX_MEM_PRESSURE`, `CLAUDECAT_MIN_AVAILABLE_MB`). Denials are `load`, `memory`, `reserved`, `duplicate` (the same Cargo command already runs in the same directory) and `resources` (load or memory could not be read). Every denial is exit 75 with a line starting `gate denied (<reason>) `. Where that line goes depends on the entry point: `gate check` and `gate run` write it to **stderr**; `gate hook` writes it to **stdout** (read stdout for a hook's denial).
- **build/test/e2e** are denied `load` when `load_1m + reserved_load + need_load > max_load`; **`other`** (the default kind) only when `load_1m > max_load`, and `reserved` when `MemAvailable − reserved_mb` is under 1024 MB (`CLAUDECAT_RESERVE_OTHER_MB`).
- **Reservations** last the warm-up window (`CLAUDECAT_RESERVE_WARMUP_SECS`, 60 s) and the load decays linearly: memory gradle 3072 MB per test fork and cargo 1536 MB (`CLAUDECAT_RESERVE_GRADLE_MB`, `CLAUDECAT_RESERVE_CARGO_MB`); load cargo max(2, cpus × 0.25) and gradle 1.0 per fork (`CLAUDECAT_RESERVE_CARGO_LOAD`, `CLAUDECAT_RESERVE_GRADLE_LOAD`).
- `CLAUDECAT_SLOTS` and `CLAUDECAT_OTHER_SLOTS` no longer apply.
- **`capacity`** in `status --json`: `cpus`, `mem_total_mb`, `max_load`, `load_1m`, `reserved_load`, `load_headroom` (never negative), `mem_available_mb`, `reserved_mb`, `mem_headroom_mb`, `mem_pressure`, `blocked` (what would deny a build now), `warming`, `warmup_secs` (the warm-up window in effect on that machine: `CLAUDECAT_RESERVE_WARMUP_SECS`, 60 s unless set to a positive number).
- **`fleet pick --need-mb N --need-load X [--json]`** needs nothing by default (0 and 0), orders by load headroom, then memory headroom, then host name, and skips machines that are `UNKNOWN` or `blocked`. JSON per host: `host`, `load_headroom`, `mem_headroom_mb`, `cpus`, `mem_total_mb`, `load_1m`, `max_load`, `blocked`, `idle`. `--kind gradle|other` is deprecated for one version (gradle = `--need-mb 3072 --need-load 1`, other = `--need-mb 1024 --need-load 0`) and says so on stderr, naming the needs in effect (a `--need-mb` or `--need-load` you passed shows its own value there). A row older than the reporting machine's warm-up window (`capacity.warmup_secs`, 60 s when the row does not say) is read without its reservations.
- **`fleet` figures of who runs.** RUN is `holders` (the `gate run` jobs) and COMP is `compiling` (compiler processes); they are two figures and never summed. `-` in the table and `null` in the JSON mean the row cannot tell, not that nothing runs: every `IDLE` row (the machine has stopped reporting), and `holders` on a machine still running a build from before #5955.
- On a denial, wait as the message says and then retry; do not poll with `sleep` in the foreground.

## Commands

```bash
claudecat scan                  # 掃描專案，輸出導航地圖（--format text|markdown|json，--map mini|auto|full）
claudecat explore --json        # 量化探索成本：無地圖 vs 地圖的 token 粗估
claudecat navigate "auth"       # 從一句話找目的地符號/檔案；--cort 優先用 cort 全量索引（需先 cort index）
claudecat usage --json           # 查看本機導航命中統計（不含原始查詢）
cort memory recall "<過去決策>" --harness claude-code # 讀取有來源的專案記憶
cort memory status                 # 目前專案識別碼、同步狀態和衝突
claudecat update --dry-run      # 更新 claudecat 自動區塊；--dry-run 只顯示差異
claudecat cort-status           # cort 索引狀態：新鮮度、chunk、relationships
```

## Project memory through cort

For a past project decision, use `cort memory recall "<query>"` from the target repo.
`claudecat memory` forwards active memory commands to cort for compatibility. Cortexyoung
owns the memory table client, credentials, local replica, and sync behavior.
Pass `--harness claude-code` or `--harness codex` when known; this is a declared label, not
verified harness identity. `claudecat memory usage --json` counts only older ClaudeCat recall rows, not
eligible questions or correct answers. A paired, reviewer-scored report uses
`claudecat memory eval-report <eval.json> --json`.
TursoDB holds explicitly curated project memory. `sync_status: synced` means this invocation
completed push and pull; `offline_or_pending` means the local replica may be behind.
The legacy file `.claudecat/memory.json` is imported only by `memory import-legacy`, not automatically
extracted from conversations. Treat `source_unchanged` as a byte-fingerprint check only; open the
cited `source:source_line` before relying on the claim. Treat `source_changed`, `source_missing`,
and `conflict` as unresolved. `ledger_status: absent` means no shared ledger exists here; an empty
`hits` array in a present ledger means only that this query found no match. A new claim requires
`cort memory record --key K --claim C --source RELATIVE_FILE --line N`; replacing one
requires `--supersedes OLD_ID`. Review the cited source line
before relying on it. Do not copy an unverified pasted agent report into memory.
Cloud records require a tracked, committed source file. If `memory status --json` lists a
conflict, use `memory resolve --key K --claim C --source RELATIVE_FILE --line N` with one
`--supersedes ID` for every active ID under that key. A resolution creates a reviewed new
entry and keeps the old entries for audit. `source_line_status: same_line` helps locate an
edit, but `source_changed` still requires reviewing the full source context.
`source_line_status: moved` means the cited text now sits only at `source_line_now`; it locates
the line but does not vouch for the claim. Read the line and its section there; skip the
supersession only if that context still supports the claim. `recorded_machine` is the writer's
self-reported machine id (a hint, not proof; absent on entries an older cort wrote).

## Where output goes

- `update` writes **only** the `claudecat:auto` / `guardrails` / `map-pointer` blocks in the rules file (AGENTS.md / CLAUDE.md symlink) — never touches hand-written rules.
- The generated project map goes to `~/.local/share/claudecat/<project-id>/map.md` (`CLAUDECAT_DATA_DIR` overrides); `update` prints the `map ->` path. Maps do not go into git.

## Deployment boundaries

- Local maps and usage DBs stay per-machine (kimi denominators differ across hosts by design). Project memory uses TursoDB; configure each machine with `cort memory configure`, then `cort memory sync`.
- No MCP server — never re-register one; the CLI is the only interface.
- No `npm install -g claudecat` — install with `cargo install --path <repo>` from the Rust checkout; `~/.local/bin/claudecat` typically symlinks to the repo's `target/release/claudecat`.

## Markdown navigation sidecar

`navigate` also indexes gitignore-aware Markdown headings independently from code symbols.
When a document matches, follow the reported `cort read <file> --start N --end N` route;
run it from the target project root. A document heading never becomes a `cort impact` symbol.
Navigation usage is recorded locally without raw query text; set `CLAUDECAT_NO_USAGE=1` to
disable it. Use `claudecat usage --json` to inspect whether the sidecar is helping.
