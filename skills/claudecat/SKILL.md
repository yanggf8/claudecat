---
name: claudecat
description: Run ClaudeCat (pure Rust CLI v2) to map and navigate a project, query explicitly recorded project memory with source freshness, quantify exploration cost, integrate with cortexyoung/cort, and run the daily cort-audit. Use when the user asks about a past project decision, a project map, a symbol location, cort index health, the daily audit, or invokes /claudecat.
---

# ClaudeCat — Project Map & cort Integration

Pure Rust CLI. Run via Bash; binary is on PATH as `claudecat`. All subcommands default to cwd (`--root` to override).

## Commands

```bash
claudecat scan                  # 掃描專案，輸出導航地圖（--format text|markdown|json，--map mini|auto|full）
claudecat explore --json        # 量化探索成本：無地圖 vs 地圖的 token 粗估
claudecat navigate "auth"       # 從一句話找目的地符號/檔案；--cort 優先用 cort 全量索引（需先 cort index）
claudecat usage --json           # 查看本機導航命中統計（不含原始查詢）
claudecat memory recall "<過去決策>" --json # 讀取有來源的專案記憶
claudecat memory status --json       # 目前專案識別碼、同步狀態和衝突
claudecat telemetry sync --json      # 上傳現有結構化收集事件
claudecat update --dry-run      # 更新 claudecat 自動區塊；--dry-run 只顯示差異
claudecat cort-status           # cort 索引狀態：新鮮度、chunk、relationships
claudecat doctor                # 體檢追蹤循環；--install 一鍵部署 crontab（冪等）
claudecat cort-audit --track CORT-AUDIT.md   # 每日審計 + 寫長期指標表（原子、同日更新）
claudecat findings - <<'EOF'    # 把每日分析發現寫進文件發現區塊（--file 預設 CORT-AUDIT.md）
...markdown findings...
EOF
```

## Project memory trial

For a past project decision, use `claudecat memory recall "<query>" --json` from the target repo.
Pass `--harness claude-code` or `--harness codex` when known; this is a declared label, not
verified harness identity. `claudecat memory usage --json` counts local recall states only, not
eligible questions or correct answers. A paired, reviewer-scored report uses
`claudecat memory eval-report <eval.json> --json`.
TursoDB holds explicitly curated project memory. `sync_status: synced` means this invocation
completed push and pull; `offline_or_pending` means the local replica may be behind.
The legacy file `.claudecat/memory.json` is imported only by `memory import-legacy`, not automatically
extracted from conversations. Treat `source_unchanged` as a byte-fingerprint check only; open the
cited `source:source_line` before relying on the claim. Treat `source_changed`, `source_missing`,
and `conflict` as unresolved. `ledger_status: absent` means no shared ledger exists here; an empty
`hits` array in a present ledger means only that this query found no match. A new claim requires
`claudecat memory record --key K --claim C --source RELATIVE_FILE --line N`; replacing one
requires `--supersedes OLD_ID`. Review the cited source line
before relying on it. Do not copy an unverified pasted agent report into memory.
Cloud records require a tracked, committed source file. If `memory status --json` lists a
conflict, use `memory resolve --key K --claim C --source RELATIVE_FILE --line N` with one
`--supersedes ID` for every active ID under that key. A resolution creates a reviewed new
entry and keeps the old entries for audit. `source_line_status: same_line` helps locate an
edit, but `source_changed` still requires reviewing the full source context.

## Where output goes

- `update` writes **only** the `claudecat:auto` / `guardrails` / `map-pointer` blocks in the rules file (AGENTS.md / CLAUDE.md symlink) — never touches hand-written rules.
- The generated project map goes to `~/.local/share/claudecat/<project-id>/map.md` (`CLAUDECAT_DATA_DIR` overrides); `update` prints the `map ->` path. Maps do not go into git.

## Daily audit workflow

1. `claudecat cort-audit --track CORT-AUDIT.md` — appends/updates today's row in the long-term metrics table.
2. Read the full report (index health, coverage gaps, hook census, harness hit rates).
3. `claudecat findings -` with a curated markdown summary (date, key deltas, rule decisions) — replaces that day's findings block atomically, preserving everything outside the block.

## Deployment boundaries

- Local maps and usage DBs stay per-machine (kimi denominators differ across hosts by design). Project memory uses TursoDB; configure each machine with `memory configure`, then `memory sync`.
- Structured cort and ClaudeCat usage rows and daily cost reports are copied to the same
  TursoDB by `claudecat telemetry sync`; hooks still write locally first. The upload keeps
  every source row and its available machine attribution. Old navigation rows are explicitly
  `legacy_unknown`. Raw conversation transcripts are outside this command's scope.
- No MCP server — never re-register one; the CLI is the only interface.
- No `npm install -g claudecat` — install with `cargo install --path <repo>` from the Rust checkout; `~/.local/bin/claudecat` typically symlinks to the repo's `target/release/claudecat`.

## Markdown navigation sidecar

`navigate` also indexes gitignore-aware Markdown headings independently from code symbols.
When a document matches, follow the reported `cort read <file> --start N --end N` route;
run it from the target project root. A document heading never becomes a `cort impact` symbol.
Navigation usage is recorded locally without raw query text; set `CLAUDECAT_NO_USAGE=1` to
disable it. Use `claudecat usage --json` to inspect whether the sidecar is helping.
