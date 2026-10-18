# 工業級 PLC 資料服務層執行期規格書 (PlcDataService Specification v1.0)

> **文件狀態**：正式草案 (Draft for Implementation)  
> **適用模組**：`crates/plc-data-service` (即將建立之核心執行期服務庫)  
> **上游依賴**：[`crates/mm-drivers`](file:///mnt/c/Users/wydct/orca/projects/PLCDriver/crates/mm-drivers/src/lib.rs) (v1.0)、[`crates/machine-model`](file:///mnt/c/Users/wydct/orca/projects/PLCDriver/crates/machine-model/src/lib.rs) (v0.3 §9.3)  
> **下游端點**：Tauri Desktop App (`src-tauri`)、Web HMI / SCADA 儀表板、邊緣運算 / MES 介接

---

## 目錄 (Table of Contents)
1. [架構定位與設計哲學](#1-架構定位與設計哲學)
2. [核心型別與抽象模型](#2-核心型別與抽象模型)
3. [語意點位引擎與機台模型整合 (MachineModel Binding)](#3-語意點位引擎與機台模型整合-machinemodel-binding)
4. [雙軌分層動態訂閱架構 (Two-Tier Subscription Architecture)](#4-雙軌分層動態訂閱架構-two-tier-subscription-architecture)
5. [智慧拼包排程器 (Register Packing & Coalescing Engine)](#5-智慧拼包排程器-register-packing--coalescing-engine)
6. [寫入仲裁與插隊協議 (Preemptive Write & Verification Protocol)](#6-寫入仲裁與插隊協議-preemptive-write--verification-protocol)
7. [連線狀態機、斷線容錯與熔斷機制 (Fault Tolerance & Circuit Breaker)](#7-連線狀態機斷線容錯與熔斷機制-fault-tolerance--circuit-breaker)
8. [上層應用通訊介面契約 (API Contracts: IPC / WebSocket / REST)](#8-上層應用通訊介面契約-api-contracts-ipc--websocket--rest)
9. [專案模組架構與工程落實路徑](#9-專案模組架構與工程落實路徑)

---

## 1. 架構定位與設計哲學

### 1.1 核心使命
`PlcDataService` 是介於「實體驅動層（`mm-drivers`）」與「業務應用層（HMI / SCADA / AI）」之間的**單一數據真理來源（Single Source of Truth, SSOT）**。

上層應用**嚴禁直接接觸底層驅動與實體暫存器位址**（如 `D100`、`M10`、`DB1.DBD4`），所有資料交換必須透過本服務以**語意點位標籤（Semantic Tag ID）**進行。

```
┌────────────────────────────────────────────────────────────────────────┐
│                        應用展示層 (HMI / Web / MES)                     │
│               唯讀快取、事件訂閱、語意寫入 (feeder.speed = 1500)            │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ IPC / WebSocket / REST (Tag-based)
┌───────────────────────────────────▼────────────────────────────────────┐
│              資料服務層 (crates/plc-data-service)                      │
│  ┌──────────────────────┐  ┌─────────────────┐  ┌──────────────────┐   │
│  │ 即時快取 (DataStore) │  │ 雙軌動態訂閱器  │  │ 寫入仲裁與護欄   │   │
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

### 1.2 設計準則
1. **100% 安全 Rust**：全專案遵守 `#![forbid(unsafe_code)]`，嚴禁任何未定義行為。
2. **通訊總線保護**：針對半雙工 RS-485 與低頻寬乙太網，落實「最大連續拼包」與「插隊預佔」，徹底杜絕總線塞車。
3. **安全本質優先**：貫徹 `machine-model` §9.3 定義之可寫性判定與護欄，任何軟體異常寫入均在服務層被直接攔截。
4. **零等待前端體驗**：所有讀取操作直接命中記憶體快取（`< 1 µs`），前端畫面恆定維持 60 FPS 流暢度。

### 1.3 執行期架構拓撲：混合模式 (Hybrid Mode)
本服務採用雙重發布拓撲，兼顧單機極簡與工廠聯網擴展：
1. **單機桌面模式 (In-Process Embedded Library)**：
   * `crates/plc-data-service` 作為 Rust Library 直接靜態連結至 Tauri 應用程式。
   * UI 啟動時在同進程背景自動運行，記憶體直通存取（延遲 < 0.1 µs），現場單機交付為單一執行檔，零環境相依。
2. **無頭微服務模式 (Headless Daemon Mode)**：
   * 可獨立編譯為二進位守護行程（`cargo run -p plc-data-service -- --serve --port 8080`）。
   * 開放 WebSocket 與 REST API，支援多個 Web 瀏覽器、平板 HMI 或 MES 系統共享同一個機台連線。
3. **單一硬體所有權與防衝突 (Single Hardware Ownership & Auto-Detection)**：
   * 永遠只有 DataService 能夠開啟實體 COM / TCP 通訊埠。
   * Tauri UI 啟動時自動探測本機是否已有獨立 Daemon；若已有，UI 自動作為 Client 掛載，避免產生 `COM port already in use` 衝突。

---

## 2. 核心型別與抽象模型

### 2.1 點位快照 (`TagSnapshot`)
快取中的每一個 Tag 均以原子或並發讀寫結構儲存：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSnapshot {
    /// 語意標籤名稱 (例如: "feeder.spindle.actual_speed")
    pub tag_id: String,
    /// 當前數值
    pub value: ScalarValue,
    /// 資料品質
    pub quality: DataQuality,
    /// 驅動層原始擷取時間戳記 (UNIX Epoch Nanoseconds)
    pub timestamp_ns: u64,
    /// 本地連續變化版本號 (每次數值更新 +1，供前端比對)
    pub version: u64,
}
```

### 2.2 資料品質列舉 (`DataQuality`)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataQuality {
    /// 數據有效，硬體通訊正常
    Good,
    /// 設備斷線、Socket 拒絕或逾時，數值為最後已知無效值
    Bad,
    /// 資料暫時無法保證精確 (如初次連線建立中、感測器校正中)
    Uncertain,
    /// 通訊正常但超過設定之陳舊門檻未獲得更新 (如低頻輪詢暫停)
    Stale,
}
```

### 2.3 變化發布事件 (`TagChangeEvent`)
當數值超越死區（Deadband）或品質改變時觸發推播：
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagChangeEvent {
    pub tag_id: String,
    pub old_value: Option<ScalarValue>,
    pub new_value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp_ns: u64,
}
```

---

## 3. 語意點位引擎與機台模型整合 (MachineModel Binding)

### 3.1 模型載入與映射建構
服務啟動時載入 [`crates/machine-model`](file:///mnt/c/Users/wydct/orca/projects/PLCDriver/crates/machine-model/src/lib.rs) 編譯產出之 `ResolvedMachineModel`：
1. **Tag 路由表**：將 `Tag ID` 綁定至 `(driver_id, register_address, data_type, byte_order)`。
2. **死區過濾 (Deadband)**：支援浮點數絕對差額 (`abs_delta`) 與百分比 (`ratio`) 變化過濾。

### 3.2 嚴格落實 §9.3 四大寫入護欄
當上層發起 `write_tag(tag_id, value)` 時，服務層必須在下發 Driver 之前完成四大校驗：
```rust
// 呼叫 machine_model::datasvc::evaluate_plc_data_service_writability
let report = evaluate_plc_data_service_writability(&resolved_model, &service_overrides);
```
* **護欄 1：唯讀點位攔截**：模型標記為不可寫（`final_writable == false`，如純輸入接腳 `field.in`）時，直接拒絕寫入。
* **護欄 2：安全點位覆寫阻擋**：命中 `man_*`、`par_*` 等安全與模式接腳時，強制阻擋未經審查的覆寫。
* **護欄 3：數值範圍與型別檢查 (Range Guard)**：根據點位定義之上下限（如 `min: 0.0, max: 3000.0`），超限直接拋出 `OutOfBoundsError`。
* **護欄 4：安全審計日誌 (Audit Trail)**：所有寫入操作記錄 `(operator_id, client_ip, tag_id, old_val, new_val, timestamp)`。

---

## 4. 雙軌分層動態訂閱架構 (Two-Tier Subscription Architecture)

針對 RS-485 及受限頻寬工業網路，本規格建立**雙軌分層訂閱機制**，徹底解決「全局全時輪詢導致總線塞車」與「動態退訂導致警報失明」的根本矛盾。

```
                 總訂閱點位集合 (Subscribed Registry)
                                │
        ┌───────────────────────┴───────────────────────┐
        ▼                                               ▼
 【軌道 1：永久常駐軌 (Permanent)】             【軌道 2：動態視角軌 (Viewport-Only)】
 ────────────────────────────────               ───────────────────────────────────
 • 機台急停、光柵、安全互鎖點位                  • 當前 HMI 畫面的調機指示燈與點動按鈕
 • 系統總警報觸發點 (Alarm Bits)                 • 儀表刻度盤、非關鍵製程數據
 • 生產累計數、稼動狀態                          • 即時單動氣缸到位指示
 • 歷史資料記錄器 (Data Logger)                  • 支援 3 秒退訂冷卻 (Cooldown)
 ★ 永不退訂，生命週期同系統運作                   ★ 隨 UI 畫面進入/離開動態掛載與卸載
```

### 4.1 軌道 1：永久常駐軌 (Permanent Track)
* **包含點位**：由 `machine.yaml` 標記為 `system_critical: true`，或背景警報服務、MES 拋轉引擎所註冊的點位。
* **排程行為**：系統開機即常駐背景輪詢，**任何前端頁面切換或連線中斷均不可退訂此軌道**。

### 4.2 軌道 2：動態視角軌 (Viewport Track) 與生命週期
* **客戶端動態註冊**：前端畫面載入時發送 `subscribe_viewport(client_id, view_id, tag_list)`。
* **引用計數管理 (Reference Counting)**：同一個 Tag 若被多個開啟中的視窗引用，維護 `ref_count`；只有當 `ref_count == 0` 時才觸發退訂流程。
* **3 秒退訂冷卻計時器 (3s Cooldown Timer)**：
  * 當畫面關閉觸發 `unsubscribe_viewport` 時，服務層**不立即刪除點位**，而是啟動 3 秒倒數計時。
  * 若使用者在 3 秒內快速切回該頁面，計時器重置，避免頻繁拼包重組引發的總線抖動。
* **首幀就緒防護機制 (Readiness Guard)**：
  * 視角剛訂閱時，該批點位初始品質為 `Uncertain`。
  * 排程器將此視角的第一輪採樣設為 `Burst Priority`（優先插隊掃描一次）。
  * 首次採樣成功後，發送 `ViewportReady` 事件，前端解鎖操作按鈕。
* **殭屍連線自動回收 (Zombie Auto-GC)**：
  * 服務端透過 WebSocket / IPC 管道偵測連線生命週期。
  * 若前端進程崩潰或通道關閉，自動釋放該 `client_id` 底下所有動態訂閱，杜絕頻寬洩漏。

---

## 5. 智慧拼包排程器 (Register Packing & Coalescing Engine)

### 5.1 間隙容忍相鄰拼包演算法 (Gap-Tolerant Coalescing)
底層序列埠與 TCP 協議的開銷主要在於 Frame Header、交握延遲與設備響應時間。排程器必須對同一個 PLC 設備的暫存器進行合併：

1. **排序點位**：將當前需輪詢的暫存器位址依號碼遞增排序：$A_0, A_1, A_2, \dots, A_n$。
2. **間隙計算 (Gap)**：若相鄰暫存器距離 $A_{k+1} - (A_k + \text{len}_k) \le \text{MaxGap}$（預設 RS-485 為 4 個 Word，TCP 為 16 個 Word）：
   * 將其合併至同一區塊。
   * 即使中間的暫存器未被任何 Tag 訂閱，合併讀取的總耗時仍遠小於拆成兩次通訊請求！
3. **區塊上限限制 (Max Block Size)**：
   * Modbus RTU / 三菱 FX：單包最大 64 ~ 120 Words。
   * Siemens S7 / 以太網 MC：單包最大 200 ~ 500 Words。
   * 超過上限時自動分裂為下一組連續區塊。

### 5.2 多速率分群佇列 (Multi-Rate Scheduling Queues)
排程器維護三級時間輪（Timing Wheel）：
* **Queue 1 (High Rate: 50 ~ 100ms)**：急停、高頻伺服座標、目前動態畫面之首幀。
* **Queue 2 (Normal Rate: 200 ~ 500ms)**：一般運轉狀態、氣缸感測器、動態視角普通點位。
* **Queue 3 (Low Rate: 1000 ~ 3000ms)**：溫度、累積產能、非即時配方與環境監控。

---

## 6. 寫入仲裁與插隊協議 (Preemptive Write & Verification Protocol)

### 6.1 最高優先權插隊通道 (Write Preemption)
在半雙工通訊中，寫入指令絕不能排在幾十個輪詢任務後方等待：
1. **專屬優先佇列**：寫入請求推入 `Priority::Urgent` 佇列。
2. **當前幀等待**：正在線路上傳輸的單一讀取幀不可中斷（避免總線數據損壞）；但該幀一完成回傳，排程器**立刻暫停後續讀取，優先執行寫入**。
3. **下發與確認**：
   * 呼叫 `mm-drivers` 執行寫入。
   * 接收硬體回傳之 ACK（如 Modbus 功能碼 05/06/15/16 回執）。
   * 收到 ACK 即確認寫入成功，更新快取，通知前端操作成功。

### 6.2 零冗餘回讀原則 (No Redundant Immediate Read)
* **嚴禁在單點按鈕寫入後立刻補發 `read_single` 命令**（會造成 RS-485 頻寬翻倍耗費）。
* **狀態同步交由既有背景輪詢自然吸收**：由下一輪高頻/中頻排程更新物理感測器回饋。
* **例外機制**：整批配方（Recipe）寫入時，提供選配的 `verify_batch` 選項，在全批寫完後以單次 Batch Read 進行一次性一致性校驗。

### 6.3 斷線異常寫入決策：即時拋錯阻擋 (Fail-Fast Policy)
* 若目標設備處於斷線、通訊逾時或熔斷隔離（`DataQuality::Bad`），任何上層寫入請求**立刻回傳 `DeviceOfflineError` 失敗報錯**。
* **嚴禁在離線期間進行寫入指令排隊 (No Offline Queue)**：防止通訊連線恢復瞬間突發未預期的物理動作（如累積的啟動、汽缸動作指令瞬間爆發下發），徹底杜絕產線工安事故。

---

## 7. 連線狀態機、斷線容錯與熔斷機制 (Fault Tolerance & Circuit Breaker)

```
        ┌──────────────┐      連線成功      ┌─────────────┐
        │ Disconnected │ ─────────────────> │ Connected   │
        └──────────────┘                    └──────┬──────┘
               ▲                                   │ 連續 3 次逾時
               │ 退避逾時結束                       ▼
        ┌──────┴───────┐                    ┌─────────────┐
        │  Backoff /   │ <───────────────── │  Degraded / │
        │  Reconnecting│     降頻重試失敗    │  Tripped    │
        └──────────────┘                    └─────────────┘
```

1. **緊湊逾時 (Tight Timeout)**：
   * RS-485 逾時設為 `理論耗時 + 25ms`（如 50ms），禁止使用預設的 1000~2000ms。
2. **單站熔斷 (Station Circuit Breaker)**：
   * 多站 RS-485 系統中，若 Station 3 斷電，連續 3 次無響應後進入 `Degraded` 狀態。
   * 熔斷器將 Station 3 隔離，輪詢週期降為 5000ms 探測一次，**防止單一離線設備拖垮整個總線上其他正常站點**。
3. **資料陳舊標記 (Stale Marking)**：
   * 設備斷線後，快取數值不清除，但品質立刻轉為 `DataQuality::Bad`，前端儀表呈現反灰警告。

---

## 8. 上層應用通訊介面契約 (API Contracts)

### 8.1 本地端 Tauri IPC 指令 (`src-tauri`)
```rust
// 1. 讀取單點或多點快照 (Direct Memory Read, < 1 µs)
#[tauri::command]
async fn datasvc_get_snapshot(tags: Vec<String>) -> Result<HashMap<String, TagSnapshot>, String>;

// 2. 視角進入 (動態訂閱)
#[tauri::command]
async fn datasvc_enter_view(client_id: String, view_id: String, tags: Vec<String>) -> Result<(), String>;

// 3. 視角離開 (啟動 3 秒退訂冷卻)
#[tauri::command]
async fn datasvc_leave_view(client_id: String, view_id: String) -> Result<(), String>;

// 4. 具備護欄的語意寫入
#[tauri::command]
async fn datasvc_write_tag(tag: String, value: serde_json::Value, operator: String) -> Result<WriteResult, String>;
```

### 8.2 變化推播策略 (Change-of-State / Deadband Event Stream)
* **狀態變化才推播 (Change-of-State, COS)**：
  * 布林點位：狀態 0/1 翻轉時立即觸發推播。
  * 浮點/整數點位：超過設定之死區閾值（Deadband，如絕對值或百分比）時才觸發推播。
  * 品質改變：如通訊斷線由 `Good` 轉為 `Bad`，立即廣播通知前端反灰或告警。
* **無變化不推播 (Zero-Waste)**：若數值維持在死區範圍內，背景排程器僅安靜更新內部 `TagStore`，完全不發送 IPC/WebSocket 事件，徹底消除前端 DOM 的無謂重繪。
* **事件 Payload 規格**：
  前端監聽單一 WebSocket 或 Tauri Event `datasvc://tag_changed`：
```json
{
  "event": "tag_changed",
  "data": {
    "tag_id": "feeder.spindle.speed",
    "value": 1500.5,
    "quality": "Good",
    "timestamp_ns": 1729180000000000,
    "version": 42
  }
}
```

---

## 9. 專案模組架構與工程落實路徑

### 9.1 新增 Crate 檔案結構規劃
建議在 workspace 建立獨立的資料服務層 crate：
```
crates/plc-data-service/
├── Cargo.toml
└── src/
    ├── lib.rs                  # 服務對外公開 API 與匯出
    ├── cache/
    │   ├── mod.rs              # In-Memory TagStore 實作
    │   └── snapshot.rs         # TagSnapshot 與 DataQuality 定義
    ├── subscription/
    │   ├── mod.rs              # 雙軌訂閱管理器 (SubscriptionManager)
    │   ├── tracker.rs          # 引用計數與 3 秒退訂冷卻定時器
    │   └── events.rs           # Tokio Broadcast / Event 推播發布
    ├── poller/
    │   ├── mod.rs              # 輪詢協調器 (PollerCoordinator)
    │   ├── coalescing.rs       # 間隙容忍暫存器相鄰拼包演算法
    │   └── scheduler.rs        # 多速率時間輪佇列 (Multi-rate Scheduler)
    ├── arbiter/
    │   ├── mod.rs              # 寫入仲裁器 (WriteArbiter)
    │   └── guardrails.rs       # 對接 machine-model §9.3 護欄檢查
    └── gateway/
        ├── mod.rs              # 驅動管理整合介面 (對接 mm-drivers)
        └── circuit_breaker.rs  # 熔斷與逾時容錯控制器
```

### 9.2 開發里程碑 (Milestones)
* **Phase 1: TagStore 快取與機台模型綁定**（實現語意 Tag 到暫存器位址映射，落實唯讀防呆）。
* **Phase 2: 相鄰拼包與背景排程器**（實現 Gap-Tolerant Coalescing 與 RS-485 優先插隊寫入）。
* **Phase 3: 雙軌動態訂閱與冷卻機制**（實現 Viewport 訂閱、引用計數與 3 秒延遲退訂）。
* **Phase 4: Tauri / Web GUI 整合**（全面替換現有的直接暫存器呼叫，達成 60 FPS 流暢度）。
