## 長期指標 (claudecat cort-audit)

| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | FTS drift | 命令數/30d | core/30d | deep/30d | 命令數/7d | deep/7d | decline-top |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2026-09-06 | `/home/yanggf/a/claudecat` | fresh | 586 | 310 | 5 | synced | 12051 | 473 | 3 | 12035 | 2 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 605 | 341 | 5 | synced | 13413 | 473 | 3 | 13397 | 2 | not_a_search_tool=165 |
| 2026-09-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 609 | 345 | 5 | synced | 16152 | 473 | 3 | 16128 | 2 | pattern_not_symbol=213 |
| 2026-09-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 625 | 369 | 6 | synced | 18298 | 474 | 3 | 17283 | 2 | pattern_not_symbol=324 |

## 每日分析發現 (claudecat cort-audit)

_2026-09-09_

- deep 指標持平：deep/30d=3、deep/7d=2，自 09-06 基線無變化；命令數/30d 爬升是使用量成長，30 天滾動窗口解讀要看成分而非總數。
- context_flag 前提已過時：7 筆全在 09-07，當天 aa4e71ac 已讓 context 意圖改走 `cort context` 而非沉默，今日零新增，不為它開規則。
- pattern_not_symbol 仍是最大可調標籤（累計 326，單日 206→73→47 下降中），但按政策先採樣再決定；usage.db 只存標籤不存 pattern，採樣走 hook-probe 語料，列為下一步。
- unparseable_command 30 筆全在 09-07、09-08 起歸零，疑似解析修復生效或流量結構改變，先觀察一天，持續為零就移出候選。
- concrete_file_read（24）、unindexed_extension（16）等形狀明確且量小（單檔閱讀、未索引語言），目前不值得單獨開規則。
- 數據品質無異常：fresh、FTS synced、decline 欄完整，binary 今晨已更新且其後僅 docs 提交；origin/master 無新 commit。今日結論：不開新規則。
