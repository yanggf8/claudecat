# ClaudeCat V3 — 重工輔助與管理（給共用一台機器的 coding agent）

## 定位（V3，2026-10-24 重定位）

同一台機器上常同時跑好幾個 Claude Code／Codex／Kimi session，再加上它們委派出去的模型；
大家各自開 build、測試、E2E，負載或 swap 一衝高，使用者連打字都會卡（2026-10-24 晚上 swap 到
1.1 GiB 時，按 Enter 送不出去）。ClaudeCat 管的是這些**重工**：看機器資源、決定誰可以開跑，並幫
agent 把重工跑得不擾民。

把關（gating）是 ClaudeCat 的主要功能；跨機器看全局、查哪台放得下（`claudecat fleet`）是它的延伸。

## 負責什麼、不負責什麼

| 事項 | 歸屬 | 說明 |
|---|---|---|
| 機器資源規則：負載（含預留負載）、記憶體壓力與可用記憶體（含預留）、`nice`、殘留程序；沒有名額、沒有編譯鎖 | **ClaudeCat** | 門檻只在這裡維護一份。這些規則跟專案無關，以前散在各專案的派工腳本與各 session 的記憶裡。 |
| 各專案自己的派工流程：誰寫誰審、工單、認領、推送規矩 | 各專案 | 各專案只管流程，要開重工前問 ClaudeCat（`claudecat gate check`），不再自己維護門檻。 |
| Agent 與專案記憶 | cortexyoung/cort | `cort memory` 是唯一的帳本；ClaudeCat 不另存一份。 |
| Agent 工作方式的量測：收集與分析 | cortexyoung/cort | `cort measure` 由 cort 自己的 hook 觸發、自己分析。ClaudeCat 不再上傳或稽核任何東西；V2 導航仍在本機 `usage.db` 記自己的統計（`CLAUDECAT_NO_USAGE=1` 可停用），隨 V2 一起移除。 |
| Claude Code／Codex／Kimi 上的 hook | cortexyoung/cort | ClaudeCat 不另裝 hook，經 cort 的通用轉交點被呼叫。 |

依賴方向只有一個：ClaudeCat → cort。cort 不指名 ClaudeCat，拿掉 ClaudeCat 對 cort 沒有任何影響。

## 管理

- **放行只看資源**：把關是 ClaudeCat 的主要功能。重工開始前看負載（含預留負載）、記憶體壓力、可用記憶體（含預留）；
  沒有名額，也沒有編譯鎖（2026-11-28 起，AI-Board claude-code #5955）。拒絕原因有五種，都是 exit 75：

  | 原因 | 什麼時候 |
  |---|---|
  | `load`、`memory`、`reserved` | 資源不夠，依 kind 走下表其中一列 |
  | `duplicate` | 同目錄、同 target、同參數的 Cargo 指令已在執行（負載判斷之前就擋，見下面「`duplicate`」） |
  | `resources` | 讀不到負載或記憶體（沒有數字就分不出閒置的機器和正在 swap 的機器，見下面「內部錯誤」） |

  拒絕訊息維持 `gate denied (<原因>) ` 前綴，多個原因以逗號相接。輸出的串流依入口而定：**`gate check`／`gate run` 寫 stderr；
  `gate hook` 寫 stdout**（那是給 agent 看的拒絕說明與重試指示；放行時 gate 自己不輸出任何東西）。

  | kind | `load` | `memory` | `reserved` |
  |---|---|---|---|
  | build／test／e2e（cargo、Gradle、認不出的建置） | `load_1m + reserved_load + need_load > max_load` | PSI `some avg10` > 10，或 MemAvailable < 1536 MB | `MemAvailable − reserved_mb < need_mb` |
  | other（含 JS／Python 測試執行器；`gate check`／`gate run` 的預設） | `load_1m > max_load`，不吃別人的預留負載 | 同上 | `MemAvailable − reserved_mb < 1024` |

  `need_mb`、`need_load` 是這個指令自己的估計值（由指令內容推得，kind 只決定走哪一列）；`reserved_load`、`reserved_mb`
  是暖機窗內其他工作的預留（見下）。`load` 拒絕會印 `load=`、`reserved_load=`、`need_load=`。
- **門檻**：負載上限 `max_load` = CPU 數 × 1.0（NUC 的 WSL 是 6 顆，即 6）。記憶體看 PSI `some avg10` > 10 或
  MemAvailable < 1536 MB，任一成立就擋；`nice` 只讓得出 CPU、讓不出記憶體，所以負載不高時也可能卡，記憶體要分開擋。
  可用 `CLAUDECAT_MAX_LOAD`、`CLAUDECAT_MAX_MEM_PRESSURE`、`CLAUDECAT_MIN_AVAILABLE_MB` 調整（不是正數就用預設）。
  kernel 沒有 PSI 時只看 MemAvailable。swap 已用量照樣顯示，但不是擋下的理由：WSL2 上長駐程序的閒置分頁會一直留在 swap，
  已用量不會降，機器閒著也會被永遠擋住。
- **`duplicate`**：hook 在負載判斷前找同目錄、同 target、同參數的執行中 Cargo 指令，找到就擋，即使機器閒置（詳見下面「`gate hook`」）。
- **沒有名額、沒有編譯鎖**：`gate run` 要開幾個就開幾個，只受上面的規則約束；不同專案的 cargo 可以同時編，同一個 target 目錄由
  cargo 自己的檔案鎖排隊。已經有人在跑 `rustc`，或有 Gradle worker 正在吃 CPU，都不再是拒絕的理由：`status` 的 `compiling`
  列出 rustc 與 CPU tick 有增長的 `GradleWorkerMain`（取樣兩次 CPU tick，預設間隔 100 ms，`CLAUDECAT_GRADLE_SAMPLE_MS`
  可改），純供參考。`CLAUDECAT_SLOTS`、`CLAUDECAT_OTHER_SLOTS` 不再有作用：設了的話，文字版 `claudecat status`
  在 stderr 印一行提示（不進 `--json`）。
- **預留（暖機窗內）**：剛起動（預設 60 秒，`CLAUDECAT_RESERVE_WARMUP_SECS`）的工作還沒把資源用出來，gate 先替它預留估計值；
  暖機後的實際用量已在 `load_1m` 與 `MemAvailable` 裡，不再重複預留。誰在預留：`gate run` 的持有者（登記在 gate 目錄的
  `run-<pid>-<隨機>.lock`，數量不設上限；換版前起動、還沒結束的舊 `slot-N`／`other-N`／`compiler-*.lock` 也算）加 hook 的放行紀錄。
  - 記憶體：gradle 3072 MB、cargo 1536 MB、`other` 1024 MB（`CLAUDECAT_RESERVE_GRADLE_MB`、`CLAUDECAT_RESERVE_CARGO_MB`、
    `CLAUDECAT_RESERVE_OTHER_MB`）；`MemAvailable` 扣掉預留後不夠新指令的估計值就拒絕（`reserved`）。Gradle **測試**
    （gradle／gradlew 帶 `test` 或 `*:test` 任務；`-x test` 這類選項值不算）的 Gradle 預留是「fork 數 × 3072 MB」：fork 數取
    `-PtestForks=N`（或 `-P testForks=N`），沒寫就照 build 的預設——本機 MemTotal ≥ 8192 MB 且 ≥ 4 CPU 取 2，否則（含讀不到）取 1。
    原因：2 個 fork 的後端測試在 mgpu 實測尖峰約 5.4 GB，固定 3 GB 低估（2026-11-23，AI-Board claude-code #4781）。
    拿掉 other 池後，派工沒有數量上限，風險在記憶體（gpu 上 other 的 RSS p50 1.6 GB、p90 6.9 GB、最大 12 GB），
    所以 other 的 holder 在暖機窗內各留 1024 MB，新的 other 要過 `reserved`。
  - 負載：cargo `max(2.0, cpus × 0.25)`（16 核 → 4.0、6 CPU → 2.0）、Gradle `1.0 × fork 數`（非測試 Gradle＝1）、其他 0
    （`CLAUDECAT_RESERVE_CARGO_LOAD`、`CLAUDECAT_RESERVE_GRADLE_LOAD`；預設值暫定，等 `gate usage` 的量測校正）。
    在窗內線性遞減：經過 t 秒時計入「估計值 × (1 − t／窗長)」，大致跟上 `load_1m` 反映實際負載的速度，窗尾不重複計、
    窗後不漏計，幾秒的短指令也不會把預留卡滿 60 秒。
- **hook 放行紀錄**：`gate hook` 放行的建置／測試指令是由 Claude 直接執行，不會登記持有者，所以 hook 在放行時會在 gate 目錄的 `admitted.jsonl` 追加一行 `{ts, session, toolchains}`，Gradle 測試多於 1 fork 時另帶 `gradle_forks`（不含指令文字；舊版寫的列沒有這欄，算 1 fork），在同一個暖機視窗內與持有者一樣按 toolchain 計入預留（`reserved=` 與 `status` 都會顯示），讓同一輪同時發出的 `cargo test` 與 `./gradlew test` 不會都通過、也讓直接執行的 Gradle 暖機期可見。超過暖機視窗的列忽略並在下次寫入時清除，最多保留 64 列。檢查與寫入由 `admission.lock` 序列化（等候上限約 50 ms）；鎖逾時或任何 I/O 錯誤時**不記錄放行紀錄，但判斷照常**：hook 用鎖外的快照（連同其中讀到的持有者與放行紀錄）決定放行或拒絕，資源不夠一樣擋，不是一律放行（見下面「共用臨界區」）。被拒絕的指令、`gate check`、`gate run`（已登記持有者）都不寫入。同一個 session 對同一 toolchain 的新紀錄會取代舊紀錄（循序執行的指令不會累積預留；不同 session、不同 toolchain 各自保留）；時間戳在未來的列與同列重複的 toolchain 一律忽略或只算一次。取捨：hook 放行後實際沒執行的指令，其預留會留到暖機視窗結束。
- **共用臨界區**：hook 與 `gate run` 的「檢查→登記」都在 `admission.lock` 內完成：鎖內重新列舉持有者與放行紀錄、判斷、登記
  （hook 寫一行 `admitted.jsonl`；`gate run` 把登記檔換成正式名稱），所以同一輪同時發出的兩個 cargo 不會都看到「還有餘裕」。
  hook 等鎖至多 50 ms，等不到或拿鎖時出錯，就用鎖外的快照判斷、不記錄；`gate run` 等至多 2 秒，逾時（或拿鎖時出錯）印一行警告後**不經判斷**直接執行，
  拿得到鎖但登記寫不進去時也是警告一行後執行。Gradle 的 CPU
  取樣留在鎖外。因預留而起的拒絕，hook 的等待提示是「等暖機過（最多 N 秒）」。
- **登記是鎖，不是公告**：`gate run` 的登記用 `flock`，程序死掉時由 kernel 釋放，不會留下過期紀錄；鎖檔記著持有者的 pid 與指令，
  「誰在跑」就是從鎖算出來的。被 SIGTERM 的 `gate run` 沒有訊號處理，會留下殘檔：下一個列舉的 claudecat 清掉它，不計為持有者。
  gate 目錄權限 `0700`；換版前建立的目錄若權限過寬，`status` 會提示。
- **兩個入口，同一套門檻**：
  - shell 與派工腳本呼叫 `claudecat gate check`，用 exit code 回答；
  - agent 直接下的指令，經 cort 的 `PreToolUse` hook 轉交過來。Claude Code、Codex、Kimi 三邊都裝了
    cort 的 hook，所以委派出去的模型也管得到。轉交有時間上限，ClaudeCat 不在、逾時或出錯一律放行。
- **內部錯誤**：讀不到負載與記憶體時擋住（exit 75，原因 `resources`），因為沒有數字就分不出閒置的機器和正在 swap 的機器；
  gate 目錄建不起來等錯誤則放行，只印一行警告。

## 輔助

- 被擋下時說清楚在等什麼：現在的負載、記憶體壓力（PSI、MemAvailable）與 swap、誰在跑（`gate run` 的持有者）、哪些編譯程序在跑、從什麼時候開始；因預留而起的拒絕會說明最多還要暖機幾秒。
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
| `claudecat gate run [--kind …] -- <cmd>` | 登記、判斷，以 `nice` 執行，結束時註銷 | 指令本身的 exit code；被擋回 75 |
| `claudecat status [--json]` | 整台現況：負載、記憶體壓力（PSI、MemAvailable）、swap、容量（`capacity`）、`gate run` 持有者、編譯程序、孤兒 Codex broker | 只回報 |
| `claudecat gate usage [--json] [--days N]` | 彙總 `gate run` 的實測用量，供校準預留記憶體與門檻 | 無記錄時輸出空表、exit 0 |
| `claudecat fleet [--json]`、`claudecat fleet pick …` | 各機器的容量一張表；列出放得下一件工作的機器（見下面「Fleet」） | 沒有機器合適時 exit 1 |
| `claudecat gate hook` | 由 cort 轉交 Bash payload，從 stdin 讀 `tool_input.command` | 重工被擋時 stdout 一行原因與重試指示、exit 75；其他情況安靜放行 |
| `claudecat gate install` | 透過 `cort hook-gate set` 安裝目前的 ClaudeCat | 原樣顯示 cort 輸出與 exit code |
| `claudecat gate uninstall` | 透過 `cort hook-gate clear` 移除轉交 | 原樣顯示 cort 輸出與 exit code |

負載門檻依本機資源推得（`claudecat status` 的 `limits:` 一行列出數值與依據）：負載上限 = CPU 數 × 1.0
（NUC 的 WSL 是 6 顆，即舊值 6），讀不到 cpuinfo 時退回 6。門檻用 `CLAUDECAT_MAX_LOAD`、
`CLAUDECAT_MAX_MEM_PRESSURE`、`CLAUDECAT_MIN_AVAILABLE_MB` 覆寫（不是正數就用推得值或預設）；預留用
`CLAUDECAT_RESERVE_WARMUP_SECS`、`CLAUDECAT_RESERVE_GRADLE_MB`、`CLAUDECAT_RESERVE_CARGO_MB`、
`CLAUDECAT_RESERVE_OTHER_MB`、`CLAUDECAT_RESERVE_CARGO_LOAD`、`CLAUDECAT_RESERVE_GRADLE_LOAD` 覆寫。
`CLAUDECAT_SLOTS`、`CLAUDECAT_OTHER_SLOTS` 不再有作用。gate 目錄是 `CLAUDECAT_GATE_DIR`，否則
`$XDG_RUNTIME_DIR/claudecat/gate`，再否則 `/tmp/claudecat-<uid>/gate`。`gate check` 只回答「現在能不能開工」，
不登記；要讓 `status` 與 fleet 看得到這件工作，重工本身要經 `gate run` 執行（hook 放行的建置／測試
另有放行紀錄，只在暖機窗內可見）。

`claudecat` 自己被殺時，登記會由 kernel 釋放，但子程序不會跟著結束，所以派工端逾時要殺整個
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

`gate check` 與 `gate run` 的拒絕同樣寫進 deny log（`sub` 為 `check:<kind>`、`run:<kind>`，並帶 `reserved_load`、`need_load`），
放行不寫；換版前因 `slots`、`compiler`、`compiler-lock` 被擋的舊列，`gate log` 照樣彙總。

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

## Fleet（各機器的容量與 Claude session 回報）

派工前先問「哪台機器現在放得下」：每台機器把自己的快照寫進 cort 記憶用的同一個 Turso 資料庫，協調者一個指令看全局。
各機規格差距大，名額又沒辦法比（每台是照自己的 CPU 與記憶體算的），所以每台用同一把尺回報**絕對容量**。
ClaudeCat 不自動派工：`fleet pick` 只是「哪台放得下」的查詢，派工與編隊由總管機依任務決定。

| 指令 | 用途 |
|---|---|
| `claudecat fleet push` | 收集本機快照並 upsert 自己那一列（失敗時一行錯誤、exit 1） |
| `claudecat fleet [--json]` | 全部機器一張表，每台一列（欄位見下）。超過 180 秒沒更新標 `IDLE`；讀不到資源的機器標 `UNKNOWN` |
| `claudecat fleet pick [--need-mb N] [--need-load X] [--json]` | 依適合度列出放得下的機器：非 UNKNOWN、沒被擋（`blocked` 為空）、記憶體餘裕 ≥ N MB、負載餘裕 ≥ X 核（兩個旗標預設 0，也就是不要求）。負載餘裕最大者優先，同值看記憶體餘裕，再同值照主機名。沒有任何機器合適時 exit 1 |
| `claudecat fleet pick --kind gradle\|other` | **已棄用，保留一個版本。** 等於 `--need-mb 3072 --need-load 1`（gradle）或 `--need-mb 1024 --need-load 0`（other）；明寫的 `--need-mb`／`--need-load` 蓋過對應的預設，並在 stderr 印一行提示改用哪兩個旗標。提示裡的是這次實際的需求值：被蓋過的那一個照旗標的值印，照著提示改寫不會改變需求 |

表的欄位：

| 欄位 | 意義 |
|---|---|
| HOST、STATE、AGE | 主機、狀態（`ok`／`IDLE`／`UNKNOWN`）、資料年齡 |
| CPUS、MEM_GB | CPU 數、記憶體總量（GB） |
| LOAD | `load_1m/max_load` |
| HEADROOM | 負載餘裕（核，不小於 0）。近似值：Linux 的 load 含 D 狀態，WSL2 看不到 Windows 主機的負載 |
| FREE_GB | 記憶體餘裕（GB，一位小數）：可用記憶體扣掉暖機預留 |
| BLOCKED | 此刻會擋下一件 build／test 的原因（`load`、`memory`），逗號分隔；`-` 表示不擋 |
| WARM | 暖機中的工作數 |
| RUN | `gate run` 的工作數（登記的持有者，JSON 的 `holders`）。一件工作底下有幾個編譯程序在跑，都只算一件 |
| COMP | 編譯程序數（rustc、吃 CPU 的 Gradle worker，即 `status` 的 `compiling`；JSON 的 `compiling`）。純供參考，不參與判斷 |
| SESS | 執行中的 Claude session 數 |

RUN 與 COMP 是兩個數字，不相加。`-` 表示這一列說不上來，不是 0：`IDLE` 的列兩欄都是 `-`（機器沒在回報，現在跑著什麼不得而知）；
WARM、RUN 在沒有 `capacity`、`holders` 的舊版列也是 `-`（舊版列的 COMP 照它的 `compiling` 顯示）。

名額不再是任何地方的判斷依據，表裡也沒有這一欄。`fleet --json` 每台一個物件，欄位是 `host`、`os`、`state`、
`updated_at`、`age_secs`、`cpus`、`mem_total_mb`、`load_1m`、`max_load`、`load_headroom`、`mem_available_mb`、
`mem_headroom_mb`、`blocked`、`warming`、`holders`、`compiling`、`sessions_busy`、`sessions_total`、`sessions_unsupported`。
`holders`、`compiling` 是上面 RUN、COMP 的兩個數；說不上來時是 `null`（`IDLE` 的列、沒有該欄位的舊版列），不是 0。
`fleet pick --json` 每筆有 `host`、`load_headroom`、`mem_headroom_mb`、`cpus`、`mem_total_mb`、`load_1m`、`max_load`、
`blocked`（列得出來的主機一律是 `[]`）與 `idle`；`idle: true` 表示記憶體與 load 是舊值。文字版每台一行：
`<host> load_headroom=<核> mem_headroom_mb=<MB> load=<load_1m>/<max_load>`。

**換版遷移（名額與編譯鎖移除後，`fleet --json` 與 `fleet pick --json` 不再有這些欄位）：**

| 已移除的欄位 | 改讀 |
|---|---|
| `slots_used`、`slots_total`、`other_slots_used`、`other_slots_total`（`fleet --json`） | 無：名額已不存在。放不放得下改看 `load_headroom`、`mem_headroom_mb`、`blocked` |
| `compilers`（`fleet --json`） | `compiling`（編譯程序數）；`gate run` 的工作數是 `holders` |
| `free_slots`（`fleet pick --json`） | 無：名額已不存在。列得出的主機都放得下（需求用 `--need-mb`／`--need-load` 指定） |
| `mem_available_mb`（`fleet pick --json`） | `mem_headroom_mb`（可用記憶體扣掉暖機預留） |
| `load_ratio`（`fleet pick --json`） | `load_headroom`（還能再加幾核；排序改依它）。要比值用 `load_1m` 除以 `max_load` |

- **快照內容：**`status --json` 的同一份資料，加上本機執行中的 `claude` 程序（pid、cwd、CPU%、已執行秒數，抓得到時加 tmux session 名）。
  不含環境變數、指令列參數、檔案內容。session 掃描目前只支援 Linux；其他系統回報 `sessions_unsupported: true` 與空清單。
- **容量（`status --json` 的 `capacity`，也是 `fleet push` 上傳的內容）：**`cpus`、`mem_total_mb`、`max_load`、`load_1m`、
  `reserved_load`、`load_headroom`、`mem_available_mb`、`reserved_mb`、`mem_headroom_mb`、`mem_pressure`、`blocked`、`warming`、
  `warmup_secs`（這台機器生效中的暖機窗，即 `CLAUDECAT_RESERVE_WARMUP_SECS`，沒設或不是正數就是 60；預留只能相信這麼久）。
  `load_headroom` = `max(0, max_load − load_1m − reserved_load)`，**不為負**：餘裕是還能再承受多少，最少是 0，超載多少看
  `load_1m` 與 `blocked`。`blocked` 是此刻會擋下一件 build／test 的原因，讓總管不被「看起來有餘裕」誤導。頂層的 `load_1m`、
  `mem_available_mb`、`swap_used_mb`、`mem_pressure_some_avg10`、`limits.max_load`、`limits.cpus`、`limits.mem_total_mb` 保留，
  舊版總管讀得到；名額與編譯鎖欄位（`slots`、`other_slots`、`compiler_lock`、`compiler_locks`、`limits.slots*`）已移除。
- **新舊列一起看：**還沒換版的機器沒有 `capacity`（更舊的還附名額）。fleet 一律用頂層的 `load_1m`、`limits.max_load`、
  `mem_available_mb`、`limits.cpus`、`limits.mem_total_mb` 判斷：負載餘裕 = `max(0, max_load − load_1m)`、記憶體餘裕 =
  `mem_available_mb`（舊版不回報預留，可用的都是這件工作的），`BLOCKED` 只看得出負載；表的 WARM、RUN 顯示 `-`。
  舊列的名額不參與判斷。
- **列齡：**機器最多 60 秒回報一次，總管讀到時預留多半已過期。暖機窗是**回報那台機器自己的**：列裡的 `capacity.warmup_secs`
  （它的 `CLAUDECAT_RESERVE_WARMUP_SECS`；沒有這個欄位的舊列用預設的 60 秒）。列齡超過暖機窗時，fleet 不再扣
  `reserved_load`／`reserved_mb`：顯示與挑選用的餘裕是 `max(0, max_load − load_1m)` 與 `mem_available_mb`，WARM 當 0；
  `blocked` 裡只因預留而起的 `load`（`load_1m ≤ max_load`）一併去掉，其他原因照舊。暖機窗內的列照機器回報的數字（已扣預留），
  所以一台暖機窗設成 600 秒的機器，它的列到 600 秒前都還是扣過預留的。
- **儲存：**Turso HTTP pipeline API，表 `claudecat_fleet (host PRIMARY KEY, updated_at, payload)`，一台一列；值一律以參數帶入。
  連線設定讀 `cort memory configure` 存的檔（`$CORT_MEMORY_DIR` 或 `$XDG_DATA_HOME/cortexyoung/memory` 或 `$HOME/.local/share/cortexyoung/memory` 下的 `turso-memory.json`），
  也可用 `CLAUDECAT_FLEET_URL` ＋ `CLAUDECAT_FLEET_TOKEN` 覆蓋（兩個要一起設）。token 不會出現在任何輸出或錯誤訊息。
- **回報時機、不用常駐程式：**`gate check`／`gate run`／`status` 每次最多每 60 秒在背景起一個 `fleet push`（逾時 2 秒，失敗無聲，絕不拖慢或擋住閘門）；
  時間戳存在資料目錄的 `fleet-last-push`。設了 `CLAUDECAT_PROC_ROOT`（測試用的假 /proc）或 `CLAUDECAT_FLEET_AUTOPUSH=0` 時不推。
- **閒置機器：**機器只在跑閘門時回報，所以超過 180 秒沒回報（`IDLE`）就是沒工作量，視為可接工作，`fleet pick` 照常列入、排序同正常機器。
  `IDLE` 列的 RUN、COMP 顯示 `-`（JSON 為 `null`）：機器沒在回報，現在跑著什麼不得而知，不當 0。忙碌 session 當 0；
  暖機中的工作（WARM）在暖機窗過後當 0。CPU 與記憶體總量照最後一次回報；負載、負載餘裕與記憶體餘裕是最後一次回報的值，
  表格以 `*` 標示（AGE 為資料年齡），pick JSON 的 `idle: true` 同義。
  因此不需要 crontab 或常駐程式定時回報；讀不到資源的機器（`UNKNOWN`）不論新舊都不選。

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
