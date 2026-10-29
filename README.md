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

- **負載與記憶體閘門**：重工開始前同時看 1 分鐘負載與 swap 已用量。`nice` 只讓得出 CPU、
  讓不出記憶體，所以負載不高時也可能卡，swap 要分開擋。起始門檻：負載 > 6、swap ≥ 1024 MB，
  都可用環境變數調整。
- **不疊兩個完整編譯**：已經有人在跑 `rustc`，或有 Gradle worker（`GradleWorkerMain`）正在吃 CPU 時，新的 build／測試排隊，不疊上去。建置結束後留著的閒置 worker 不算：gate 取樣兩次 CPU tick（預設間隔 100 ms，`CLAUDECAT_GRADLE_SAMPLE_MS` 可改，上限 1000 ms，即 hook-gate 時限的上限），沒有增長就放行；間隔必須小於 `cort hook-gate` 實際設定的時限（預設 200 ms），否則逾時放行，所有拒絕都會失效。
  閒置的 Gradle daemon 不算：它在建置後會常駐數小時。代價是沒經過 `gate run` 的 Gradle 純編譯
  （Java 在 daemon 裡編，不開 worker）偵測不到；要擋它，Gradle 建置就要經 `gate run --kind build`。
  `gate run --kind build|test` 另外持有一把 `compiler.lock`，蓋住 cargo 產生第一個 `rustc` 前的起動窗口。
- **名額是鎖，不是公告**：整台的重工名額（預設 3 個）用 `flock`，程序死掉時由 kernel 釋放，
  不會留下過期紀錄；鎖檔記著持有者的 pid 與指令，「誰在跑」就是從鎖算出來的。
- **兩個入口，同一套門檻**：
  - shell 與派工腳本呼叫 `claudecat gate check`，用 exit code 回答；
  - agent 直接下的指令，經 cort 的 `PreToolUse` hook 轉交過來。Claude Code、Codex、Kimi 三邊都裝了
    cort 的 hook，所以委派出去的模型也管得到。轉交有時間上限，ClaudeCat 不在、逾時或出錯一律放行。
- **閘門壞掉不擋人**：讀不到 `/proc`、鎖目錄建不起來等內部錯誤一律放行，只印一行警告。

## 輔助

- 被擋下時說清楚在等什麼：現在的負載與 swap、誰占著名額、哪些編譯程序在跑、從什麼時候開始。
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
| `claudecat status [--json]` | 整台現況：負載、swap、名額持有者、編譯程序、孤兒 Codex broker | 只回報 |
| `claudecat gate hook` | 由 cort 轉交 Bash payload，從 stdin 讀 `tool_input.command` | 重工被擋時 stdout 一行原因與重試指示、exit 75；其他情況安靜放行 |
| `claudecat gate install` | 透過 `cort hook-gate set` 安裝目前的 ClaudeCat | 原樣顯示 cort 輸出與 exit code |
| `claudecat gate uninstall` | 透過 `cort hook-gate clear` 移除轉交 | 原樣顯示 cort 輸出與 exit code |

門檻用 `CLAUDECAT_MAX_LOAD`、`CLAUDECAT_MAX_SWAP_MB`、`CLAUDECAT_SLOTS` 覆寫（不是正數就用預設）；
名額目錄是 `CLAUDECAT_GATE_DIR`，否則 `$XDG_RUNTIME_DIR/claudecat/gate`，再否則
`/tmp/claudecat-<uid>/gate`。`gate check` 只回答「現在能不能開工」，不占名額；要讓名額真正反映
誰在跑，重工本身要經 `gate run` 執行。

`claudecat` 自己被殺時，名額會由 kernel 釋放，但子程序不會跟著結束，所以派工端逾時要殺整個
process group。`gate run` 會原樣回傳子程序的 exit code；子程序自己回 75 時呼叫端分不出是排隊，
要排隊語意請先 `gate check`。

`gate hook` 會辨識引號、跳脫字元、`#` 註解、開頭的重導向（`2>log cargo test`）與 shell 分段符號；各段略過前置變數設定、`nice`、`timeout`、
`env`、`exec`、`command` 後，將 Cargo 的 build/test/clippy/check/bench/run/install/doc/nextest
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
