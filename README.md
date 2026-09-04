# ClaudeCat V2 — Project Navigation Map（給 Claude Code 的導航地圖）

**引導型**：ClaudeCat 在啟動時就給 Claude Code 一張**可信任的專案地圖**
（入口、模組樹、主要符號、依賴），讓它導航時**不用盲目摸索**。
與 **cortexyoung/cort**（協助型，插在 rg 上的搜尋)互補：
cort 回答「查詢時的問題」，ClaudeCat 回答「啟動時的導航」。

## 定位（V2，2026-09 重定位）

- ❌ **不做**：「猜測式」auth/response/error pattern 偵測 + 假信心
- ✅ **只做**：從 manifest + AST 產出**可驗證的事實地圖**
- 對齊 Anthropic 官方大型 codebase 策略（agentic search > RAG）：
  給 Claude 精簡地圖（≤150 行），讓它自己做有目標的探索

## 安裝 / 建置

```bash
cargo build --release            # 產生 target/release/claudecat（單一二進位）
```

## 使用

```bash
claudecat scan                   # 輸出導航地圖（markdown）
claudecat scan --format json     # JSON 輸出
claudecat scan --format text     # 純文字輸出
claudecat update                 # 更新 CLAUDE.md 的 claudecat 自動區塊（原子寫入）
claudecat update --dry-run       # 只看會不會變，不寫入
claudecat explore                # 量化探索成本（地圖 token vs 全讀 token）
claudecat scan --root /path/to/project
```

### 對 Claude Code 的使用建議

1. 在專案根目錄執行 `claudecat update`（或讓 skill/CI 定期執行）
2. CLAUDE.md 自動維護 `<!-- claudecat:auto:begin -->` 區塊
3. Claude Code 啟動時自動載入，導航零成本

## 導航地圖內容（全部是事實）

| 區塊 | 來源 |
|---|---|
| 專案類型 / 語言 / 框架 / 套件管理器 | manifest（package.json / Cargo.toml / pyproject.toml / go.mod） |
| Entry points / Run / Build | manifest 宣告 |
| 目錄結構（檔案數、LOC） | gitignore-aware walk（`ignore` crate） |
| 主要檔案 & 符號（fn/class/struct/…） | tree-sitter AST |
| Dependencies（宣告） | manifest |

- 支援語言：JS/TS、Python、Rust、Go、C/C++（tree-sitter grammar）
- 自動排除：node_modules、target、dist、.git、legacy 等

## 技術決策 Guardrails（開發者維護，永不覆寫）

`claudecat update` 首次執行會在 CLAUDE.md 附加：

```markdown
<!-- claudecat:guardrails:begin -->
- 2D tilemap + Macroquad（禁 Python/3D）  ← 例：一行一條技術決策
<!-- claudecat:guardrails:end -->
```

此區塊**只存在時不動、不存在才建立**，開發者自由編輯；`scan`/`explore` 會讀出
顯示。也可用根目錄 `claudecat-guardrails.md` 優先提供。
真實案例佐證：GalaxyWarHero session 中 Claude 因缺「技術決策」把 2D 專案當 3D
分析、裝錯工具——見 [SESSION-EVIDENCE.md](SESSION-EVIDENCE.md)。

## 誠實原則（修復 V1 假信心）

- 只報可驗證事實，明確標 `*Generated at …* no inference`
- 偵測不到就空白，不標「100% High Confidence」
- CLAUDE.md 更新為原子寫入（temp + rename），無變動不寫

## V1 歷史

V1 是 TypeScript MCP server（「主動偵測 pattern + 信心分數」），因假信心與
誤判問題於 V2 重定位；舊程式碼封存於 [legacy/](legacy/)，研究說明見
[RESEARCH-V2.md](RESEARCH-V2.md)、[CLAUDECAT-GOALS.md](CLAUDECAT-GOALS.md)。

## 授權

MIT
