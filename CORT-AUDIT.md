## 長期指標 (claudecat cort-audit)

> **口徑註記（2026-09-27 起，by design）**：上游 cort 的 hook 已靜默化為純記錄器（cortexyoung `428e3398`）——fires／outcome census／decline 分佈照常且更乾淨（無注入干擾），但注入與採納類指標自此恆零；同日 parser 放行 `cd && grep`／`; grep` 人口（`eab18be9`），searches 口徑變大、`not_a_search_tool` 的 decline 份額明顯下降，跨 09-27 前後的絕對數不可直接比。

| 日期 | 專案 | host | fresh | chunks | relationships | 未chunk檔 | 真缺口 | FTS drift | 命令數/30d | core/30d | deep/30d | 命令數/7d | deep/7d | decline-top |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|
| 2026-09-06 | `/home/yanggf/a/claudecat` | fresh | 586 | 310 | 5 | ? | synced | 12051 | 473 | 3 | 12035 | 2 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 605 | 341 | 5 | ? | synced | 13413 | 473 | 3 | 13397 | 2 | not_a_search_tool=165 |
| 2026-09-07 | `/home/yanggf/a/claudecat` | i51149R3050 | fresh | 602 | 337 | 5 | ? | synced | 859 | 16 | 2 | 828 | 0 | unparseable_command=28 |
| 2026-09-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 609 | 345 | 5 | ? | synced | 16152 | 473 | 3 | 16128 | 2 | pattern_not_symbol=213 |
| 2026-09-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 625 | 369 | 6 | ? | synced | 18298 | 474 | 3 | 17283 | 2 | pattern_not_symbol=324 |
| 2026-09-10 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 639 | 395 | 5 | 0 | synced | 19939 | 474 | 3 | 16301 | 2 | pattern_not_symbol=430 |
| 2026-09-11 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 24101 | 545 | 12 | 17250 | 11 | pattern_not_symbol=614 |
| 2026-09-12 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 26022 | 546 | 12 | 15531 | 11 | pattern_not_symbol=657 |
| 2026-09-13 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 27885 | 546 | 12 | 17310 | 11 | pattern_not_symbol=737 |
| 2026-09-14 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 645 | 401 | 5 | 0 | synced | 28407 | 546 | 12 | 14994 | 9 | pattern_not_symbol=773 |
| 2026-09-15 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 647 | 405 | 5 | 0 | synced | 30750 | 552 | 12 | 14597 | 9 | pattern_not_symbol=939 |
| 2026-09-16 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 647 | 405 | 5 | 0 | synced | 34896 | 552 | 12 | 16598 | 9 | pattern_not_symbol=1135 |
| 2026-09-17 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 647 | 405 | 5 | 0 | synced | 39070 | 552 | 12 | 19354 | 9 | pattern_not_symbol=1198 |
| 2026-09-18 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 647 | 405 | 5 | 0 | synced | 41091 | 552 | 12 | 16990 | 0 | pattern_not_symbol=1248 |
| 2026-09-18 | `/home/yanggf/a/claudecat` | Thinkpade15 | fresh | 663 | 422 | 5 | 0 | synced | 6947 | 14 | 0 | 5112 | 0 | pattern_not_symbol=189 |
| 2026-09-18 | `/Users/guofang.mis/a/claudecat` | MacBook-Pro.local | fresh | 686 | 451 | 5 | 0 | synced | 3503 | 8 | 1 | 2563 | 0 | pattern_not_symbol=249 |
| 2026-09-19 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 685 | 450 | 5 | 0 | synced | 44458 | 552 | 12 | 19698 | 0 | pattern_not_symbol=1425 |
| 2026-09-19 | `/home/yanggf/a/claudecat` | i51149R3050 | fresh | 687 | 451 | 5 | 0 | synced | 3703 | 16 | 2 | 2287 | 0 | pattern_not_symbol=231 |
| 2026-09-20 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 685 | 450 | 5 | 0 | synced | 48677 | 552 | 12 | 20792 | 0 | pattern_not_symbol=1631 |
| 2026-09-21 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 685 | 450 | 5 | 0 | synced | 50950 | 552 | 12 | 22543 | 0 | pattern_not_symbol=1741 |
| 2026-09-21 | `/home/yanggf/a/claudecat` | i51149R3050 | fresh | 687 | 451 | 5 | 0 | synced | 5064 | 17 | 2 | 3198 | 0 | pattern_not_symbol=300 |
| 2026-09-22 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 685 | 450 | 5 | 0 | synced | 53651 | 552 | 12 | 22901 | 0 | pattern_not_symbol=1848 |
| 2026-09-23 | `/home/yanggf/a/claudecat` | NUC11i5 | STALE/refresh | 687 | 451 | 5 | 0 | synced | 57326 | 552 | 12 | 22423 | 0 | pattern_not_symbol=2108 |
| 2026-09-24 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 61124 | 552 | 12 | 22054 | 0 | pattern_not_symbol=2255 |
| 2026-09-25 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 63316 | 552 | 12 | 22225 | 0 | pattern_not_symbol=2375 |
| 2026-09-26 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 64411 | 552 | 12 | 19958 | 0 | pattern_not_symbol=2401 |
| 2026-09-27 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 65040 | 552 | 12 | 16363 | 0 | pattern_not_symbol=2404 |
| 2026-09-28 | `/home/yanggf/a/claudecat` | i51149R3050 | STALE | 686 | 451 | 5 | 0 | synced | 10092 | 23 | 2 | 5025 | 0 | pattern_not_symbol=679 |
| 2026-09-28 | `/home/yanggf/a/claudecat` | NUC11i5 | STALE | 686 | 451 | 5 | 0 | synced | 69004 | 552 | 11 | 18064 | 0 | pattern_not_symbol=2647 |
| 2026-09-29 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 76605 | 552 | 11 | 22970 | 0 | pattern_not_symbol=3662 |
| 2026-09-30 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 81133 | 552 | 11 | 23919 | 0 | pattern_not_symbol=4365 |
| 2026-10-01 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 86782 | 551 | 11 | 25616 | 0 | pattern_not_symbol=5010 |
| 2026-10-02 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 91863 | 493 | 11 | 29562 | 0 | pattern_not_symbol=5804 |
| 2026-10-03 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 94917 | 482 | 11 | 34038 | 0 | pattern_not_symbol=6566 |
| 2026-10-04 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 94122 | 475 | 11 | 35933 | 0 | pattern_not_symbol=7032 |
| 2026-10-05 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 92402 | 87 | 11 | 33364 | 0 | pattern_not_symbol=7276 |
| 2026-10-06 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 92918 | 87 | 11 | 26820 | 0 | pattern_not_symbol=7480 |
| 2026-10-07 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 93732 | 84 | 9 | 25996 | 0 | pattern_not_symbol=8214 |
| 2026-10-08 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 95335 | 84 | 9 | 24781 | 0 | pattern_not_symbol=8982 |
| 2026-10-09 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 98389 | 84 | 9 | 23809 | 0 | pattern_not_symbol=9723 |
| 2026-10-10 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 101937 | 84 | 9 | 23204 | 0 | pattern_not_symbol=10440 |
| 2026-10-11 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 100790 | 13 | 0 | 23918 | 0 | pattern_not_symbol=10843 |
| 2026-10-12 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 101738 | 13 | 0 | 24120 | 0 | pattern_not_symbol=11080 |
| 2026-10-13 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 103777 | 12 | 0 | 28172 | 0 | pattern_not_symbol=11928 |
| 2026-10-14 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 112294 | 12 | 0 | 33556 | 0 | pattern_not_symbol=13263 |
| 2026-10-15 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 114794 | 6 | 0 | 34057 | 0 | pattern_not_symbol=13662 |
| 2026-10-16 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 117611 | 6 | 0 | 35820 | 0 | pattern_not_symbol=14458 |
| 2026-10-17 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 118872 | 6 | 0 | 36289 | 0 | pattern_not_symbol=15150 |
| 2026-10-18 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 686 | 451 | 5 | 0 | synced | 119278 | 6 | 0 | 35478 | 0 | pattern_not_symbol=15468 |
| 2026-10-19 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 786 | 704 | 5 | 0 | synced | 117941 | 6 | 0 | 35901 | 0 | pattern_not_symbol=15524 |
| 2026-10-20 | `/home/yanggf/a/claudecat` | NUC11i5 | STALE/refresh | 786 | 704 | 5 | 0 | synced | 115093 | 6 | 0 | 31964 | 0 | pattern_not_symbol=15520 |
| 2026-10-21 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 785 | 700 | 5 | 0 | synced | 114501 | 6 | 0 | 24750 | 0 | pattern_not_symbol=15697 |
| 2026-10-22 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 785 | 700 | 5 | 0 | synced | 117699 | 6 | 0 | 25806 | 0 | pattern_not_symbol=16346 |
| 2026-10-23 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 748 | 545 | 5 | 0 | synced | 126382 | 15 | 0 | 31105 | 0 | pattern_not_symbol=17841 |
| 2026-10-24 | `/home/yanggf/a/claudecat` | NUC11i5 | fresh | 748 | 545 | 5 | 0 | synced | 129604 | 16 | 0 | 32786 | 0 | pattern_not_symbol=18672 |
## 每日分析發現 (claudecat cort-audit)

_2026-10-24_

- 量續升、昨日「窗口滾動」結論不變：命令數/30d 與/7d 連兩日回升（各 +3,198、+1,681 口徑量級）；core/30d 15→16（impact 多一筆）；deep/30d 歸零第十四天、deep/7d 續零——但上游已移除 context／struct／read／recall 動詞，deep 歸零是口徑死亡不是 adoption 下滑，往後 adoption 改盯 core／impact 與 measure 新口徑，不再拿 deep 基線（09-06 deep30=3）說事。
- 可動作標籤仍從缺，不開新規則：context_flag 在全母體 census 缺席（零樣本，<10 誠實等）；pattern_not_symbol 雖續增但 09-11 已裁決不再重開；其餘 unindexed_extension／concrete_file_read／target_not_source／non_source_target 分屬精確度閘門／正確沉默；no_shape 覆蓋百分之百但前五仍全是 Bash／Grep envelope 形狀——issue #3 暫無新依據。
- 剝皮規則觀察期繼續、不下結論：命中 22 筆（+2）全是 claude-code（0.04%），遠低於 5% 驗收線；codex／kimi 依然零命中；harness_declared 不符 claude-code 一筆、grok 二十七筆持平，歸因一律用實測值。
- 數據品質皆好、errors 已查根因：census 加總＝fires、無 unknown 桶、fresh、FTS synced、真缺口為零（五檔未 chunk 全是 chunk_count=0 的正確沉默）；errors=37 全落在 10-22／23——三十五筆 measure（error_code=measure_error）加兩筆 memory，是本機 measure 分支開發試驗的殘留，非 hook／index 品質事件，不當事件辦；index_stale 查詢持平於三。
- self-heal 無新訊號：scanned=16／healed=1／deferred=2／legacy=13——legacy 膨脹是三十天窗口納入 heal 欄位之前的舊 impact 列所致，與「零自癒」是兩回事；healed 與 deferred 皆無新增。
- 上游無需動作：fetch 後 HEAD..origin/master 無落後；本地 measure 分支（f99ef6c8）已大幅超前任務追蹤點 5f6d5267，多為 measure／docs 口徑、無 hook census 變動，下次更新追蹤點即可。
