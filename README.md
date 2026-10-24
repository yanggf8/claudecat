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
| Agent 工作方式的量測：收集與分析 | cortexyoung/cort | `cort measure` 由 cort 自己的 hook 觸發、自己分析。ClaudeCat 不再收集任何東西。 |
| Claude Code／Codex／Kimi 上的 hook | cortexyoung/cort | ClaudeCat 不另裝 hook，經 cort 的通用轉交點被呼叫。 |

依賴方向只有一個：ClaudeCat → cort。cort 不指名 ClaudeCat，拿掉 ClaudeCat 對 cort 沒有任何影響。

## 管理

- **負載與記憶體閘門**：重工開始前同時看 1 分鐘負載與 swap 已用量。`nice` 只讓得出 CPU、
  讓不出記憶體，所以負載不高時也可能卡，swap 要分開擋。起始門檻：負載 > 6、swap ≥ 1024 MB，
  都可用環境變數調整。
- **不疊兩個完整編譯**：已經有人在跑 `rustc` 或 Gradle 時，新的 build／測試排隊，不疊上去。
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
  只回報，不自動收掉。

## 介面（建置中）

| 指令 | 用途 | 回答 |
|---|---|---|
| `claudecat gate check [--kind build\|test\|e2e\|other]` | 腳本與 shell 問「現在能不能開工」 | exit 0 可以；**exit 75**（EX_TEMPFAIL）＝排隊，原因印在 stderr |
| `claudecat gate run [--kind …] -- <cmd>` | 拿名額、以 `nice` 執行、結束時釋放 | 指令本身的 exit code；拿不到名額回 75 |
| `claudecat status [--json]` | 整台現況：負載、swap、名額持有者、編譯程序、孤兒 Codex broker | 只回報 |
| `claudecat gate hook` | 由 cort 的轉交點呼叫，payload 走 stdin | deny 並附原因；逾時或出錯一律放行 |

上線順序：`gate check`、`gate run`、`status` 先上（不依賴 cort）；`gate hook` 等 cort 拆完搜尋建議、
加上通用轉交點之後。

## 不再提供的東西

- **V2 導航地圖**（`scan`、`update`、`navigate`、`explore`、`track`）：做出來過，也寫進過 agent 的
  指示檔，但 30 天內 agent 呼叫 0 次（`claudecat usage`），Claude Code 的 transcript 裡沒有任何一次
  呼叫；相關程式碼會隨 V3 移除。
- **用量上傳與每日稽核**（`claudecat telemetry sync`、`CORT-AUDIT.md`）：改由 `cort measure` 負責。

## 授權

MIT
