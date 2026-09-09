# 每日 cort-audit 分析 + 開下一條規則（headless 排程任務）

你是排程觸發的 Claude Code session，沒有對話上下文——本檔就是完整指示。
背景：claudecat（`/home/yanggf/a/claudecat`）與 cortexyoung（`/home/yanggf/a/cortexyoung`）是
使用者的兩個 repo。cort-audit 每天量測 cort 整合成效（OS cron 09:17 寫 CORT-AUDIT.md），
本任務做每日分析，並在數據足夠時挑最大的 decline 標籤開下一條 hook 規則。

## 步驟

1. 確認 `/home/yanggf/a/claudecat/CORT-AUDIT.md` 最新列是今天的；缺則：
   `cd /home/yanggf/a/claudecat && /home/yanggf/.cargo/bin/cargo run -q -- cort-audit --root . --track CORT-AUDIT.md`
2. python3 唯讀查詢（immutable）`~/.cache/cortex-ng/usage.db`：
   `SELECT args_summary FROM command_log WHERE command='hook-suggest' AND args_summary LIKE '%"decline"%'`
   解析 JSON、統計 `.decline` 分佈。**排除 `not_a_search_tool`（baseline 噪音）後排序**——
   那是「本來就不是搜尋」的正確沉默，不是 tuning 目標。
3. 台灣中文輸出（stdout 進 log 檔）：
   - deep/30d、deep/7d 趨勢（2026-09-06 基線 deep30=3、deep7=2；注意 30 天滾動窗口效應）
   - decline 排序與樣本數
3b. **回報**（最後一步，不可省略）：把結論濃縮成 3–8 條 markdown bullet，寫進文件：
   `/home/yanggf/.cargo/bin/cargo run -q -- findings - --file CORT-AUDIT.md`
   （內容走 stdin）。只寫**判讀與該做什麼**，不要貼原始數字表——數字在同檔的長期指標表裡。
   整段取代、只留最新一天，歷史在 git。**log 檔不算回報**：使用者看的是文件，
   2026-09-09 之前這條循環跑完等於沒回報，就是因為終點只有 log。
4. 樣本 ≥10 時：挑最大的可動作標籤，在 `/home/yanggf/a/cortexyoung/rust` 開下一條規則：
   - 規矩：`cargo fmt --all`、clippy 0 warnings、`cargo test --locked --all-targets` 全綠
   - **不自行 commit / push**——把 `git diff` 與說明寫進輸出，使用者會 gate
   - 脈絡：yanggf8/cortexyoung issue #3，驗收線 = hook-suggest 命中率 ≥5%；
     頭號候選 `context_flag`＝把上下文搜尋導向 `cort context` 而非沉默
     （2026-09-01 的 probe 只證明了不該導向 `impact`，沒證明該沉默）；
     `pattern_not_symbol` 則先採樣看它吃掉哪些 pattern 再決定
   樣本 <10：誠實說還要等，不要硬開規則。
5. 數據品質優先（使用者的政策：初期 bug 先修）：CORT-AUDIT.md 出現「無法判讀」/`?`、
   FTS drift>0、fresh 翻 STALE、decline 欄整批消失（hooks 可能跑回舊 binary →
   提醒 `cargo install --path /home/yanggf/a/cortexyoung/rust --force`）——先查根因再回報。
5b. **未chunk檔要逐檔判形狀，不要只看數字**（2026-09-09 開出 cortexyoung#5 的那條路）：
   對報告列出的每個未chunk檔**實際開檔看**——
   - 沒有任何宣告（只 import 後呼叫的 driver script）→ 已知誤報形狀（cortexyoung#2），不必動作。
   - **有宣告卻 0 chunks → 是 cortexyoung#5**：索引停在一個從未提交、後來被 git 還原的版本，
     增量因 `git diff` 為空而永不重看（`cort index --incremental` 會回報 `files_examined: 0`）。
     驗證：比對 `file_state.file_content_hash` 與磁碟檔的 sha256，不同即確認。
     修復只能靠**全量** `cort index`；修完複查缺口數並在發現裡寫明「哪個檔、修好沒」。
   同時回報 #5 的影響面：缺口數與昨日的差、修復後是否回落、以及是否有新檔踩進同一形狀
   （這是 #5 在上游修好之前唯一的觀測手段）。
6. `git -C /home/yanggf/a/cortexyoung fetch` 後看 `HEAD..origin/master` 有無新 commit。

## 環境

- cargo：`/home/yanggf/.cargo/bin/cargo`（cron 的 PATH 很瘦）
- usage.db 全程唯讀（`file:...?immutable=1`）
- 兩個 repo 的 CLAUDE.md / AGENTS.md 都要遵守
