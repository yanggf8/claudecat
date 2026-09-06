# cortexyoung/cort 整合（2026-09-05）

## 問題（使用者的提問）
「結合使用時，能利用 cortexyoung 的索引或其他資源嗎？」

## 答案
**能。** claudecat 直接唯讀 cort 的 SQLite 索引（不重造、不改寫），
用 cort 做「精準定位層」、claudecat 做「地圖與路線層」。

## cort 提供什麼（schema v4）
| 表 | 內容 | claudecat 用途 |
|---|---|---|
| `projects` | project_id / name / path / git_head / last_indexed_at / extractor_version | `cort-status` 新鮮度 |
| `chunks` | file_path / symbol_name / chunk_type / start_line / end_line / content / language | `navigate --cort` 全量符號查詢 |
| `relationships` | source→target 邊（imports/exports/calls）+ call_site_line + confidence | 反向依賴路線 |
| `chunks_fts` | FTS5 external-content（content/symbol/file，tokenize unicode61） | `navigate --cort` 全文 fallback（symbol 未命中時） |

- DB 路徑：`$CORT_CACHE_DIR/<sha256>.db`，`project_id = sha256(real_path)`（0.40 rusqlite 唯讀開啟）
- cort 的 `last_indexed_at` 是 **epoch 毫秒**（13 位數）；freshness 以毫秒比對
  （曾誤當秒導致 7 天關卡失效，2026-09-06 修復，見下方複審）

## 誠實與邊界
- claudecat **唯讀** cort DB（`SQLITE_OPEN_READ_ONLY`）；永不寫入
- 唯讀雙保險：一般唯讀失敗（唯讀檔案系統、缺 -shm/-wal sidecar、寫入端持 EXCLUSIVE lock
  導致 query BUSY）時，自動退回 `immutable=1`（SQLite 完全不碰 sidecar/lock，直接讀主檔；
  代價是 cort 若有未 checkpoint 的 WAL 內容會讀不到——可接受的誠實取捨）
- cort 索引 STALE → `cort-status` 明確提示 `cort index`，不假裝新鮮
- cort DB 不存在 / 未命中 → `navigate` 自動回退 tree-sitter 地圖（並提示）

## 重驗（2026-09-05，換機 + 新版 cort 0.1.0，主要是 reindex）
- 換機後 claudecat 尚未被索引 → `cort index`（全量）：
  **68 files / chunks 551 / relationships 250 / fresh**（git HEAD 385bd7f349 相符，`index_is_stale: false`）
  （舊機數字 chunks=1150 relationships=2034 是新版 extractor 粒度不同，非回歸）
- 新版 schema 相容：claudecat 讀的 `projects / chunks / relationships` 欄位全部仍在；
  新增 `_cortex_meta / file_state / raw_edges / reading_notes / chunks_fts` 不影響唯讀查詢；
  `relationships` 新增 `call_form`、`confidence` 文字欄，claudecat 讀的 `rel_type / call_site_line / confidence_score` 不變
- reindex 行為驗證：`cort index --incremental` 在無基底時 0 examined（需先全量一次）；
  建立基底後 **hook-refresh 自動增量** — 編輯檔案後 chunks 548→551 自動更新、status 持續 `stale:false`
- 唯讀 fallback 修復（受限環境實測）：sandbox 把 `~/.cache` 設唯讀 + WAL DB 無 sidecar 時
  一般唯讀開不起來 → 新增 `immutable=1` fallback 後，`cort-status` / `navigate --cort` 在受限環境照常讀取；
  新測試 `cort_readonly_fallback_when_writer_holds_exclusive_lock` 以 EXCLUSIVE lock
  確定性重現「一般唯讀 query BUSY → fallback 仍讀到」，31+1=32 測試全綠
- 新 probe（舊證據 `rust/tests/staleness_cwd.rs` 已隨 V2 重寫消失）：
  `extractSymbolDefinitions`（legacy/src/core/ast-parser.ts:115）— tree-sitter **0 命中**、
  `navigate --cort` **1 命中** + 完整路線（cort context / impact）✅；反向依賴對新 schema 也通
  （persona-core `setup_conn` → 5 個 calls 依賴者）

## 實測（2026-09-05，cortexyoung 真實索引）
- `cort-status`：chunks=1150 relationships=2034，STALE（索引 HEAD 4042a7a6 ≠ 目前 7c2ad21d）✅ 誠實
- `navigate --cort "staleness"`：找到 tree-sitter top-30 找不到的符號
  （`rust/tests/staleness_cwd.rs:72`），路線含 cort context / impact / 反向依賴 ✅

## 對照（2026-09-06，`cort recall` / `cort context` vs `navigate --cort`）
- `cort context "immutable"` → `resolution=fts seeds=4`：cort 自己的 code 全文也是走 `chunks_fts`，
  且 4 個 seeds 與 `claudecat navigate --cort "immutable"` 的 4 筆 FTS 命中**完全相同** ✅
- `cort recall "immutable"` → `readings=0`：recall 是搜 `reading_notes`（需先有 read/note 進度），
  非 code 索引；**claudecat 的 FTS fallback 對應的是 `cort context`（resolution=fts），不是 recall**（文件已修正）
- 分工不變：`navigate --cort` 給路線 + 壓縮摘要（省 read）+ 反向依賴；
  `cort context` 給全量 content + call graph（icalls/ocalls/unresolved）——到達後的深挖層

## 再確認（2026-09-06，cortexyoung 又改版）
- 新版索引仍為 v4 schema（`projects/chunks/relationships/chunks_fts` 欄位對 claudecat 零影響），
  `extractor_version` 同前一版；新增 `usage.db`（command_log）與 claudecat 無關
- `chunks_fts` 確認為 **external-content FTS5**（`content=chunks, content_rowid=rowid`），
  唯讀 MATCH + JOIN chunks 實測可用 → 實作 FTS fallback（見下）

## 驗證迴圈：cort-audit（2026-09-06）
把「有沒有幫到 cort 的目的」變成**可收集、可追蹤的數據**（全部唯讀）：
- `claudecat cort-audit --root X [--window 30] [--json] [--track FILE]`
- 數據：①索引健康（fresh/HEAD 相符/age）②覆蓋缺口（file_state 有、chunks 無的檔案——
  這是 cort 自己 CLAUDE.md 開出的 completeness 缺口之一「a file the screen never read」）
  ③FTS 同步（chunks_fts docs vs chunks）④用量（cort usage.db：命令分佈、hook-suggest/
  refresh 結果、errors、index_stale、saved_bytes）
- 行動：報告尾「解讀 & 行動」依規則給建議；`--track` 每天一列寫 `CORT-AUDIT.md`（同日更新），
  時間序列驗證改善（如：未 chunk 檔數量是否下降、命中率是否上升）
- 首筆實測（2026-09-06，本 repo）：fresh、chunks=576 rels=294、FTS synced、
  **coverage 缺口 5 檔（legacy/test-*.js）**、hook-suggest 命中率 **35/6816（<1%）**、
  context+recall=3 —— 三條行動建議全部是有數據支撐的
- 附帶收穫：驗證過程抓出 `?1` 重複綁定參數的 rusqlite bug（`InvalidParameterCount` 被
  `unwrap_or(0)` 吞掉、假裝「無缺口」）→ 改 qmark 後正確回報 5 檔——正是「收集數據驗證」
  的價值，另有 3 支回歸測試保護

## 複審（2026-09-06 同日：數據正確性審查 → 兩個 P1 修復 → 三個 cortexyoung 議題）
逐項對照真實 DB 重驗首筆實測（多數可重現），審查抓到兩個 bug 並已當場修復（`03c509a`）：
- **P1：`cort-status` 的 fresh 關卡永不觸發** — `freshness()` 把 `last_indexed_at` 當秒，
  實際是毫秒（DB 實測 `1788686742481`）：40 天前的索引 `cort-status` 報 `fresh`、
  `cort-audit` 報 `STALE`，同一欄位兩套口徑。修復：共用 `is_fresh()`（`FRESH_WINDOW_MS`），
  附 40 天→STALE 回歸測試；上節「相容秒/毫秒」的說法一併修正。
- **P1：`--track` 靜默刪除表格後的內容** — `track_table()` 從 section 掃到 EOF 重寫，
  使用者筆記／其他 section 直接消失（實測重現）。修復：section 範圍改為「到下一個
  `#` 標題為止」，範圍外原樣保留；附 2 支回歸測試（含 explore+audit 雙 section 共存檔）。
  修好前 `CORT-AUDIT.md` 的時間序列其實不可信——這是先修它的理由。
- 數據覆核（直接 SQL 查 usage.db / 專案 DB）：
  - hook-suggest 命中 36/7056（0.5%），其中 `no_shape` 5857（83%）——最大槓桿（cortexyoung#3）
  - 報告裡 1032 筆 `unparsed` 全是 2026-09-01→02 的舊格式歷史列（`args_summary` 為字串
    `hook`）；09-02 起即為 `{"hook":…,"v":1}`——量測問題已自解，不需行動
  - `saved_bytes` 只在 `source=store && effective=receipt` 非零（cort `usage.rs:176-186`），
    30 天 11.4k 命令僅 1 筆非零（13 bytes）→「省了多少」目前實際沒被量到（cortexyoung#4）
  - coverage「缺口」5 檔全是 2–3 行、只 import 後呼叫的 driver script（無任何宣告）→
    extractor 沒漏，是指標語意問題（cortexyoung#2）；hint 文案已對齊（`0dc1599`）
- 修復後複驗：40 tests 綠（+3 回歸）、touched files clippy 0 warnings、兩個重現腳本行為翻轉
  為正確；`CORT-AUDIT.md` 同日列更新（11461 命令 / chunks 581）

## 待辦
- `navigate --cort` 命中時帶 cort `content` 摘要進路線（省一次 read）— ✅ 已做：
  路線加「內文摘要（省一次 read）」步驟（壓縮空白、截 220 字），
  `content_summary()` + 命中 route 帶上；實測 `with_readonly` 路線直接含函式簽名
- FTS 全文檢索 fallback（`chunks_fts`，與 `cort context` 的 fts resolution 同源）— ✅ 已做：
  symbol_name 未命中時查 `chunks_fts MATCH <"token1" AND "token2">`（token 加雙引號防
  FTS 運算子注入），JOIN `chunks` 還原 CortHit，路線標示「cort FTS 全文命中」；
  實測 `immutable`（content-only）4 命中、`"immutable" AND "fallback"` 也通
- 附帶修正：cort 命中合併後重算 exact（子字串 token 命中不再壓過精確命中）並重新排序，
  路線優先指向精確符號（`with_readonly` 優先於 `render_with_profile`）
- STALE 提示 `cort index --incremental` — ✅ 已隨新版 cort 解決（`cort status` 提供
  `index_is_stale`；`hook-refresh` 編輯後自動增量，不需 claudecat 再提示）
