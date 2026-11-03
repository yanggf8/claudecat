# ClaudeCat V3 — 重工輔助與管理（給共用一台機器的 coding agent）

## 定位（V3，2026-10-24 重定位）

同一台機器上常同時跑好幾個 Claude Code／Codex／Kimi session，再加上它們委派出去的模型；
大家各自開 build、測試、E2E，負載或 swap 一衝高，使用者連打字都會卡（2026-10-24 晚上 swap 到
1.1 GiB 時，按 Enter 送不出去）。ClaudeCat 管的是這些**重工**：看機器資源、決定誰可以開跑，並幫
agent 把重工跑得不擾民。

## 負責什麼、不負責什麼

| 事項 | 歸屬 | 說明 |
|---|---|---|
| 機器資源規則：負載、記憶體與 swap、整台的重工名額、不疊兩個完整編譯、`nice`、cargo `-j`、殘留程序 | **ClaudeCat** | 門檻只在這裡維護一份。這些規則跟專案無關，以前散在各專案的派工腳本與各 session 的記憶裡。 |
| 各專案自己的派工流程：誰寫誰審、工單、認領、推送規矩 | 各專案 | 各專案只管流程，要開重工前問 ClaudeCat（`claudecat gate check`），不再自己維護門檻。 |
| Agent 與專案記憶 | cortexyoung/cort | `cort memory` 是唯一的帳本；ClaudeCat 不另存一份。 |
| Agent 工作方式的量測：收集與分析 | cortexyoung/cort | `cort measure` 由 cort 自己的 hook 觸發、自己分析。ClaudeCat 不再上傳或稽核任何東西；V2 導航仍在本機 `usage.db` 記自己的統計（`CLAUDECAT_NO_USAGE=1` 可停用），隨 V2 一起移除。 |
| Claude Code／Codex／Kimi 上的 hook | cortexyoung/cort | ClaudeCat 不另裝 hook，經 cort 的通用轉交點被呼叫。 |

依賴方向只有一個：ClaudeCat → cort。cort 不指名 ClaudeCat，拿掉 ClaudeCat 對 cort 沒有任何影響。

## 管理

- **負載與記憶體閘門**：重工開始前同時看 1 分鐘負載與當下的記憶體壓力。`nice` 只讓得出 CPU、
  讓不出記憶體，所以負載不高時也可能卡，記憶體要分開擋。起始門檻：負載 > CPU 數（NUC 的 6 顆即 6）；記憶體看
  `/proc/pressure/memory` 的 `some avg10` > 10，或 MemAvailable < 1536 MB，任一成立就擋，
  都可用環境變數調整。kernel 沒有 PSI 時只看 MemAvailable。swap 已用量照樣顯示，但不再是擋下的
  理由：WSL2 上長駐程序的閒置分頁會一直留在 swap，已用量不會降，機器閒著也會被永遠擋住。
- **同一種編譯器不疊兩個**：已經有人在跑 `rustc`，新的 cargo build／測試排隊；有 Gradle worker（`GradleWorkerMain`）正在吃 CPU，新的 gradle build／測試排隊。cargo 與 gradle 互不互擋（只在記憶體不夠時由下面的預留擋）。指令認不出是哪一種（例如 `gate check --kind build`）時保守處理，兩種都算。建置結束後留著的閒置 worker 不算：gate 取樣兩次 CPU tick（預設間隔 100 ms，`CLAUDECAT_GRADLE_SAMPLE_MS` 可改，上限 1000 ms，即 hook-gate 時限的上限），沒有增長就放行；間隔必須小於 `cort hook-gate` 實際設定的時限（預設 200 ms），否則逾時放行，所有拒絕都會失效。
  閒置的 Gradle daemon 不算：它在建置後會常駐數小時。代價是沒經過 `gate run` 的 Gradle 純編譯
  （Java 在 daemon 裡編，不開 worker）偵測不到；要擋它，Gradle 建置就要經 `gate run --kind build`。
  `gate run --kind build|test` 另外持有每種編譯器各一把鎖（`compiler-cargo.lock`、`compiler-gradle.lock`；認不出的指令如 `make`、`just` 全持有，`npm`／`npx`／`pnpm`／`yarn`／`node`／`vitest`／`jest`／`playwright`／`pytest`／`python` 這類執行器不持有也不被編譯鎖擋，只佔建置名額並受負載與記憶體檢查），蓋住 cargo 產生第一個 `rustc` 前的起動窗口。
- **預留記憶體**：剛起動（預設 60 秒內，`CLAUDECAT_RESERVE_WARMUP_SECS`）的持有者還沒把記憶體用出來，gate 先替它預留估計值（gradle 3072 MB、cargo 1536 MB，`CLAUDECAT_RESERVE_GRADLE_MB`、`CLAUDECAT_RESERVE_CARGO_MB`）；`MemAvailable` 扣掉預留後不夠新指令的估計值就拒絕（`reserved`）。暖機後的實際用量已在 `MemAvailable` 裡，不再重複預留。
- **hook 放行紀錄**：`gate hook` 放行的建置／測試指令是由 Claude 直接執行，不會登記持有者，所以 hook 在放行時會在 gate 目錄的 `admitted.jsonl` 追加一行 `{ts, session, toolchains}`（不含指令文字），在同一個暖機視窗內與持有者一樣按 toolchain 計入預留（`reserved=` 與 `status` 都會顯示），讓同一輪同時發出的 `cargo test` 與 `./gradlew test` 不會都通過、也讓直接執行的 Gradle 暖機期可見。超過暖機視窗的列忽略並在下次寫入時清除，最多保留 64 列。檢查與寫入由 `admission.lock` 序列化（等候上限約 50 ms）；鎖逾時或任何 I/O 錯誤一律放行且不記錄。被拒絕的指令、`gate check`、`gate run`（已登記持有者）都不寫入。同一個 session 對同一 toolchain 的新紀錄會取代舊紀錄（循序執行的指令不會累積預留；不同 session、不同 toolchain 各自保留）；時間戳在未來的列與同列重複的 toolchain 一律忽略或只算一次。取捨：hook 放行後實際沒執行的指令，其預留會留到暖機視窗結束。
- **名額是鎖，不是公告**：整台的重工名額用 `flock`，程序死掉時由 kernel 釋放，
  不會留下過期紀錄；鎖檔記著持有者的 pid 與指令，「誰在跑」就是從鎖算出來的。
- **名額依本機資源**：建置名額（`slot-N.lock`）= min(CPU 數／2, MemTotal／最大工具鏈預估)，
  `--kind other` 另一池（`other-N.lock`）= CPU 數，互不占用：長時間掛著、幾乎不吃資源的 LLM 包裝器
  不會擋建置。2026-11-03 實測：gpu 8／16、NUC 3／6、Thinkpade15 6／16（受記憶體限制）。
- **兩個入口，同一套門檻**：
  - shell 與派工腳本呼叫 `claudecat gate check`，用 exit code 回答；
  - agent 直接下的指令，經 cort 的 `PreToolUse` hook 轉交過來。Claude Code、Codex、Kimi 三邊都裝了
    cort 的 hook，所以委派出去的模型也管得到。轉交有時間上限，ClaudeCat 不在、逾時或出錯一律放行。
- **閘門壞掉不擋人**：讀不到 `/proc`、鎖目錄建不起來等內部錯誤一律放行，只印一行警告。

## 輔助

- 被擋下時說清楚在等什麼：現在的負載、記憶體壓力（PSI、MemAvailable）與 swap、誰占著名額、哪些編譯程序在跑、從什麼時候開始。
- 以低優先序（`nice`）執行重工，使用者的終端機保持順暢。
- 一個指令看整台機器的現況，人和 agent 看到的是同一份；包括 Codex 工作結束後留下來的孤兒
  `app-server-broker`／`codex app-server`（每組約 45 MB，不會自己結束），以及它們屬於哪個工作目錄。
  孤兒的判定是「parent 已不是 codex-companion」，不是「parent 是 pid 1」：WSL 會把孤兒交給
  session 自己的 `/init`，它的 pid 不是 1。
  只回報，不自動收掉。

## 介面（建置中）

| 指令 | 用途 | 回答 |
|---|---|---|
| `claudecat gate check [--kind build\|test\|e2e\|other]` | 腳本與 shell 問「現在能不能開工」 | exit 0 可以；**exit 75**（EX_TEMPFAIL）＝排隊，原因印在 stderr |
| `claudecat gate run [--kind …] -- <cmd>` | 拿名額、以 `nice` 執行、結束時釋放 | 指令本身的 exit code；拿不到名額回 75 |
| `claudecat status [--json]` | 整台現況：負載、記憶體壓力（PSI、MemAvailable）、swap、名額持有者、編譯程序、孤兒 Codex broker | 只回報 |
| `claudecat gate usage [--json] [--days N]` | 彙總 `gate run` 的實測用量，供校準預留記憶體與門檻 | 無記錄時輸出空表、exit 0 |
| `claudecat gate hook` | 由 cort 轉交 Bash payload，從 stdin 讀 `tool_input.command` | 重工被擋時 stdout 一行原因與重試指示、exit 75；其他情況安靜放行 |
| `claudecat gate install` | 透過 `cort hook-gate set` 安裝目前的 ClaudeCat | 原樣顯示 cort 輸出與 exit code |
| `claudecat gate uninstall` | 透過 `cort hook-gate clear` 移除轉交 | 原樣顯示 cort 輸出與 exit code |

名額與負載門檻依本機資源推得（`claudecat status` 的 `limits:` 一行列出數值與依據）：
build／test／e2e 名額 = min(CPU 數／2, MemTotal／最大工具鏈預估)；`--kind other`（LLM 包裝器、腳本，
也是 `gate check`／`gate run` 的預設 kind）另用一池 `other-N.lock`，大小 = CPU 數，不占建置名額；
負載上限 = CPU 數 × 1.0（NUC 的 WSL 是 6 顆，即舊值 6）。讀不到 cpuinfo／MemTotal 時退回 3 個名額、負載 6。
門檻用 `CLAUDECAT_MAX_LOAD`、`CLAUDECAT_MAX_MEM_PRESSURE`、`CLAUDECAT_MIN_AVAILABLE_MB`、`CLAUDECAT_SLOTS`、
`CLAUDECAT_OTHER_SLOTS` 覆寫（不是正數就用推得值或預設）；
名額目錄是 `CLAUDECAT_GATE_DIR`，否則 `$XDG_RUNTIME_DIR/claudecat/gate`，再否則
`/tmp/claudecat-<uid>/gate`。`gate check` 只回答「現在能不能開工」，不占名額；要讓名額真正反映
誰在跑，重工本身要經 `gate run` 執行。

`claudecat` 自己被殺時，名額會由 kernel 釋放，但子程序不會跟著結束，所以派工端逾時要殺整個
process group。`gate run` 會原樣回傳子程序的 exit code；子程序自己回 75 時呼叫端分不出是排隊，
要排隊語意請先 `gate check`。

`gate hook` 會辨識引號、跳脫字元、`#` 註解、開頭的重導向（`2>log cargo test`）與 shell 分段符號；各段略過前置變數設定、`nice`、`timeout`、
`env`、`exec`、`command` 後，將 Cargo 的 build/test/clippy/check/bench/run/doc/nextest
和 Gradle 指令視為重工。多段中有 test、bench 或 nextest 時，以測試門檻判斷；`gate run` 那段
已有自己的排隊機制，不再重複攔截。heredoc（`<<EOF` 到結束行）的內容是資料，不當成指令。無法解析的指令放行。已知漏網包括 `$(…)`、反引號、
`bash -c`、`make`／`just` 包裹的指令，以及腳本檔內的重工。

`gate hook` 也會在負載判斷前找同目錄、同 target、同參數的執行中 Cargo 指令；找到就回報 duplicate，
即使機器閒置也會擋。Gradle 不做重複比對。target 只依 `--target-dir`、`CARGO_TARGET_DIR` 或預設目錄估算，
只可能漏抓、不會因此誤擋；一般 Cargo 程序的 `environ` 讀不到時也可能漏抓。目錄先做字面比對，只有不相等時才對新指令目錄
呼叫一次 `canonicalize`；無回應的網路檔案系統可能讓 hook 超過 cort 的 200 ms 時限而放行。
duplicate 訊息的 `since` 只用於 `gate run` 持有者，不用於一般 Cargo 程序。

被擋的重工會記在 `$CLAUDECAT_GATE_LOG`；未設定時依序使用
`$XDG_STATE_HOME/claudecat/gate-denies.jsonl` 或 `$HOME/.local/state/claudecat/gate-denies.jsonl`。
目錄權限為 `0700`、檔案為 `0600`。每列只含時間、session 與 Cargo 指令身分的短雜湊、子命令及原因；
不記指令文字、路徑或原始 session id。Gradle 也會記 deny，但不計重試。相同 session 的 Cargo 指令
反覆被擋時，hook 從第二次起提示重試次數；並行寫入與壓縮可能讓次數成為近似值。
超過七天的列會在**下一次寫入或執行 `claudecat gate log`** 時清除；長時間沒有這兩種操作時舊列仍會留著。
`claudecat gate log [--json] [--days N]`（預設七天）彙總各原因與子命令的 deny 次數、重試分布及放行前等待秒數。
比對執行中 Cargo 指令時，若目錄位於無回應的網路檔案系統，目錄解析可能超過 cort 的 200 ms 時限而放行。

deny 列另外帶 `mem_available_mb`、`psi_some_avg10`、`load_1m`（被擋當下的數字；舊列沒有這幾欄，仍可讀），
供日後校準門檻。

`gate run` 結束時還會往 `$CLAUDECAT_GATE_USAGE_LOG`（未設定時與 deny 記錄同目錄的 `gate-usage.jsonl`）
追加一列用量，同樣 `0600`、七天清除。取樣執行緒每 `CLAUDECAT_USAGE_SAMPLE_MS`（預設 2000）毫秒走一次
子程序樹（由 `/proc` 的 ppid 往下），加總 `VmRSS` 取峰值；起跑初期會以 50 ms 起倍增的間隔多取幾次，
短工作也有讀數。每列欄位：`ts`、`session`（目前恆為 null）、`toolchain`（`cargo`|`gradle`|`unknown`）、
`class`（`test`|`build`|`compile`|`other`）、`kind`、`wall_secs`、`peak_rss_mb`、`gradle_family_rss_mb`（僅 Gradle：另含已脫離行程樹的 Gradle daemon／worker／Kotlin daemon，同使用者所有 Gradle JVM 的峰值總和）、`cpu_secs`（含已回收的子孫行程）、
`exit`，以及起跑與結束時的 `start`／`end`：`mem_available_mb`、`psi_some_avg10`、`load_1m`、`swap_used_mb`。
不記指令文字。`class` 由 argv 推得（看穿 `sh -c`）：Gradle 的 test／check／`*Test`／`--tests` 為 `test`，
`build`／`bootJar` 等為 `build`，`compile*`／`classes`／`assemble`／`jar` 為 `compile`（同時出現時 test > build > compile）；
Cargo 的 test／nextest／bench 為 `test`，build／check／clippy／doc／run 為 `build`。取樣失敗一律略過，
結束後最多多等 50 ms 收取樣緒。`RSS` 含共享頁，會略高於獨佔量。測試可用 `CLAUDECAT_USAGE_PID_ROOT` 讓取樣讀真的 `/proc`，
閘門自己的數字仍讀 `CLAUDECAT_PROC_ROOT`。
`claudecat gate usage [--json] [--days N]`（預設七天）依 (toolchain, class) 彙總次數、`peak_rss_mb` 的 p50／p90／max、
`wall_secs` 的 p50／p90 與 `cpu_secs` 平均。

安裝轉交需有 cort 的 `hook-gate` 子指令；`gate install` 與 `gate uninstall` 不直接修改 cort 設定檔。

## 不再提供的東西

- **V2 導航地圖**（`scan`、`update`、`navigate`、`explore`、`track`）：做出來過，也寫進過 agent 的
  指示檔，但 30 天內 agent 呼叫 0 次（`claudecat usage`），Claude Code 的 transcript 裡沒有任何一次
  呼叫；相關程式碼會隨 V3 移除。
- **用量上傳與每日稽核**（`claudecat telemetry`、`cort-audit`、`findings`、`doctor`、`CORT-AUDIT.md`）：
  改由 `cort measure` 負責；表格的歷史已匯入 cort 的量測資料庫（`cort measure import-audit`）。
  **升級前先清舊排程**：這些子命令已經不存在，還留著舊條目的機器在升級後會每天報 unknown
  subcommand。要清的是 crontab 裡呼叫 `claudecat cort-audit` 的 09:17 條目與跑每日分析的 09:29 條目，
  以及 `claudecat-telemetry-sync.timer`／`.service`（`systemctl --user disable --now` 後刪 unit 檔）。

## 授權

MIT
