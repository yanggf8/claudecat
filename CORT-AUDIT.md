## 長期指標 (claudecat cort-audit)

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
## 每日分析發現 (claudecat cort-audit)

_2026-09-22_

- deep/30d 持平在 12，但 deep/7d 連五天掛零（09-18 起），30 天的量全落在窗口外半部；再無 deep 使用的話，30d 指標會隨滾動窗口開始掉，adoption 面是未來幾天要盯的數字。
- 可動作標籤今日仍從缺，不開新規則：pattern_not_symbol 已裁決、剝皮規則觀察中不重開；concrete_file_read 是精確度閘門、unindexed_extension 是正確沉默、unparseable_command 是 parser 產品問題；唯一候選 context_flag 僅 7 筆（<10），誠實等樣本。
- kimi 命中率約 0.98%（分子不動、分母膨脹），剝皮規則觀察期繼續、不下結論；其餘 harness 皆低於 1%（unspecified 高命中是手動探針，不計）；grok 宣告值與實測千筆不符，歸因仍不可信。
- top no_shape shapes 覆蓋約兩成，全是 Bash／Grep 的 envelope 鍵形狀、尚無語義形狀訊號——issue #3 的需求排序暫無新依據。
- 數據品質全綠：兩 hook census 加總等於 fires、無 unknown 桶、FTS synced 且 fresh，repair=none 是自養預期不是無 staleness；真缺口為零（未 chunk 檔全是 chunk_count=0 的正確沉默）；self-heal 採樣仍零樣本，legacy 屬舊列預期、不是零自癒。
- 上游 origin/master 超前本地十個 commit（upgrade／install／deps 與 gate-audit／codegraph 文檔），無 hook 規則變動、口徑不變，下次方便時 pull 即可。

_2026-09-21 — i51149R3050_

- deep/30d=2 持平、deep7 連續歸零：過去 7 天零 context／recall 深水區使用，30 天內的 2 次深挖全落在 7 天窗口外；adoption 缺口延續，該做是維持 navigate 當 front door，下週再看 deep7 是否回升（各機 DB 獨立，不可拿 NUC11i5 的 12 來比；30 天滾動窗口會自然吞掉舊深挖）。
- 不開新規則：pattern_not_symbol=300 已裁決、剝皮規則觀察期中（不重開）；unparseable_command=39 是 parser 產品問題、concrete_file_read=20 開火只會是精確度噪音（probe 樣本多為 concrete file+bare symbol 伴隨查詢）、unindexed_extension／non_source_target 各 8 屬正確沉默；context_flag=2（<10），誠實等樣本。
- hook-suggest 命中 5/2567（<1%）；top no_shape shapes 覆蓋 32.9% 但仍全是 Bash／Grep envelope 鍵形狀、無語義形狀訊號——issue #3 的需求排序暫無新依據；grok 17 筆、codex 3 筆 harness_declared 與實測不符，歸因改用實測值。
- 數據品質全綠：兩 hook census 加總＝fires、無 unknown/ 桶、FTS synced、fresh、repair=none（查詢自癒常態下 repair=none 更常見，不得誤讀為無 staleness）、真缺口 0（5 未 chunk 檔全是 chunk_count=0 的正確沉默）；self-heal 仍零真實樣本（scanned=14 全是 legacy 舊列，屬預期）。
- 上游已同步到 e4e97bc6（HEAD..origin/master 無落後，自 09-19 起無新 commit）；跨機列不可互比（本機 hostname 即 i51149R3050）。

_2026-09-18 — MacBook-Pro.local_

- 本機（Mac，第四台）首次入表：fresh、686 chunks / 451 relationships、真缺口 0、FTS synced；30d 命令 3503、deep 1——深挖歸零與 Thinkpade15 首列同構，adoption 缺口在 Mac 上同樣鮮明。
- audit 列的 host 探測（cort.rs `host_name`）是 doctor 那個 `/etc/hostname` bug 的第二份拷貝：本機第一跑寫出 `unknown | no-index` 列。已修——刪拷貝、改呼叫 `doctor::resolve_host` 單一家（2026-09-18 修 doctor 時沒對 `resolve_host` 跑 impact，漏了這個呼叫點；「還有誰在用」正是該跑 caller-set 的場景）。
- 同步跟上 d3f9ac9（+643：markdown sidecar、usage 指標、v2 skill 進 repo）：重裝 binary 後 `usage` 子命令才出現——版號凍在 2.1.0 不動，判斷新舊要比對子命令面而非版號；skill symlink 補上 `~/.claude/skills/claudecat`。
- codex 558 筆 harness_declared 不符、claude-code 命中率 0.11%（1/916）——與 NUC11i5／Thinkpade15 同構；本機 fires 樣本尚小（1,585），只記不裁決。本機 `findings_update` 的「整段取代、只留最新一天」契約會蓋掉他機同日區塊，故本列採手動附加、保留他機內容。
