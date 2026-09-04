# 真實 Claude Code Session 證據（2026-09）

來源：本機 `~/.claude/projects/**/*.jsonl`（38 個真實 session，跨 6 個專案：
cortexyoung、claude-code-router、GalaxyWarHero、empireE、persona-core 等）。
目的：驗證「導航地圖」假設——Claude Code 因缺專案結構/決策 context 而犯錯、繞路、問蠢問題。

## 量測：探索成本佔比（explore% = Bash+Read+Glob+Grep+Write 佔全部工具數）

| session | 訊息數 | 工具數 | 探索% | 使用者糾正 |
|---|---|---|---|---|
| GalaxyWarHero 66a0c2f6 | 567 | 657 | 64% | 13 |
| cortexyoung 03faa8e3 | 64 | 162 | 87% | 1 |
| claude-code-router c3e99ec6 | 76 | 206 | 84% | 2 |
| claude-code-router dc586b54 | 41 | 33 | 93% | 0 |
| cortexyoung 5e92b5d3 | 11 | 55 | 96% | 0 |
| claude-code-router 973ef69c | 36 | 57 | 70% | 1（AskUserQuestion×6） |

**結論**：真實工作 session 中 **64–96% 的工具呼叫花在「找檔案/讀檔/跑指令確認結構」**，
這些正是導航地圖（入口+模組樹+符號+命令）能直接省下的成本。

## 真實案例

### 案例 A：把 2D 回合制戰略遊戲當 3D 渲染專案分析（GalaxyWarHero）
Claude 連續 3 輪誤判專案本質：
- ASM「**需要 3DGS/Blender/CUDA/photo-grammetry 產線**」
- USER「我們沒有3D, 技術書已經選m開頭技術做為rust 渲染引擎,你不要亂做」
- USER「這是個類empire delux的遊戲,不是什麼射擊遊戲」
- USER「cuda應該只是給GS用的...也不該有任何python」
- ASM「我這一路錯得離譜...我把一個『玩法支柱才是重點、畫面只是色塊』的遊戲,當成需要3DGS內容產線的專案在分析——這是根本性的判斷錯誤」

> **地圖能防**：`### 技術決策: 2D tilemap + Macroquad（禁 Python/Blender/3DGS）`
> 一行 guardrail 就省掉數十輪架構辯論。

### 案例 B：裝錯工具（GalaxyWarHero）
- ASM 在 grok 的 marketplace 找插件
- USER「你錯邊了,我們要在claude code裏面裝, 第一部要先設market,不是在grok裏裝」

> **地圖能防**：`### 工具鏈: 插件一律裝在 Claude Code 內`

### 案例 C：session 共享理解錯誤（claude-code-router c3e99ec6）
- USER「不是這樣,是連session都沒看到,沒共享」

### 案例 D：問「哪個專案需要 index / hook 形狀」（cortexyoung 03faa8e3）
- AskUserQuestion「Index the 4 unindexed projects now (persona-core, multiverse-wonder, GalaxyWarHero, four-block-puzzle)?」
- AskUserQuestion「How should I proceed on confirming the exact Codex TOML hook shape before implementing?」

> **地圖能防**：`cort projects` 的等價資訊直接進 CLAUDE.md，不必問。

### 案例 E：殘留 symlink 靠考古才知道（claude-code-router dc586b54）
- AskUserQuestion「acode, kcode, gccode are dangling symlinks left over from before the Rust port (renamed per HISTORY.md, not in providers.toml). Remove them?」
- Claude 必須自己翻 HISTORY.md + providers.toml 才拼出結構。

## 給 ClaudeCat 的回饋（設計結論）

1. **地圖要包含「技術決策/guardrail」區**——真實案例 A/B 顯示純符號清單不夠，
   需要一行一行可寫進 CLAUDE.md 的決策聲明（由使用者維護或偵測到就列）。
2. **地圖要包含部署/工具鏈命令**——案例 B/C/E 全是「不知道哪裡該怎麼做」。
3. **探索成本可量測**——已實作 `claudecat explore`（量化地圖 token vs 全讀 token）
   與 `claudecat track <file>`（把指標寫進本文件的「長期指標」表，原子更新，
   同日同專案不重複）。

## 長期指標 (claudecat explore)

| 日期 | 專案 | 檔案 | LOC | map tokens | 全讀 tokens | 節省% | top-10 覆蓋% | 覆蓋上限% |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 2026-09-04 | `/home/yanggf/a/claudecat` | 17 | 1732 | ~911 | ~10392 | 91.2% | 86.5% | 100% |
