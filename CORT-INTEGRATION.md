# cortexyoung/cort 整合（2026-09-05）

## 問題（使用者的提問）
「結合使用時，能利用 cortexyoung 的索引或其他資源嗎？」

## 答案
**能。** claudecat 直接唯讀 cort 的 SQLite 索引（不重造、不改寫），
用 cort 做「精準定位層」、claudecat 做「地圖與路線層」。

## cort 提供什麼（schema v5）
| 表 | 內容 | claudecat 用途 |
|---|---|---|
| `projects` | project_id / name / path / git_head / last_indexed_at / extractor_version | `cort-status` 新鮮度 |
| `chunks` | file_path / symbol_name / chunk_type / start_line / end_line / content / language | `navigate --cort` 全量符號查詢 |
| `relationships` | source→target 邊（imports/exports/calls/**references**）+ call_site_line + call_form + confidence | 反向依賴路線（`dependents()` 不過濾 rel_type，references 邊自動吃得到） |
| `chunks_fts` | FTS5 external-content（content/symbol/file，tokenize unicode61） | `navigate --cort` 全文 fallback（symbol 未命中時） |
| `_cortex_meta` | key/value：`SCHEMA_VERSION` / `graph_pending` / `extractor_version` | `cort-status` / `cort-audit` 的圖重建狀態（見 2026-09-09 一節） |

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
- 當時的索引仍為 v4 schema（`projects/chunks/relationships/chunks_fts` 欄位對 claudecat 零影響），
  `extractor_version` 同前一版；新增 `usage.db`（command_log）與 claudecat 無關
  （**後續**：cort 已於 2026-09-04 `ab1da4f4` 進到 v5，見 2026-09-09 一節）
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

## 跟上 cortexyoung（2026-09-09；對照 cort `f61ecd00`）

上游自 2026-09-04 起的四項改動，逐條實測後只有一項要動程式：

- **schema v5**（`ab1da4f4`）：只是加寬 `relationships`/`raw_edges` 的 CHECK（新增
  `references` 邊、Rust type 存成 `chunk:class`），claudecat 讀的欄位全在，
  `cort-status` 實跑正常。文件與 `src/cort.rs` 的「v4」字樣一併更新。
- **`graph_pending`（要動程式，P1）**：cort 的 schema 遷移在 `db.rs:322` 設
  `graph_pending=1` 卻**不動** `git_head`/`last_indexed_at`；而增量索引雖然每個檔案都會先設 1，
  卻是在 `incremental.rs` **與時戳更新同一個 transaction** 裡清 0。
  所以外部讀到持續 `1` 只有兩種情況：①遷移後還沒跑過索引（時戳仍新 → claudecat 舊行為會
  **誤報 fresh**，而 relationships 是升級前的舊邊）②增量中斷（時戳沒前進，本來就 STALE）。
  修法（與 09-06 的毫秒事件同一原則：不假裝健康、也不假裝壞掉）：
  `index_info` / `audit_index` 同一口徑讀 `_cortex_meta`，
  `fresh = HEAD+age && graph_pending != Some(true)`；讀不到（舊版 DB／查詢失敗）→ `None`，
  **不翻布林**，另給一條「無法判讀」提示。日表不加欄，改讓既有 `fresh` 格分成
  `fresh` / `fresh?`（圖狀態未知）/ `STALE` / `STALE/graph`。三支回歸測試。
  claudecat **不硬編碼**期望的 SCHEMA_VERSION——那是 cort 自己的常數，寫死必然像文件一樣爛掉。
- **Java / AngularJS 1.x / HTML 索引**（`7227fd96`）：cort 端能力，claudecat 唯讀照吃
  （`navigate --cort` 對 Java 專案反而更有價值）。claudecat 自己的 tree-sitter 不跟進加
  `tree-sitter-java`：那是跟索引搶同一份工作。實際做的兩件小事：①`walk.rs` 第三欄語意
  正名為 `is_code`（只進 LOC/樹/key_files，不代表本地有 grammar；把 java 改成 false 會讓
  Java 專案整個從地圖消失，比「檔案在、符號空」更騙）②outline 對沒有本地 grammar 的
  key file 補一句「無本地 AST → 走 `navigate --cort`」（`symbols::has_grammar`，附測試；
  純 Rust 專案不出現這句）。`html/htm` 不進 `CODE_EXT`：只會灌 total_files/languages，
  進不了 key_files，也拿不到符號。
- **安裝／更新路徑改版**（`26fd3155`…`f61ecd00`，新 `cort-upgrade` 為正式更新路徑）：
  對 claudecat 零影響。`usage.db` 因此多了 `internal-shim` / `internal-ast-grep`，
  只灌 `total_commands`，**不進** hook-suggest 命中率分母（那是
  `max(by_command["hook-suggest"], Σoutcomes)`），`core`/`deep` 時間序列照舊可比 →
  日表分母**不動**（改了 09-06～09-08 的列就不可比，本檔已經有過一次不可比）。

## cortexyoung#3 複量（2026-09-09）：83% no_shape 不是一根槓桿

09-06 把 `no_shape` 5857（83%）記成「最大槓桿」。歸因上線後重量，**這個結論翻轉**——
資料源同前（唯讀 `~/.cache/cortex-ng/usage.db`，機器 `NUC11i5`，對照 cort `f61ecd00`）：

- **歸因這一半上游已做完**：`c290c383` 之後，2026-09-07 00:00Z 起的 `no_shape`
  **100% 帶 decline tag**（09-07 1130、09-08 839，untagged 0）；6447 筆沒 tag 的全部早於 09-07，
  是歷史列不是缺口。**這也是 claudecat 日表 `decline-top` 欄能開始有值的原因。**
- **全量標記窗口（09-07→09-09，hook-suggest 1989 筆／`no_shape` 1975）的拆解**：
  `not_a_search_tool` 1650（84%）、`pattern_not_symbol` 277（14%）——兩者都是設計要的沉默
  （`hook.rs` 的 `judge()` 只在「project source 裡的單一 bare symbol」開火）；
  其餘 `concrete_file_read` 21 / `unindexed_extension` 14 / `non_source_target` 10 /
  `target_not_source` 3 —— **可調表面 48 筆（2.4%）**，不是 83%。
- **命中率沒動**：該窗口 3 命中；30d 39/9849＝0.40%。歸因給出的答案是
  「流量本來就不是 symbol 形狀」，不是「規則太嚴」。
- **誠實限制**：`args_summary` 只記 decline tag、**不記 pattern**，所以能證明「哪條規則擋的」，
  不能證明那 277 筆 `pattern_not_symbol` 每筆都真的不是 symbol 查詢
  （`\bfoo\b`、引號、單項 alternation 都會落進同一格）。要回答得記 pattern 的**形狀類別**
  （不是 pattern 本身）。這是目前唯一可能還藏著靶心的地方。
- **沒人用過的維度**：payload 已是 v3，帶 `harness`——claude-code 1761／codex 217／kimi-code 11，
  **3 個命中全在 claude-code**；`pattern_not_symbol` 佔比 claude-code 14%、codex 11%（形狀問題看來與
  harness 無關，命中卻不是）。claudecat 的 `cort-audit` 已加上這個切面（每 harness：hook-suggest 數／命中／命中率／
  no_shape／top decline／refresh），並把兩件會說謊的事做成明文：①沒有 `harness` 欄的
  v3 前歷史列**另計**（不攤進任一 harness，否則加總悄悄對不上）②`harness_declared` 與實測
  不符的列數要看得見（實測 grok 481 筆宣告成 claude-code——按宣告值分群的歸因會被汙染）。
  30d 實測：claude-code 6604 筆 0.15%、codex 1435 筆 0.63%、grok 218 筆 0.92%、
  **kimi-code 106 筆 3.77%**（差 25 倍）——總命中率 0.4% 看不出這件事。
  切面只進報告、**不進日表**（加欄會斷掉跨日可比性）。

三個 issue 的現況（同日一併複量，已回貼 issue）：#3 見上；
**#4** 30d 17788 筆命令、`saved_bytes>0` 僅 1 筆 13 bytes（比 09-06 多量 6.3k 筆命令，結論不變，
不是短窗抽樣）；**#2** 上游 `rust/src` 未見對應變動（`coverage.rs` 的 `unindexed`/`scan_skipped`
是 recall 側另一張螢幕），claudecat 端仍是同樣 5 檔 `legacy/test-*.js`，5/5 是無宣告的 driver script。
