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
   同一批列（cortexyoung 09f55136 起）還帶 `.shape`＝`工具名|排序後的 top-level key 名`，
   那是 issue #3 給的需求排序依據：步驟 4 要開哪條規則，看哪個 shape 最常被沉默掉，
   不要只看 decline 標籤。`shape` 只含欄位名，沒有 payload 內容，可以直接貼進報告。
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
   提醒先跑 `cort_upgrade --check` 診斷，再 `cort_upgrade` 修；它不動才退回
   `cargo install --path /home/yanggf/a/cortexyoung/rust --force`）——先查根因再回報。
5b. **未chunk檔用欄位判形狀，不要再開檔人工猜**（cortexyoung schema v7 起；09-09 那條人工走法
   已被一個欄位取代）：`file_state.chunk_count` 是三態，cort-audit 報告直接給分類——
   - `0`＝extractor 掃過、檔內沒有可 chunk 的宣告，是**正確的沉默**（cortexyoung#2），不必動作。
   - `>0` 卻不在 chunks＝**真缺口**，就是 cortexyoung#5 的形狀（索引停在一個從未提交、
     後來被 git 還原的版本，增量因 `git diff` 為空而永不重看）。修復只能靠**全量**
     `cort index`；修完複查缺口數並在發現裡寫明「哪個檔、修好沒」。
   - `-1`＝v7 之前寫入且從未重寫，**不是掃描結果**；要全量索引一次才有定論，不可當成缺口或無缺口。
   另有 v6 的 `file_state.indexed_uncommitted`：>0 表示有檔案索引自未提交內容（#5 的漂移來源），
   看到就報。報告若說「無法用欄位判讀」＝DB 還是舊 schema 或查詢失敗，先按步驟 5 查根因。
   **只有真缺口 >0 時**才回報 #5 的影響面（與昨日的差、修復後是否回落、是否有新檔踩進同一形狀）。
6. `git -C /home/yanggf/a/cortexyoung fetch` 後看 `HEAD..origin/master` 有無新 commit。

## 環境

- cargo：`/home/yanggf/.cargo/bin/cargo`（cron 的 PATH 很瘦）
- usage.db 全程唯讀（`file:...?immutable=1`）
- 兩個 repo 的 CLAUDE.md / AGENTS.md 都要遵守
