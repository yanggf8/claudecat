# Gate 只看負載與容量：拿掉名額與編譯鎖（設計 v2）

- 日期：2026-11-28
- 需求：AI-Board claude-code #5955（補充 #5956～#5960）
- 狀態：v1 → 作者自審 → 他審（Fable：APPROVE WITH CHANGES，4 blocker／7 should-fix）→ v2（逐條核實後修入）→ **用戶確認（2026-11-28「加，照標準工法開工」）** → 實作
- 行號皆指 main 08646bb

## 1. 為什麼改

用戶 2026-11-28：

- 「不用名額啦,都用loading」「cargo 編譯鎖也拿掉」
- 「這樣nuc才能知道全局來派工」「每一台能力差距大,也只能看capacity」
- 「我們各機還是會由總管機先按任務分派編隊, 但總管需要幫我看全局」
- 「claudecat 主要功能是 gating」

**名額跨機器沒辦法比。** 每台的名額是照自己的 CPU 和記憶體算的上限（`min(cpus/2, MemTotal/預留)`，other 池＝cpus）。總管看到「A 台 1/3、B 台 2/8」，看不出哪台還放得下一件 6 GB 的 Gradle 測試。各台規格差很多（NUC 6 CPU、gpu 16 核），能比的只有**絕對容量**。

**hook 路徑上，名額從沒擋過；擋最多的是編譯器互斥。** 各機 `claudecat gate log`（2026-11-28 收集）：

| 機器 | slots | compiler | compiler-lock | load | memory |
|---|---|---|---|---|---|
| gpu（16 核） | 0 | 21 | 2 | 8 | 0 |
| NUC（6 CPU） | 0 | 7 | 0 | 4 | 1 |
| i51149R3050 | 0 | 1 | 0 | 0 | 0 |
| ThinkPad、Mac | 0 | 0 | 0 | 0 | 0 |

注意：deny log 只有 hook 會寫（`record_deny` 唯一呼叫點 `gate.rs:2235`）；`gate run`、`gate check` 的拒絕只印 stderr。所以這張表**無法**說明派工用的 other 池有沒有碰過上限。本次改動讓 gate run／check 的拒絕也寫 deny log（4.6）。

## 2. 目標與非目標

目標：

1. gate 放行只看資源：負載（含預留）、記憶體壓力、可用記憶體（含預留）。
2. 每台用同一把尺回報**絕對容量**，供總管看全局。
3. 「誰在跑」繼續看得到。

非目標：

- 不自動派工。派工、編隊由總管機依任務決定；`fleet pick` 只是「哪台放得下」的查詢。
- 不改 hook 的分類規則；不改 `duplicate`。

## 3. 現況

拒絕原因：`load`、`memory`、`slots`、`compiler`、`compiler-lock`、`reserved`、`duplicate`。

- **hook**（PreToolUse，經 cort hook-gate）：只判斷放行，指令由 Claude 執行，不持鎖。放行時在 `admission.lock` 保護下寫一列 admission（`gate.rs:2178-2187`），暖機窗內代表它。hook 不佔名額。
- **gate run**：先 snapshot、判斷，再依序 `try_lock` 名額鎖檔 `slot-N`／`other-N`（`gate.rs:2420-2440`）；cargo 另佔 `compiler-cargo.lock`。**不拿 `admission.lock`、不寫 admission**——名額與編譯鎖的 flock 就是它唯一的原子互斥。
- **登記：** 名額鎖檔內容就是 holder。記憶體預留讀 build 池 holder（不讀 other 池）加 admissions（`gate.rs:1063-1095`）；duplicate 另掃執行中的 cargo 程序（`gate.rs:1611-1621`）。
- **編譯器偵測：** Linux 讀 `/proc`、Mac 用原生 API（`gate.rs:1317-1320`）；rustc，以及 CPU tick 有增長的 `*.GradleWorkerMain` java（`gate.rs:1444-1476`）。
- **`--kind other`：** 只看負載、記憶體與 other 池；不看編譯器、不看記憶體預留（`gate.rs:1716-1723`）。

## 4. 設計

### 4.1 放行規則

拒絕原因剩四種：`load`、`memory`、`reserved`、`duplicate`（讀不到資源時的既有處理不變）。

| kind | load | memory | reserved |
|---|---|---|---|
| build／test（cargo、Gradle、認不出的建置） | `load_1m + reserved_load + need_load > max_load` | 不變 | `MemAvailable − reserved_mb < need_mb`（不變，含 #4781 fork 倍數） |
| other（含 JS／Python 測試執行器） | **`load_1m > max_load`（與現行相同，不吃別人的預留負載）** | 不變 | `MemAvailable − reserved_mb < other_mb`（**新**，見 4.3） |

- `need_load`、`need_mb` 由指令內容估計（`argv_estimate`／`command_estimate`），kind 只決定走哪一列。`gate run --kind other -- sh -c 'cargo test'` 走 other 列，但它的 holder 依指令內容計入預留（cargo）。
- other 不吃預留負載，是為了讓外部使用者在負載上**零行為變化**（Fable S2）；other 自己的保護放在記憶體（4.3）。
- `max_load` 不變：`CLAUDECAT_MAX_LOAD`，否則 cpus × 1.0。
- `slots`、`compiler`、`compiler-lock` 三種原因移除。`CLAUDECAT_SLOTS`、`CLAUDECAT_OTHER_SLOTS` 不再有作用；設了的話，文字版 `claudecat status` 在 stderr 印一行提示（不進 `--json` 的 stdout）。
- **stderr 契約：** 拒絕訊息維持 `gate denied (<原因>) ` 前綴，並以測試釘住。`load` 拒絕要印 `load=…`、`reserved_load=…`、`need_load=…`；因預留而起的拒絕，hook 的等待提示改為「等暖機過（最多 N 秒）」（`gate.rs:2194-2210` 跟著改）。

### 4.2 不設上限的登記

`gate run` 每次執行登記一個鎖檔，數量不設上限。協定（Fable B2）：

1. 檔名 `run-<pid>-<16 位隨機十六進位>.lock`；pid 另存在 holder JSON。
2. 先以不符合列舉樣式的暫名（`.reg-<隨機>`）`create_new`、0600 建檔，**阻塞式** flock，寫完 holder JSON 並 fsync，再 `rename` 成正式名。列舉看得到的檔，一定已被鎖住且內容完整。
3. 子程序存活期間持鎖；結束時，持鎖狀態下 unlink 再放鎖。
4. **列舉：** 只列 `run-*.lock`，以唯讀方式開檔（**不用** `lock_file` 的 `create(true)`，`gate.rs:1240-1247`），非阻塞探測 flock。只認擁有者 uid 是自己的檔。
5. **清理殘檔**（被 SIGTERM 的 gate run 沒有訊號處理，殘檔是常態）：清理者 flock 成功後，比對 fd 與路徑的 (dev, ino) 相同才 unlink；不同代表檔已被換掉，放手。
6. **時間戳：** holder 的 `started_at` 若在未來（超過 5 秒寬限），視為無效、不計暖機（比照 admission，`gate.rs:1141`）。
7. **換版相容：** 換版前就起動、還沒結束的舊版 gate run，其 `slot-*.lock`、`other-*.lock`、`compiler-*.lock` 以**列舉**（不是 `0..count`）探測，被佔用就算 holder；同一 pid 握兩個舊鎖只算一次。編譯鎖檔新版不再建立、不再當判斷依據。
8. **gate 目錄權限：** 建立時改為 0700（現在是 `create_dir_all` 預設權限，`gate.rs:1554`，而路徑在 `/tmp` 下可預測）；已存在的目錄若權限過寬，status 提示。

### 4.3 預留

#### 共用臨界區（Fable B3）

hook 與 gate run **共用 `admission.lock`** 做「檢查→登記」：

```
拿 admission.lock（hook 等 50 ms；gate run 等最多 2 秒）
  → 鎖內重新列舉 run-*.lock 與 admissions
  → 判斷（4.1）
  → 登記：hook 寫 admission；gate run 完成 4.2 的 rename
→ 放鎖
```

Gradle CPU 取樣（sleep 100 ms）留在鎖外。拿不到鎖：hook 照現行行為，用鎖外的 snapshot 判斷、但不記錄 admission（v2 原寫「放行」不符現行程式，A1 審查時更正）；gate run 在等待預算內重試，逾時則照現行 `warning` 後執行，且**執行前先放掉已拿到的鎖**。

#### 預留負載

暖機窗（`CLAUDECAT_RESERVE_WARMUP_SECS`，預設 60 秒）內，每個 holder 與 admission 計入估計負載，並**在窗內線性遞減**（Fable S4）：經過 t 秒時計入 `估計值 × (1 − t/窗長)`。load_1m 的時間常數約 60 秒，線性遞減大致跟上實際反映的速度，避免窗尾重複計、窗後漏計，也讓 hook 放行的短指令（例如幾秒的 `cargo check`）不會把預留整整卡滿 60 秒。

估計值（**暫定**，Fable B1：v1 依據讀錯了單位，`gate usage` 的 cpu_mean 是每次執行的 CPU 秒數平均，不是百分比）：

- cargo：`CLAUDECAT_RESERVE_CARGO_LOAD`，預設 **max(2.0, cpus × 0.25)**——隨機器縮放（16 核 → 4.0、6 CPU → 2.0）。唯一樣本 81.9 CPU 秒／34 秒 ≈ 2.4 核，冷編譯會吃滿全部核。
- Gradle：`CLAUDECAT_RESERVE_GRADLE_LOAD`，預設 **1.0 × 測試 fork 數**（非測試 Gradle＝1）。現有 Gradle 的 cpu_secs 多半不含脫離行程樹的 daemon 與測試 worker（`gate_stats.rs:84-86`、`:453-460`），不能當依據；本次同時補量測（4.6），資料夠了再校正。
- 其他：0。
- 同一 session 的新 admission 取代舊的（`gate.rs:1191-1194`），負載預留同樣繼承這個行為：同一 session 在背景並行的兩支 cargo 只算一支。既有取捨，照舊。

#### other 的記憶體預留（Fable S5，用戶 2026-11-28 確認「加」）

拿掉 other 池後，派工路徑沒有任何併發界線。派工多半是等待 LLM 的低 CPU 工作，負載撐得住；風險在記憶體：gpu 上 other 122 次，RSS p50 1.6 GB、p90 6.9 GB、最大 12 GB。一次丟 20 件到同一台，每件需求為 0 會全部放行，`MemAvailable` 快用光才開始擋，已放行的繼續長。

提案：`CLAUDECAT_RESERVE_OTHER_MB`，預設 **1024**，other 的 holder 在暖機窗內計入記憶體預留，新的 other 要過 `reserved` 檢查。這仍是「只看資源」，不是名額。用戶 2026-11-28 確認採用。

### 4.4 狀態與容量回報

`status_value()`（`claudecat status --json`，也是 `fleet push` 上傳的內容）：

- **保留**頂層 `load_1m`、`mem_available_mb`、`swap_used_mb`、`mem_pressure_some_avg10`、`limits.max_load`、`limits.cpus`、`limits.mem_total_mb`（`fleet_view.rs:131-133` 與舊版總管在讀）。
- **移除**：`slots`、`other_slots`、`compiler_lock`、`compiler_locks`、`limits.slots*`、`limits.other_slots*`。
- **新增** `holders`（gate run 持有者，含舊版鎖）與 `capacity`：

| 欄位 | 意義 |
|---|---|
| `cpus`、`mem_total_mb` | 這台的總量；讀不到時為 null |
| `max_load`、`load_1m`、`reserved_load` | 負載上限、目前負載、暖機預留負載（已遞減） |
| `load_headroom` | `max(0, max_load − load_1m − reserved_load)`，取到小數 2 位。**不為負**（用戶 2026-11-28「不可能是負的」，B 批起）：餘裕是還能再承受多少，最少是 0；超載多少看 `load_1m` 與 `blocked`。**近似值**：Linux 的 load 含 D 狀態，Mac 不含 IO 等待、且效能核與節能核等同；WSL2 看不到 Windows 主機的負載 |
| `mem_available_mb`、`reserved_mb`、`mem_headroom_mb` | 可用、暖機預留、兩者相減（不小於 0） |
| `mem_pressure` | PSI some avg10；Mac 沒有 PSI，為 null |
| `blocked` | **此刻**會擋下一件 build／test 的原因清單（例如 `["memory"]`），讓總管與 `fleet pick` 不被「看起來有餘裕」誤導（Fable S6） |
| `warming` | 暖機中的 holder＋admission 數 |
| `warmup_secs` | 這台機器生效中的暖機窗（`CLAUDECAT_RESERVE_WARMUP_SECS`，預設 60）。B 批審查後新增：fleet 讀列時以它為準（4.5 列齡），不再一律當 60 秒 |

- `compiling` 保留為**純資訊**，不參與判定。
- **已知盲點：** hook 放行、且不是 rustc／Gradle worker 的工作（例如執行中的 `cargo test` 測試程式、npm／vitest），過了暖機窗就不在任何清單裡。它們吃的資源仍反映在 `load_1m` 與 `MemAvailable`，**容量數字是準的**，只是「誰在吃」看不到。要補得擴大程序掃描，不在本次範圍。

### 4.5 fleet

- **A 批就要做的最小修改（Fable B4）：** `summarize`／`pick` 讀到沒有 `slots` 的列時，不再以名額過濾，改用 `capacity`（新列）或頂層 `load_1m`／`limits.max_load`／`mem_available_mb`（舊列）判斷放不放得下。否則 A 合併後、B 合併前，`fleet pick` 對所有已換版機器回空。
- **B 批：** 全局表每台一列：CPU、記憶體總量、負載餘裕（核，近似）、可用 MB、`blocked`、暖機中、執行中工作、Claude session。執行中工作是兩個數字、不相加（B 批審查後更正）：RUN＝`gate run` 的工作數（`holders`），COMP＝編譯程序數（`compiling`）；`IDLE` 的列機器沒在回報，兩欄都是 `-`（JSON 為 `null`，「不得而知」），不當 0。`fleet pick --need-mb N --need-load X`：濾掉 `blocked` 非空與放不下的機器，依負載餘裕再依可用 MB 排序。`--kind gradle|other` 保留一個版本，對應成預設需求並提示改用新旗標（提示印的是實際生效的需求值）。
- **列齡（Fable S6）：** 上傳最多 60 秒一次，總管讀到時預留多半已過期。fleet 端在列齡超過暖機窗時，不再扣 `reserved_load`／`reserved_mb`。暖機窗是回報那台機器自己的（列裡的 `capacity.warmup_secs`；沒有這個欄位的列才用預設 60 秒）——設成 600 秒的機器，它的列到 600 秒前都還是扣過預留的。

### 4.6 量測與紀錄

- `gate run`、`gate check` 的拒絕也寫 deny log（含新欄位 `reserved_load`、`need_load`）。
- `gate usage` 加一欄平均核數（`cpu_secs / wall_secs`）；Gradle 另量整個 family 的 CPU（比照 `gradle_family_rss_mb`），作為校正 4.3 預設值的依據。
- `gate log` 讀到舊原因（`slots`、`compiler`、`compiler-lock`）的舊列，照樣彙總。

## 5. 相容性與外部使用者

- **結束碼不變**：放行 0、要等 75。agent-portal-tpm（本機有 checkout，已讀其 `dispatch/env.sh`、`regression_check.py`、`run.sh`）只看結束碼與 stderr。
- **行為變化：**
  - **派工併發不再有數量上限**；other 只受負載與（若採用 4.3）記憶體預留約束。
  - **`gate check --kind test` 變弱**：以前有 rustc 在編或 cargo 編譯鎖被佔就回 75（`gate.rs:1699-1712`），之後只看負載與記憶體。`regression_check.py` 的把關因此放寬，屬預期（用戶要拿掉編譯鎖），合併公告寫明。
  - **不同專案的 cargo 可以同時編**；同一 target 目錄由 cargo 自己的檔案鎖排隊。
  - **stderr 內容變**：`slots=…`、`compiler_lock=[…]` 消失，`gate denied (<原因>) ` 前綴不變。
- **推版順序**（Fable B4、S7）：
  1. 總管機（NUC）先換，它讀得懂新舊兩種列。
  2. 每台換版後比對 cort hook-gate 登記的執行檔路徑與 PATH 上的 `claudecat`；不同就重跑 `claudecat gate install`。`status` 在兩者不同時提示（hook 路徑是 `gate install` 當下的 `current_exe()`，`gate.rs:2298-2305`，不保證和 PATH 同一個檔）。
  3. agent-portal-tpm `dispatch/README.md` 與 `env.sh` 註解裡的「整台名額 3、有人在編譯就不開新的」，合併後在板上通知總管改。

## 6. 風險

| 風險 | 處理 |
|---|---|
| 瞬間湧入（load 延遲） | 預留負載（遞減）＋兩條路徑共用臨界區（4.3） |
| 兩個 cargo 同時編、各自吃滿核 | 第二個在暖機窗內看到第一個的預留；過窗由真實 load 擋。最壞是兩件一起慢 |
| 預留估太高卡死小機器 | 預設隨 cpus 縮放、遞減、可用環境變數調；拒絕寫 deny log 方便校正 |
| 派工記憶體爆量 | 4.3 的 other 記憶體預留 |
| 登記檔競態、pid 重用、殘檔 | 4.2 協定 |
| hook 與 gate run 不是同一個執行檔 | 推版步驟 2、status 提示 |
| 外部腳本讀 `slots` 欄位 | agent-portal-tpm 只用結束碼；其他使用者未知，合併公告列出欄位異動 |

## 7. 測試計畫

每條都要能在「實作錯」時變紅（Fable 測試缺口 1～10）：

1. **無上限：** 設 `CLAUDECAT_SLOTS=1`、`CLAUDECAT_OTHER_SLOTS=1`，同時開超過該數的 gate run，build 與 other 各測，全部放行、全部列在 holders。
2. **預留負載：** 記憶體給足，拒絕原因**恰為** `load`；邊界等於上限時放行；holder 與 admission 兩種來源各一測；hook 對 hook、gate run 對 gate run、hook 對 gate run 各一測；遞減（窗中段只計一半）；other 不吃預留負載。
3. **登記：** 執行中 holders＝1、結束後＝0，且孫程序還活著時也是 0（取代寫死舊檔名的 `locks_are_not_inherited_by_grandchildren`，`tests/gate_test.rs:1117-1143`，否則改用 glob 後迴圈跑 0 次、斷言不可能失敗）；殘檔被清且不計；0600；未來時間戳不計；非自己 uid 的檔不計。暫名→鎖→rename 的順序觀察不到，**在原始碼上斷言，並寫明這是較弱的斷言**。
4. **舊鎖：** `other-N.lock`、超出索引的 `slot-7.lock`、同一 pid 同時握 `slot-0.lock` 與 `compiler-cargo.lock` 只算一次。
5. **Gradle 倍數：** 明確的 `-PtestForks=3` 與機器預設各測，預設依 fixture 的 cpus 與 MemTotal 變。
6. **契約：** `gate check --kind other`、`gate check --kind test`、`gate run --kind other` 的 0／75 與 stderr 前綴；有 rustc 編譯中時 `check --kind test` 放行（新行為）。
7. **端到端：** 真正的 `status --json` 輸出餵進 `summarize`→`pick`，換版後的機器不會被濾掉。
8. **deny log：** gate run／check 的拒絕有寫入；含舊原因的舊列照樣彙總。
9. **故意改壞清單**，每項確認變紅：拿掉 `reserved_load`、拿掉 `need_load`、不列舉 holders、不讀 admissions、gate run 不進臨界區、other 吃預留負載。

## 8. 審查紀錄

### 作者自審（Opus）

1. v0 說「混版期間舊版看不到新版的工作」；單機的混版只存在於換版前就起動的 gate run。已改 4.2。
2. 補上 hook 放行工作在暖機窗後的可見性盲點（4.4）。

### 他審（Fable，APPROVE WITH CHANGES），Opus 逐條核實

| 編號 | 結論 | 處理 |
|---|---|---|
| B1 cpu_mean 是 CPU 秒數不是百分比 | 成立（`gate_stats.rs:637-646` 的 `cpu_secs_mean`）；v1 讀錯 | 4.3 預設值改為暫定並重訂依據，4.6 補量測 |
| B2 登記檔先建後鎖的競態、pid 重用 | 成立 | 4.2 協定 |
| B3 gate run 的檢查與登記沒有序列化 | 成立（`run()` 不拿 `admission.lock`） | 4.3 共用臨界區 |
| B4 拿掉 slots 後 fleet pick 回空 | 成立（`fleet_view.rs:237-241` 要求 total > used） | 4.5 最小修改併入 A，5 推版順序，7-7 端到端測試 |
| S1 證據只涵蓋 hook | 成立（`record_deny` 只在 hook） | 1 限定說法，4.6 補紀錄 |
| S2 need 為 0 的工作要不要吃預留負載 | 成立 | 4.1：other 不吃 |
| S3 `check --kind test` 變弱 | 成立 | 5 列入行為變化 |
| S4 cargo 固定 2.0、預留階梯式 | 成立 | 4.3 隨 cpus 縮放、線性遞減 |
| S5 other 無併發界線、記憶體風險 | 成立 | 4.3，用戶確認採用 |
| S6 容量數字誤導 pick、未來時間戳、權限 | 成立 | 4.4 `blocked`、4.5 列齡、4.2 第 6、8 點 |
| S7 hook 與 gate run 不保證同一執行檔 | 成立（`gate install` 存 `current_exe()`） | 5 推版步驟 2 |
| Nit | 採納 | 3 現況改寫；4.1 數量更正；4.4 平台差異與 null |

## 9. 分批

- **A1：** 放行規則與登記：4.1、4.2、4.3（含共用臨界區、預留負載、other 記憶體預留）＋對應測試。
- **A2：** 回報與量測：4.4、4.6 ＋ fleet 最小修改（4.5 第一點）＋對應測試。A1、A2 合完才一起推各機（中間不推，避免 pick 回空）。
- **B：** fleet 全局表與 pick（4.5 其餘）＋README／SKILL.md。
- 每批：GLM 實作 → codex 審 → Opus 核實 → 合 main → 依第 5 節順序推各機。
