# ClaudeCat V2 — Project Navigation Map（給 Claude Code 的導航地圖）

**引導型**：ClaudeCat 在啟動時就給 Claude Code 一張**可信任的專案地圖**
（入口、模組樹、主要符號、依賴），讓它導航時**不用盲目摸索**。
與 **cortexyoung/cort**（協助型，插在 rg 上的搜尋)互補：
cort 回答「查詢時的問題」，ClaudeCat 回答「啟動時的導航」。

## 定位（V2，2026-09 重定位）

- ❌ **不做**：「猜測式」auth/response/error pattern 偵測 + 假信心
- ❌ **不做**：transit 壓縮——不改寫 agent 與 LLM 之間的任何位元組，
  地圖每行（`file:symbol:line`）必須逐字可查；與壓縮層（headroom 等）互補不重疊
- ✅ **核心**：從 manifest + AST 產出**可驗證的事實地圖**；另有明示記錄的專案記憶試驗
- 對齊 Anthropic 官方大型 codebase 策略（agentic search > RAG）：
  給 Claude 精簡地圖（≤150 行），讓它自己做有目標的探索

## 安裝 / 建置

```bash
cargo build --release            # 產生 target/release/claudecat（單一二進位）
```

## 使用

```bash
claudecat scan                   # 輸出導航地圖（markdown，auto 依規模選樣式）
claudecat scan --format json     # JSON 輸出
claudecat scan --format text     # 純文字輸出
claudecat scan --map mini        # 強制迷你地圖（小專案省 token）
claudecat scan --map full        # 強制完整導航地圖
claudecat update                 # 更新 CLAUDE.md 的 claudecat 自動區塊（原子寫入）
claudecat update --dry-run       # 只看會不會變，不寫入
claudecat explore                # 量化探索成本（地圖 token vs 全讀 token）
claudecat explore --json         # 機器可讀指標輸出
claudecat track SESSION-EVIDENCE.md  # 把指標寫入長期指標表（原子、同日不重複）
claudecat navigate "<要找什麼>"      # 從一句話給出目的地符號/檔案 + cort 路線
claudecat navigate --cort "<要找什麼>"  # 優先吃 cort 全量索引（含反向依賴）
claudecat usage --json             # 顯示本機導航統計（不含原始查詢）
claudecat memory recall "deployment" --json  # 查有來源的專案記憶及來源新鮮度
claudecat memory status --json         # 專案識別碼、同步狀態、有效記憶與衝突
claudecat memory usage --json           # 本機記憶查詢狀態；非正確率
claudecat telemetry sync --json         # 將本機結構化收集事件同步到共用 TursoDB
claudecat cort-status --root <proj>   # cort 索引新鮮度 / chunks / relationships
claudecat track METRICS.md --root /repo/a --root /repo/b   # 多 repo 一次寫入
claudecat track METRICS.md --roots-file repos.txt          # 從檔案讀 repo 清單
claudecat scan --root /path/to/project
```

## 可審核的專案記憶（Rust 試驗版）

`memory` 只處理**明示記錄**的專案主張，不自動把對話或貼來的 agent 報告寫成事實。
記憶現在存於 TursoDB，同一資料庫與 Git origin 的專案識別供 Claude Code、Codex 和其他機器共用。
先建立 **TursoDB**（本次使用 `turso db create --tursodb harnessuse`），取得 URL 與 token，於每台機器設定：
`turso://` 或 `libsql://` 是連線 URL 前綴；資料庫類型以 `turso db show` 的 Type 判定。

```sh
turso db tokens create harnessuse | claudecat memory configure --url "$(turso db show harnessuse --url)" --token-stdin
claudecat memory sync
```

token 只從 stdin 讀取，保存在本機 `~/.local/share/claudecat/turso-memory.json`（Unix 權限 0600）；
同步副本也在本機資料目錄，不進 Git。若沒有 Git origin，設定相同的
`CLAUDECAT_MEMORY_PROJECT` 以讓不同機器選到同一專案。舊 `.claudecat/memory.json` 可用
`claudecat memory import-legacy` 明示匯入；確認同步後再另行處理舊檔。`--root` 可指定目標 repo。

```bash
claudecat memory record --key "deployment option" --claim "Option B is current" \
  --source docs/decision.md --line 2
claudecat memory status --json
claudecat memory recall "deployment option" --json
claudecat memory record --key "deployment option" --claim "Option C is current" \
  --source docs/decision.md --line 2 --supersedes <舊紀錄的 id>
# 同一 key 有兩筆以上有效紀錄時，列出所有 ID，再以新主張明確收斂：
claudecat memory resolve --key "deployment option" --claim "Reviewed choice" \
  --source docs/decision.md --line 2 --supersedes <ID1> --supersedes <ID2>
```

上傳前要求來源檔已追蹤且提交。記錄保留來源路徑、行號、整檔與來源行 SHA-256。
`source_line_status: same_line` 只協助定位變動，不能把 `source_changed` 改判為可信。
`recall` 的 `source_unchanged` 只表示來源位元組
與記錄時相同，**不證明主張確實由該行支持**；`source_changed`、`source_missing`、
`conflict` 都不得當成現行事實。同 key 的更正必須明確指出舊 id，舊紀錄保留作審計。
格式損毀時查詢報錯，不回傳看似完整的空集合。這是來源與更新契約的第一輪，還沒有
語義驗證、跨 repo 個人偏好、或自動載入到代理上下文的 hook。
JSON 輸出包含 `ledger_status`（`absent` / `present`）、`sync_status` 及 `hits`；
空的專案記憶不等於確認沒有相關決策。導航地圖及統計資料仍是本機資料。
`memory recall` 的 `sync_status` 為 `synced` 才表示這次 push/pull 成功；
`offline_or_pending` 的結果只代表本機副本，不能當成跨機最新版本。
`memory recall` 可加 `--harness claude-code|codex` 記自報來源；`memory usage --json` 只統計
本機查詢、命中、來源與同步狀態，不存查詢原文、主張或來源路徑。這些數字不能判定答案正確；
`memory eval-report <eval.json> --json` 對有凍結答案依據與原始輸出雜湊的成對試驗計分，
格式及評分限制見 [試驗紀錄](docs/2026-10-18-memory-trial.md)。

## 共用資料庫的結構化資料收集

`cort` 的 hook 依目前程式預設只把判定寫入本機 `usage.db`，不對 Claude Code、Codex
或 Kimi 輸出建議；`PostToolUse` 仍可修復既有索引。ClaudeCat 的導航和記憶查詢也先
寫入本機 SQLite。網路同步由獨立的 Rust 指令執行，避免每次 hook 等待 TursoDB：

```sh
claudecat telemetry sync --json
```

這個指令逐筆上傳 `cort.command_log`、ClaudeCat 的 `navigation_log` 與
`memory_recall_log`，以及 `cort-evals cost-collect` 產生的每日 JSON。上傳內容保留
本機原欄位、機器標記、專案標記；重跑以事件 ID 去重。缺少來源會在回報的
`missing_sources` 指出。`telemetry_project_aliases` 把本機路徑專案 ID 映到 Git-origin
共享 ID；無法映射的事件數由 `unmatched_project_rows` 明列，不應跨機合併。
舊導航列沒有逐列機器標記，會明列 `legacy_unknown`；新列
才記錄機器 ID。`cort` schema-v2 新列也逐筆記錄機器 ID；schema-v1 舊列只有整個
DB 的機器戳，搬過 DB 後無法可靠拆分逐列來源。同步回報以 `legacy_cort_rows` 明列
這個風險，分析時仍須檢查混機狀態。這條路徑收集的是現有結構化紀錄與每日彙總，
不是原始對話逐字稿。Turso 同步副本會拉取整個資料庫，因此要觀察容量與首次同步時間。
記憶查詢與遙測上傳各用自己的本機同步副本及檔案鎖，雲端仍是同一個
`harnessuse`；長時間的遙測上傳不會佔住記憶查詢的本機鎖。
專案別名取同步當下的 Git origin；若 repo 改過 origin，它不能證明舊事件當時屬於
同一遠端。`by_source`、`unattributed_machine_rows`、`unmatched_project_rows` 與
`missing_sources` 是每次同步的資料品質檢查，不能把未歸屬的列合併分析。

每日收集可以用 `systemd/claudecat-telemetry-sync.{service,timer}`。先安裝此 Rust
binary，再把兩個 unit 放到 `~/.config/systemd/user/`，執行
`systemctl --user daemon-reload` 和
`systemctl --user enable --now claudecat-telemetry-sync.timer`。此 timer 每日
01:45 UTC 執行；與 `cortexyoung/evals/systemd/cort-cost-collect.timer` 的 01:30 UTC
錯開。每台機器都需設定自己的 Turso token 並安裝兩個 timer，才會有該機的完整後續資料。

**迷你地圖（auto）**：專案 <300 LOC 時，讀全部比導航地圖便宜，`auto` 會自動改用
15 行內的迷你地圖（About/Entry/Run/Build/Structure/Deps/Guardrails），避免負效益。
`--map mini|full` 可手動覆蓋。

### 對 Claude Code 的使用建議

1. 在專案根目錄執行 `claudecat update`（或讓 skill/CI 定期執行）——Project Map 寫到
   **各機器的資料目錄** `${XDG_DATA_HOME:-$HOME/.local/share}/claudecat/projects/<project-id>/map.md`
   （`CLAUDECAT_DATA_DIR` 可覆蓋），輸出印出 `map ->` 確切路徑
2. CLAUDE.md（或其 symlink 本體 AGENTS.md）**只放手寫規則**：v2.1 首次執行會自動移除
   舊版 `claudecat:auto` 區塊，並播下 guardrails 與「地圖位置」兩個種子（種一次、永不改寫）
3. 要看地圖就讀 `map ->` 指到的檔；Claude Code 啟動時載入的 CLAUDE.md 保持精簡，
   `git status` 不再被機器生成的內容弄髒

> 注意：把 `CLAUDECAT_DATA_DIR` 指進專案目錄內，會讓掃描把地圖也算進專案——預設位置
> 在 HOME 下，不受影響。

## 導航能力：全圖之外，還要有「路」

> 與 cortexyoung/cort 的結合使用細節見 [CORT-INTEGRATION.md](CORT-INTEGRATION.md)。

**問題**（2026-09-05）：作為導航工具，光有「全圖」不夠——全圖是靜態的「東西在哪裡」，
真正的導航是**從一句話高速低本到達目的地**。缺少它時，Claude 仍要自己
Glob/Read 繞路（真實 session 59–94% 工具呼叫是搜尋類）。

**解法：`claudecat navigate <query>`**——輸入意圖，輸出：
1. **命中符號表**（檔案:行號 + 種類，精確命中優先）
2. **命中檔案**
3. **文件段落**（Markdown heading path + 起訖行號；不把文件標題當成程式符號）
4. **低成本路線**：先讀哪個檔案:行號 → 建議 `cort` 精確查詢
   （`cort context <symbol> --content full -f lean`、`cort impact --symbol <symbol>`）
   → 沒中時擴大搜尋指令

```bash
claudecat navigate "guardrail"
# → src/guardrails.rs:4 mod guardrails
# → 路線: cort context guardrails --content full -f lean …
claudecat navigate "auth" --json   # 機器可讀
```

**分工**：`scan/update` = 全圖（場景）；`navigate` = 路線（導航）；
`cortexyoung/cort` = 精準定位（到達後深挖）。三者串成「快速低本到達目的地」。

Markdown 文件是 `navigate` 的獨立 sidecar：它只索引 gitignore-aware 的 `.md` heading，
保存 heading path、起訖行號與一行預覽。文件命中只建議 `cort read`，不會產生
`cort impact` 或改變 caller-set / coverage 的判定。frontmatter、fenced code 與
`legacy`/`archive` 等排除目錄不會進入文件索引。

`claudecat usage` 讀取本機資料目錄下的 `usage.db`，只保存導航統計：query shape、
命中數、是否使用 `--cort`、路由類型與文件位置；原始查詢不落盤。設置
`CLAUDECAT_NO_USAGE=1` 可停用記錄。統計是改善排序、文件範圍與 Claude Code 導航路由的
證據，不代表使用者採用了建議；真正的 `cort` 使用仍由 `cort` 自己的 local usage.db 記錄。

### 結合 cortexyoung：直接吃 cort 的索引（2026-09-05）

claudecat **不重造索引**——直接唯讀 cort 的 SQLite
（`~/.cache/cortex-ng/<sha256>.db`，schema v5 相容）：`chunks`（全 project 符號）、
`relationships`（calls/references/imports 邊）、`projects`（git_head / 索引時間）、
`_cortex_meta`（schema 版本 / `graph_pending`）。

```bash
claudecat cort-status --root <project>    # 索引新鮮度 / chunks / relationships
claudecat navigate --cort "extractSymbolDefinitions"   # 優先查 cort 全量索引（tree-sitter 只掃 top-N）
claudecat cort-audit --root <project>    # 驗證整合成效：健康 / 覆蓋缺口 / FTS / 用量 / harness 切面；--track 寫長期指標
```

- `cort-status fresh`：git HEAD 相符 + 索引 ≤7 天 + `graph_pending != 1`；STALE 時提示 `cort index`。
  `graph_pending=1`（schema 遷移後未重建、或增量中斷）代表 relationships 是舊邊——
  **反向依賴先別信**，且此時 HEAD/時戳可能仍是新的，所以另行提示、不與 STALE 混講；
  讀不到 `_cortex_meta`（舊版 DB）→ 顯示 `?`，不視為健康、也不據此判 STALE。
- `navigate --cort`：命中 cort `chunks`（**比 tree-sitter top-N 更完整**——實測 legacy 檔的
  `extractSymbolDefinitions`：tree-sitter 0 命中、cort 全量索引 1 命中）。
  symbol_name 未命中時自動 fallback 到 **`chunks_fts` 全文**（content/file 命中；
  與 `cort context` 的 `resolution=fts` 同源；注意 `cort recall` 是搜 reading_notes，非 code 索引）。
  路線自動含 **content 摘要**（省一次 read）/ `cort context` / `cort impact` / **反向依賴清單**。
- 唯讀雙保險：一般 `SQLITE_OPEN_READ_ONLY`；失敗（唯讀 FS / 缺 sidecar / 寫入端持鎖 BUSY）
  自動退回 `immutable=1` 直接讀主檔。claudecat 永不寫 cort 的 DB；
  DB 不存在或未命中自動回退 tree-sitter。
- `cort-audit`（收集數據驗證 → 據此改善）：唯讀彙整 ①索引健康（fresh/HEAD/age）
  ②覆蓋缺口（file_state 有、chunks 無的檔案＝completeness 缺口）③FTS 同步
  ④用量（cort usage.db：命令分佈、hook-suggest/refresh 結果、errors、index_stale、saved_bytes）
  ⑤**harness 切面**（`harness` 只在 hook payload 上，所以這張表講的是「router 面對誰」，
  不是「誰用了 cort」；無 `harness` 欄的舊列另計，`harness_declared` 不符的列數也照實列出），
  並依規則給「解讀 & 行動」；`--track <file>` 把每天一列寫進長期指標表（同日更新），
  例如 `CORT-AUDIT.md`，讓「有沒有達成」變成可看的趨勢。

## 導航地圖內容（全部是事實）

| 區塊 | 來源 |
|---|---|
| 專案類型 / 語言 / 框架 / 套件管理器 | manifest（package.json / Cargo.toml / pyproject.toml / go.mod） |
| Entry points / Run / Build | manifest 宣告 |
| 目錄結構（檔案數、LOC） | gitignore-aware walk（`ignore` crate） |
| 主要檔案 & 符號（fn/class/struct/…） | tree-sitter AST |
| Dependencies（宣告） | manifest |

- 支援語言：JS/TS、Python、Rust、Go、C/C++（tree-sitter grammar）
- 自動排除：node_modules、target、dist、.git、legacy 等

## 技術決策 Guardrails（開發者維護，永不覆寫）

`claudecat update` 首次執行會在 CLAUDE.md 附加：

```markdown
<!-- claudecat:guardrails:begin -->
- 2D tilemap + Macroquad（禁 Python/3D）  ← 例：一行一條技術決策
<!-- claudecat:guardrails:end -->
```

此區塊**只存在時不動、不存在才建立**，開發者自由編輯；`scan`/`explore` 會讀出
顯示。也可用根目錄 `claudecat-guardrails.md` 優先提供。
真實案例佐證：GalaxyWarHero session 中 Claude 因缺「技術決策」把 2D 專案當 3D
分析、裝錯工具——見 [SESSION-EVIDENCE.md](SESSION-EVIDENCE.md)。

**2026-09-05 獨立評審（Grok）**：抓到 26 條問題，其中 6 個 P0（目錄 LOC 雙計、
大 repo 截斷、dual-manifest 標錯語言、explore 假指標、track 誤刪兄弟 repo、
update 汙染父專案）全部實測屬實並已修復＋回歸測試；每條 corroboration 與
剩餘待辦見 [GROK-REVIEW-CORROBORATION.md](GROK-REVIEW-CORROBORATION.md)。

## 誠實原則（修復 V1 假信心）

- 只報可驗證事實，明確標 `*Generated at …* no inference`
- 偵測不到就空白，不標「100% High Confidence」
- CLAUDE.md 更新為原子寫入（temp + rename），無變動不寫

## V1 歷史

V1 是 TypeScript MCP server（「主動偵測 pattern + 信心分數」），因假信心與
誤判問題於 V2 重定位；舊程式碼封存於 [legacy/](legacy/)，研究說明見
[RESEARCH-V2.md](RESEARCH-V2.md)、[CLAUDECAT-GOALS.md](CLAUDECAT-GOALS.md)。

## 授權

MIT
