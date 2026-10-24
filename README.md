# ClaudeCat V3 — 重工輔助與管理（給共用一台機器的 coding agent）

## 定位（V3，2026-10-24 重定位）

同一台機器上常同時跑好幾個 Claude Code／Codex／Kimi session，再加上它們委派出去的模型；
大家各自開 build、測試、E2E，負載一衝高，使用者連打字都會延遲。ClaudeCat 管的是這些**重工**：
看機器資源、決定誰可以開跑，並幫 agent 把重工跑得不擾民。

**管理**

- **名額與負載閘門**：重工指令（`cargo build/test/clippy`、Gradle、E2E 等）開始前，看 1 分鐘負載與
  同時執行的重工名額；負載過高或名額已滿就回 deny，並說明現在是誰在跑、負載多少。
- **名額是鎖，不是公告**：名額用 `flock`，程序死掉時由 kernel 釋放，不會留下過期紀錄；鎖檔裡記著
  持有者的 pid 與指令，「誰在跑」就是從鎖算出來的。
- **不另加 hook**：由 cortexyoung/cort 既有的 `PreToolUse` hook 經一個通用轉交點呼叫（cort 不指名
  ClaudeCat，依賴方向維持 ClaudeCat → cort）。Claude Code、Codex、Kimi 三邊都裝了 cort 的 hook，
  所以委派出去的模型也管得到。轉交有時間上限，ClaudeCat 不在、逾時或出錯一律放行。

**輔助**

- 被擋下時告訴 agent 該等什麼：現在的負載、誰占著名額、從什麼時候開始跑。
- 讓重工以低優先序執行（`nice`），使用者的終端機保持順暢。
- 一個指令查整台機器現在的重工狀況，人和 agent 看到的是同一份。

狀態：**建置中**。

### 不再負責的事

- **專案記憶**：由 cortexyoung/cort 的 `cort memory` 管理。
- **用量收集與分析**：由 `cort measure` 自己收、自己分析（取代 `claudecat telemetry sync` 與
  `CORT-AUDIT.md` 的每日稽核）。
- **V2 導航地圖**（`scan`、`update`、`navigate`、`explore`、`track`）：不再提供。它做出來過，
  也寫進過 agent 的指示檔，但 30 天內 agent 呼叫 0 次（`claudecat usage`），Claude Code 的
  transcript 裡沒有任何一次呼叫；相關程式碼會隨 V3 移除。

## 授權

MIT
