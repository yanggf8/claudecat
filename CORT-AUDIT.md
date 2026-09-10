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

- 真缺口連續第 2 天 = 0；#5 的漂移（src/claude_md.rs）在 commit 後自癒，indexed_uncommitted 警示消失。這條指標現在是乾淨基線，出現非零再查。
- **Grep shape 翻案**：24 筆 Grep 形狀的 pattern_not_symbol **全部來自 kimi-code**，不是 claude-code——而 kimi 是 30d 命中率最高（3.77%，約 claude-code 25 倍）的 harness，規則若開，收益落在最有反應的流量上。
- pattern 本身無法採樣：args_summary 不記 pattern（09-09 已記明），補採 kimi transcript 需要使用者授權（敏感 session 記錄，權限分類器擋下）。
- 儘管如此傾向**開保守放行規則**：不是「猜那 23 筆是不是 symbol」，而是「把可確定是 symbol 的形狀放行」——pattern 去掉 \b／引號／錨點後是 bare identifier 才開火，誤報風險由規則形狀本身壓住；樣本 23 過了 ≥10 門檻。
- 驗收線掛 issue #3 的命中率 ≥5%：看 kimi 的 3.77% 開規則後是否上移；`context_flag` 樣本僅 7，繼續等。
- deep30=3／deep7=2，與基線持平；FTS synced、graph_pending=0、schema=7，無數據品質訊號。
