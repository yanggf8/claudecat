## 長期指標 (claudecat cort-audit)

| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | 真缺口 | FTS drift | 命令數/30d | core/30d | deep/30d | 命令數/7d | deep/7d | decline-top |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2026-09-06 | `/home/yanggf/a/claudecat` | fresh | 586 | 310 | 5 | ? | synced | 12051 | 473 | 3 | 12035 | 2 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 605 | 341 | 5 | ? | synced | 13413 | 473 | 3 | 13397 | 2 | not_a_search_tool=165 |
| 2026-09-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 609 | 345 | 5 | ? | synced | 16152 | 473 | 3 | 16128 | 2 | pattern_not_symbol=213 |
| 2026-09-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 625 | 369 | 6 | ? | synced | 18298 | 474 | 3 | 17283 | 2 | pattern_not_symbol=324 |
| 2026-09-10 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 639 | 395 | 5 | 0 | synced | 19939 | 474 | 3 | 16301 | 2 | pattern_not_symbol=430 |
| 2026-09-11 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 24101 | 545 | 12 | 17250 | 11 | pattern_not_symbol=614 |
## 每日分析發現 (claudecat cort-audit)

_2026-09-11_

- deep30=12／deep7=11，與昨日持平（基線 3／2；09-11 的跳變後維持，深水區使用是真實增加，不是 30 天窗口效應）。
- decline 排序：pattern_not_symbol=619（日增僅 +5）仍最大但已裁決、進觀察期；次大 concrete_file_read=43 經 hook-probe＋hook.rs 註解確認是刻意留的精確度閘門（命名具體單檔＝reading 而非 caller-set），不可動作。
- 可動作標籤今日從缺：context_flag 仍 7（<10 繼續等）；不開新規則，誠實等樣本。
- 驗收線觀察：kimi 命中率 3.03%（4/132），較昨日 3.77% 下移但仍居冠（grok 0.92%、codex 0.50%、claude-code 0.12%）；9e725c76 已在本地 HEAD——剝皮規則生效中，觀察期繼續，單日小分母波動不下結論。
- 數據品質全綠：兩表加總＝fires、無 unknown 桶、FTS synced、真缺口連續第 3 天 0；unparseable 近 10 天僅 09-06 殘留 30 筆、之後零新增，歷史列假設成立。
- indexed_uncommitted=3 檔是工作樹未提交編輯的預期現象（6 檔 M），commit 後自癒；上游 fetch 後無新 commit（HEAD=f8d3d3f4）。
