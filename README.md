# ClaudeCat V2 — Project Navigation Map（給 Claude Code 的導航地圖）

**引導型**：ClaudeCat 在啟動時就給 Claude Code 一張**可信任的專案地圖**
（入口、模組樹、主要符號、依賴），讓它導航時**不用盲目摸索**。
與 **cortexyoung/cort**（協助型，插在 rg 上的搜尋)互補：
cort 回答「查詢時的問題」，ClaudeCat 回答「啟動時的導航」。

## 定位（V2，2026-09 重定位）

- ❌ **不做**：「猜測式」auth/response/error pattern 偵測 + 假信心
- ✅ **只做**：從 manifest + AST 產出**可驗證的事實地圖**
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
claudecat cort-status --root <proj>   # cort 索引新鮮度 / chunks / relationships
claudecat track METRICS.md --root /repo/a --root /repo/b   # 多 repo 一次寫入
claudecat track METRICS.md --roots-file repos.txt          # 從檔案讀 repo 清單
claudecat scan --root /path/to/project
```

**迷你地圖（auto）**：專案 <300 LOC 時，讀全部比導航地圖便宜，`auto` 會自動改用
15 行內的迷你地圖（About/Entry/Run/Build/Structure/Deps/Guardrails），避免負效益。
`--map mini|full` 可手動覆蓋。

### 對 Claude Code 的使用建議

1. 在專案根目錄執行 `claudecat update`（或讓 skill/CI 定期執行）
2. CLAUDE.md 自動維護 `<!-- claudecat:auto:begin -->` 區塊
3. Claude Code 啟動時自動載入，導航零成本

## 導航能力：全圖之外，還要有「路」

> 與 cortexyoung/cort 的結合使用細節見 [CORT-INTEGRATION.md](CORT-INTEGRATION.md)。

**問題**（2026-09-05）：作為導航工具，光有「全圖」不夠——全圖是靜態的「東西在哪裡」，
真正的導航是**從一句話高速低本到達目的地**。缺少它時，Claude 仍要自己
Glob/Read 繞路（真實 session 59–94% 工具呼叫是搜尋類）。

**解法：`claudecat navigate <query>`**——輸入意圖，輸出：
1. **命中符號表**（檔案:行號 + 種類，精確命中優先）
2. **命中檔案**
3. **低成本路線**：先讀哪個檔案:行號 → 建議 `cort` 精確查詢
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

### 結合 cortexyoung：直接吃 cort 的索引（2026-09-05）

claudecat **不重造索引**——直接唯讀 cort 的 SQLite
（`~/.cache/cortex-ng/<sha256>.db`，schema v4 相容）：`chunks`（全 project 符號）、
`relationships`（calls/imports 邊）、`projects`（git_head / 索引時間）。

```bash
claudecat cort-status --root <project>    # 索引新鮮度 / chunks / relationships
claudecat navigate --cort "extractSymbolDefinitions"   # 優先查 cort 全量索引（tree-sitter 只掃 top-N）
```

- `cort-status fresh`：git HEAD 相符 + 索引 ≤7 天；STALE 時提示 `cort index`。
- `navigate --cort`：命中 cort `chunks`（**比 tree-sitter top-N 更完整**——實測 legacy 檔的
  `extractSymbolDefinitions`：tree-sitter 0 命中、cort 全量索引 1 命中）
  且路線自動含 `cort context` / `cort impact` / **反向依賴清單**。
- 唯讀雙保險：一般 `SQLITE_OPEN_READ_ONLY`；失敗（唯讀 FS / 缺 sidecar / 寫入端持鎖 BUSY）
  自動退回 `immutable=1` 直接讀主檔。claudecat 永不寫 cort 的 DB；
  DB 不存在或未命中自動回退 tree-sitter。

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
