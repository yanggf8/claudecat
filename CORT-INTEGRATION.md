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
- cort 索引 STALE → `cort-status` 明確提示 `cort index`，不假裝新鮮
- cort DB 不存在 / 未命中 → `navigate` 自動回退 tree-sitter 地圖（並提示）

## 實測（2026-09-05，cortexyoung 真實索引）
- `cort-status`：chunks=1150 relationships=2034，STALE（索引 HEAD 4042a7a6 ≠ 目前 7c2ad21d）✅ 誠實
- `navigate --cort "staleness"`：找到 tree-sitter top-30 找不到的符號
  （`rust/tests/staleness_cwd.rs:72`），路線含 cort context / impact / 反向依賴 ✅

## 待辦
- `navigate --cort` 命中時把 cort 的 `content` 摘要帶進路線（省一次 read）
- FTS 全文檢索 fallback（`cort recall` 對應）
- `cort-status` 的 STALE 時可提示 `cort index --incremental`（而非全量）
