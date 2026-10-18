# Industrial PLC DataService (PlcDataService v1.0)

> 工業級 PLC 資料服務層執行期引擎 (Single Source of Truth, SSOT)  
> 依據《工業級 PLC 資料服務層執行期規格書 (DATA_SERVICE_SPEC.md v1.0)》與 `dataservice.md` 完整還原之專案。

---

## 1. 專案架構概覽 (Architecture)

本專案遵守 `#![forbid(unsafe_code)]`，向上對接 Tauri Desktop App (`src-tauri` In-Process IPC) 與外部 SCADA / MES / Web HMI 系統，向下整合 `machine-model` (§9.3 寫入護欄) 與 `mm-drivers` (底層硬體通訊協議)：

```
┌────────────────────────────────────────────────────────────────────────┐
│                        應用展示層 (HMI / Web / MES)                     │
│               唯讀快取、事件訂閱、語意寫入 (feeder.speed = 1500)            │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ IPC / WebSocket / REST (Tag-based)
┌───────────────────────────────────▼────────────────────────────────────┐
│              資料服務層 (crates/plc-data-service)                      │
│  ┌──────────────────────┐  ┌─────────────────┐  ┌──────────────────┐   │
│  │ 即時快取 (TagStore)  │  │ 雙軌動態訂閱器  │  │ 寫入仲裁與護欄   │   │
│  │ (Value/Quality/Time) │  │ (Permanent/View)│  │ (MachineModel§9.3)│  │
│  └──────────────────────┘  └─────────────────┘  └──────────────────┘   │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │       智慧相鄰拼包與優先級排程引擎 (Coalescing & Priority Poller)       │  │
│  └──────────────────────────────────────────────────────────────────┘  │
└───────────────────────┬───────────────────────────────▲────────────────┘
                        │ 載入點位映射與護欄規則           │ 呼叫區塊讀寫 (Batch I/O)
┌───────────────────────┴────────────────────────┐ ┌────┴─────────────────┐
│ 機台語意模型 (crates/machine-model §9.3)        │ │ 底層驅動 (mm-drivers)  │
│ (machine.yaml + unit-types + service.toml)     │ │ 56種硬體 / TCP / 485  │
└────────────────────────────────────────────────┘ └──────────────────────┘
```

---

## 2. 還原檔案清單 (Restored Files)

```
DataService/
├── Cargo.toml                              # Workspace 根設定檔
├── DATA_SERVICE_SPEC.md                    # 工業級 PLC 資料服務層執行期規格書
├── dataservice.md                          # 原始規格與程式碼對照文件
├── plc_data_service_workbench_demo.html    # 全功能視覺化測試與調校工作台 DEMO
├── README.md                               # 專案說明文件
├── crates/
│   ├── machine-model/                      # 機台語意模型與 §9.3 護欄規則
│   ├── mm-drivers/                         # 56 種工業通訊協議驅動核心
│   └── plc-data-service/                   # 資料服務層執行期核心 Crate
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs                      # 包含五大核心子模組與整合測試
├── src-tauri/                              # Tauri 2.0 桌面應用與 IPC 介面層
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── datasvc_commands.rs             # §8.1 四大核心 IPC 指令與診斷介面
│       ├── lib.rs
│       └── main.rs
└── ui/
    ├── index.html                          # Tauri 前端入口 (與工作台 DEMO 同步)
    └── plc_data_service_workbench_demo.html
```

---

## 3. 六大核心機制落實對照

| 規格章節 | 核心機制 | 實作位置 | 關鍵特性與設計 |
| :--- | :--- | :--- | :--- |
| **§1.3** | **混合拓撲與單一硬體所有權** | `src/lib.rs`, `src-tauri` | 支援 Tauri 同進程直連（`< 0.1 µs`）與無頭 Daemon 模式；避免 COM 連接埠鎖定衝突。 |
| **§2 & §8.2** | **即時快取與死區變化推播 (COS)** | `cache` 模組 | 納秒時間戳 (`timestamp_ns`)、原子版本號 (`version`)、`DataQuality` (Good/Bad/Uncertain/Stale)；支援絕對值與比例死區過濾 (Zero-Waste)。 |
| **§3 & §6** | **寫入仲裁、四大護欄與插隊預佔** | `arbiter` 模組 | 護欄 1 (唯讀點位攔截)、護欄 2 (安全監控點位阻擋)、護欄 3 (上下限超界攔截)、護欄 4 (安全審計日誌)。寫入推入 `Priority::Urgent` 插隊佇列，§6.3 離線 Fail-Fast 阻擋（嚴禁離線排隊）。 |
| **§4** | **雙軌分層動態訂閱架構** | `subscription` 模組 | 軌道 1 (Permanent 永久常駐軌，急停/警報永不退訂)；軌道 2 (Viewport 動態視角軌，具備 `ref_count` 引用計數、3 秒退訂冷卻計時器、首幀 Burst 就緒防護與殭屍連線自動回收)。 |
| **§5** | **間隙容忍相鄰拼包演算法** | `poller` 模組 | 實作 $A_{k+1} - (A_k + \text{len}_k) \le \text{MaxGap}$ 相鄰暫存器拼包（RS-485 預設 MaxGap=4, MaxBlock=64；TCP 預設 MaxGap=16, MaxBlock=200），減少總線往返次數與通訊延遲。 |
| **§7** | **緊湊逾時與單站熔斷器** | `gateway` 模組 | RS-485 緊湊逾時（理論耗時 + 25ms）；連續 3 次逾時觸發 `TrippedDegraded` 隔離狀態，降頻為 5000ms 探測一次，自動標記點位為 `DataQuality::Bad`，保護同總線其他正常站點。 |

---

## 4. 上層 IPC 通訊契約 (`src-tauri`)

`src-tauri/src/datasvc_commands.rs` 提供五大核心指令：
1. `datasvc_get_snapshot`: 讀取單點或多點記憶體快照（`< 1 µs` 直讀）。
2. `datasvc_enter_view`: 動態訂閱視角畫面點位，重置退訂冷卻計時器並排入 Burst 首幀優先佇列。
3. `datasvc_leave_view`: 離開畫面時啟動 3 秒退訂冷卻倒數計時器，防範畫面頻繁切換之重組震盪。
4. `datasvc_write_tag`: 執行具備四大護欄攔截與 Priority::Urgent 插隊預佔之語意寫入。
5. `datasvc_get_diagnostics`: 供開發者與診斷面板獲取雙軌訂閱引用計數與相鄰拼包區塊資訊。

---

## 5. 測試與驗證 (Verification)

### 執行 Rust 單元與整合測試
```bash
cargo test -p plc-data-service
```
測試項目包括：
- `test_gap_tolerant_coalescing_merges_nearby_registers`: 驗證相鄰暫存器拼包演算法正確跨越未訂閱間隙。
- `test_two_tier_subscription_and_3s_cooldown`: 驗證雙軌訂閱、3 秒退訂冷卻與永久常駐軌保護機制。
- `test_write_guardrails_and_fail_fast_offline_protection`: 驗證唯讀攔截、上下限超界攔截、安全點保護與單站熔斷離線 Fail-Fast 阻擋。

### 執行編譯檢查
```bash
cargo check
```

### 開啟視覺化工作台 DEMO
直接在瀏覽器中開啟 `plc_data_service_workbench_demo.html`：
- 100% 離線內嵌樣式 (ISA-101 深色工業風，無外網 CDN 依賴)。
- 提供即時 Tag 快照、動態視角切換、3 秒冷卻倒數、拼包排程可視化、四大護欄測試、單站熔斷模擬及 API Contracts 測試器。
