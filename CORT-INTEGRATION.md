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
| `chunks_fts` | FTS5（content/symbol/file） | （未來）`navigate` 全文檢索 |

- DB 路徑：`$CORT_CACHE_DIR/<sha256>.db`，`project_id = sha256(real_path)`（0.40 rusqlite 唯讀開啟）
- cort 的 `last_indexed_at` 是 **epoch 毫秒**（13 位數）；freshness 相容秒/毫秒

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

## 待辦
- `navigate --cort` 命中時把 cort 的 `content` 摘要帶進路線（省一次 read）— 未做
- FTS 全文檢索 fallback（`chunks_fts` / `cort recall` 對應）— 未做
- STALE 提示 `cort index --incremental` — ✅ 已隨新版 cort 解決（`cort status` 提供
  `index_is_stale`；`hook-refresh` 編輯後自動增量，不需 claudecat 再提示）
