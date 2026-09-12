## 長期指標 (claudecat cort-audit)

| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | 真缺口 | FTS drift | 命令數/30d | core/30d | deep/30d | 命令數/7d | deep/7d | decline-top |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2026-09-06 | `/home/yanggf/a/claudecat` | fresh | 586 | 310 | 5 | ? | synced | 12051 | 473 | 3 | 12035 | 2 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 605 | 341 | 5 | ? | synced | 13413 | 473 | 3 | 13397 | 2 | not_a_search_tool=165 |
| 2026-09-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 609 | 345 | 5 | ? | synced | 16152 | 473 | 3 | 16128 | 2 | pattern_not_symbol=213 |
| 2026-09-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 625 | 369 | 6 | ? | synced | 18298 | 474 | 3 | 17283 | 2 | pattern_not_symbol=324 |
| 2026-09-10 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 639 | 395 | 5 | 0 | synced | 19939 | 474 | 3 | 16301 | 2 | pattern_not_symbol=430 |
| 2026-09-11 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 24101 | 545 | 12 | 17250 | 11 | pattern_not_symbol=614 |
| 2026-09-12 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 26022 | 546 | 12 | 15531 | 11 | pattern_not_symbol=657 |
## 每日分析發現 (claudecat cort-audit)

_2026-09-12_

- deep30=12／deep7=11 連續第 3 天持平（基線 3／2）——09-11 的跳變確認是真實新增的深水區使用，不是 30 天窗口效應；但 recall 30 天僅 1 次，深挖幾乎全靠 context，召回側（recall/impact 路線）是下一個 adoption 缺口。
- 可動作標籤今日仍從缺，不開新規則：pattern_not_symbol（已裁決、觀察中）、concrete_file_read=47（精確度閘門）、unparseable_command=30（parser 產品問題，非 hook 規則可解）、context_flag 仍 7（<10 誠實等樣本）。
- **hook_row v4 上線即兌現**：今晚已有 2 筆 no_evidence 帶 `symbol`/`why`（皆 `absent`）——00:26 是開發 session 對 cortexyoung 的人工驗證；00:56 是本分析自己的 grep：bare identifier 經 9e725c76 剝皮規則放行→probe→no_evidence，`project_id` 正確歸到 claudecat。漏斗端到端可用，refusals 開始變成可追的工作清單。
- 觀察效應備案：audit 自己的採樣（grep 符號名）會生成 probe-paid 列，與 `unspecified` 手動探針同類的汙染源。v4 列目前個位數不影響任何排序，但未來按 symbol/why 歸因時要先扣掉 audit 自身流量。
- kimi 命中率 2.84%（4/141）仍居冠（grok 0.88%、codex 0.49%、claude-code 0.12%），剝皮規則觀察期繼續，單日小分母不下結論；7d 命令數 15531 較前日下移，與 09-12 排程沒跑（機器沒開）一致，非使用衰退訊號。
- 數據品質全綠：兩 hook census 加總＝fires、無 unknown/ 桶、FTS synced、真缺口 0、repair=none。09-12 兩個排程缺席與補測列的 UTC 日期口徑已記在 CORT-INTEGRATION.md 同日節。
