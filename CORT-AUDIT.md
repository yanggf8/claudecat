## 長期指標 (claudecat cort-audit)

| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | 真缺口 | FTS drift | 命令數/30d | core/30d | deep/30d | 命令數/7d | deep/7d | decline-top |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2026-09-06 | `/home/yanggf/a/claudecat` | fresh | 586 | 310 | 5 | ? | synced | 12051 | 473 | 3 | 12035 | 2 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 605 | 341 | 5 | ? | synced | 13413 | 473 | 3 | 13397 | 2 | not_a_search_tool=165 |
| 2026-09-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 609 | 345 | 5 | ? | synced | 16152 | 473 | 3 | 16128 | 2 | pattern_not_symbol=213 |
| 2026-09-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 625 | 369 | 6 | ? | synced | 18298 | 474 | 3 | 17283 | 2 | pattern_not_symbol=324 |
| 2026-09-10 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 639 | 395 | 5 | 0 | synced | 19939 | 474 | 3 | 16301 | 2 | pattern_not_symbol=430 |
## 每日分析發現 (claudecat cort-audit)

_2026-09-10_

- deep 指標持平：deep/30d=3、deep/7d=2，自 09-06 基線連 5 天無變化；命令數 30d 漲、7d 跌是滾動窗口效應（近期單日量低於剛滾出 7d 窗口的高流量日），解讀看成分而非總數。
- pattern_not_symbol 續為最大可調標籤，今日約增百筆、先前下降趨勢中斷；但按政策先採樣再決定，不直接開規則——shape 指向新版含 model+turn_id 的 Bash hook 事件為最大可行動沉默來源，列為 hook-probe 採樣第一順位（shape 覆蓋率僅約兩成，解讀保留）。
- unparseable_command 連續 3 天零新增，確認是歷史包袱，移出候選，不為它開規則。
- context_flag 未達開規則門檻且自 09-07 零新增，不開；concrete_file_read、unindexed_extension、non_source_target 量小且形狀明確，暫不開。
- 數據品質乾淨：真缺口首次歸零（之前是問號）；未chunk 5 檔全是 legacy JS 的正確沉默；indexed_uncommitted 無漂移；origin/master 無新提交。今日結論：不開新規則。
