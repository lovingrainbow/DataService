# Industrial PLC DataService (PlcDataService) 使用手冊 (USAGE)

> **版本**：v1.0.0  
> **適用專案**：`DataService` (包含 `plc-data-service`, `datasvc-demo`, `src-tauri`, `machine-model`, `mm-drivers`)  
> **標準規範**：遵循《工業級 PLC 資料服務層執行期規格書 (DATA_SERVICE_SPEC.md v1.0)》

---

## 目錄 (Table of Contents)
1. [專案架構與定位](#1-專案架構與定位)
2. [專案目錄與模組職責](#2-專案目錄與模組職責)
3. [快速開始 (Quick Start)](#3-快速開始-quick-start)
   - [3.1 編譯與全域測試](#31-編譯與全域測試)
   - [3.2 執行規格逐項驗證報告](#32-執行規格逐項驗證報告)
   - [3.3 啟動終端互動式 HMI](#33-啟動終端互動式-hmi)
   - [3.4 啟動無頭 Daemon 服務](#34-啟動無頭-daemon-服務)
   - [3.5 開啟視覺化工作台 Web DEMO](#35-開啟視覺化工作台-web-demo)
   - [3.6 設定速率與站點逾時 (service.toml)](#36-設定速率與站點逾時-servicetoml)
4. [Rust 程式庫整合指南 (Library Integration)](#4-rust-程式庫整合指南-library-integration)
   - [4.1 初始化服務實例](#41-初始化服務實例)
   - [4.2 掛載底層硬體通訊驅動](#42-掛載底層硬體通訊驅動)
   - [4.3 監聽 COS (Change-of-State) 變化推播](#43-監聽-cos-change-of-state-變化推播)
   - [4.4 驅動排程輪詢循環 (Poller Loop)](#44-驅動排程輪詢循環-poller-loop)
5. [六大核心機制 API 使用手冊](#5-六大核心機制-api-使用手冊)
   - [§2 即時快取與死區過濾 (In-Memory TagStore & Deadband)](#2-即時快取與死區過濾-in-memory-tagstore--deadband)
   - [§3 & §6 & §9.3 寫入仲裁、四大護欄與插隊預佔](#3--6--93-寫入仲裁四大護欄與插隊預佔)
   - [§4 雙軌動態訂閱與 3 秒退訂冷卻 (Two-Tier Subscription)](#4-雙軌動態訂閱與-3-秒退訂冷卻-two-tier-subscription)
   - [§5 間隙容忍相鄰拼包 (Gap-Tolerant Coalescing)](#5-間隙容忍相鄰拼包-gap-tolerant-coalescing)
   - [§7 單站緊湊逾時熔斷器 (Station Circuit Breaker)](#7-單站緊湊逾時熔斷器-station-circuit-breaker)
   - [§8.1 Tauri IPC 通訊契約](#81-tauri-ipc-通訊契約)
6. [無頭 Daemon HTTP API 參考](#6-無頭-daemon-http-api-參考)
7. [視覺化工作台 DEMO 操作指南](#7-視覺化工作台-demo-操作指南)
8. [常見問題與故障排除 (FAQ)](#8-常見問題與故障排除-faq)

---

## 1. 專案架構與定位

`PlcDataService` 是介於「實體硬體驅動層（`mm-drivers` 56種通訊協議）」與「業務展示層（Tauri Desktop / Web HMI / MES）」之間的**單一數據真理來源（Single Source of Truth, SSOT）**：

```
┌────────────────────────────────────────────────────────────────────────┐
│                        應用展示層 (HMI / Web / MES)                     │
│               唯讀快取、事件訂閱、語意寫入 (feeder.speed = 1500)            │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ IPC / WebSocket / HTTP (Tag-based)
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

### 設計核心特點
1. **全安全性保證**：全核心標記 `#![forbid(unsafe_code)]`。
2. **極致微秒延遲**：讀取直接命中記憶體快取（`< 1 µs`），支撐前端 60 FPS 零抖動。
3. **通訊總線保護**：針對半雙工 RS-485 與低頻寬網路，落實相鄰拼包與寫入插隊。
4. **安全本質優先**：落實機台模型 §9.3 四大寫入護欄，軟體錯誤與超界寫入在服務層攔截。

---

## 2. 專案目錄與模組職責

```
DataService/
├── Cargo.toml                              # Workspace 根設定檔
├── DATA_SERVICE_SPEC.md                    # 工業級 PLC 資料服務層執行期規格書 (v1.0)
├── README.md                               # 專案總覽文件
├── USAGE.md                                # 本使用說明書
├── UPGRADE_PLAN_DATASERVICE1.md            # 架構升級分析與設計文件
├── plc_data_service_workbench_demo.html    # 全功能視覺化測試與調校工作台
├── crates/
│   ├── plc-data-service/                   # 核心執行期資料服務庫
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      # Facade 門面、頂層引擎與整合測試
│   │       ├── config.rs                   # service.toml 解析：[poll] 速率與 [station.<id>] 逾時
│   │       ├── cache/                      # 即時快取、原子版本號、Deadband 與品質
│   │       ├── subscription/               # 雙軌訂閱器、引用計數、3s 冷卻、Zombie GC
│   │       ├── poller/                     # 相鄰拼包演算法與多速率時間輪排程佇列
│   │       ├── arbiter/                    # Urgent 插隊、§9.3 四大護欄與審計日誌
│   │       └── gateway/                    # 連線狀態機與單站緊湊逾時熔斷器
│   ├── datasvc-demo/                       # 可執行之測試與示範應用程式
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs                     # CLI 入口（支援三種運作模式）
│   │       ├── verify.rs                   # 規格逐項自動化驗證報告 (§1 ~ §9)
│   │       ├── interactive.rs              # 終端互動式 HMI 調試器
│   │       ├── daemon.rs                   # 唯讀 HTTP JSON 無頭 Daemon
│   │       └── model.rs                    # 示範機台模型建構器
│   ├── machine-model/                      # 機台語意模型與 §9.3 護欄判定庫
│   └── mm-drivers/                         # 56 種 PLC / 儀表通訊協議驅動庫
├── src-tauri/                              # Tauri 2.0 桌面應用 IPC 介面層
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── datasvc_commands.rs             # 供前端呼叫之 5 大 Tauri IPC Commands
│       ├── lib.rs
│       └── main.rs
└── ui/
    ├── index.html                          # Tauri 前端入口（與工作台同步）
    └── plc_data_service_workbench_demo.html
```

---

## 3. 快速開始 (Quick Start)

### 3.1 編譯與全域測試
在專案根目錄下，執行完整的 Workspace 建置與測試：
```bash
# 1. 編譯檢查整個 Workspace (含所有 crates 與 src-tauri)
cargo check --workspace

# 2. 執行全專案單元與整合測試 (42 個測試全數通過)
cargo test --workspace
```

---

### 3.2 執行規格逐項驗證報告
若要快速驗證本專案是否完整遵循《DATA_SERVICE_SPEC.md v1.0》的所有規範：
```bash
cargo run -p datasvc-demo
```
**輸出範例**：
```
==================================================================
  PlcDataService 規格驗證報告 (DATA_SERVICE_SPEC.md §1 ~ §9)
==================================================================
[PASS] §3.1   模型載入後每個點位都建立路由並綁定位址
[PASS] §3.2   護欄 1：唯讀點位攔截
[PASS] §3.2   護欄 3：上下限攔截，範圍內寫入成功
[PASS] §3.2   護欄 4：審計日誌記錄每次嘗試並標示是否允許
[PASS] §4.1   永久軌點位不受視角退訂影響
[PASS] §4.2   引用計數：仍有視角引用時不退訂
[PASS] §4.2   3 秒退訂冷卻：到期前保留、到期後移除
[PASS] §4.2   冷卻期間重新進入會取消退訂
[PASS] §4.2   首幀就緒：Burst 掃描一次後視角標記為 ready
[PASS] §4.2   殭屍連線回收：心跳逾時 30 秒後回收
[PASS] §5.1   相鄰位址在間隙容忍內合併為單一區塊
[PASS] §5.1   間隙超過 MaxGap 時拆分區塊
[PASS] §5.1   合併長度超過 MaxBlock 時拆分區塊
[PASS] §5.2   多速率分群：各速率層依間隔到期
[PASS] §5.2   輪詢讀取驅動值並依有號 int16 解碼後更新快取
[PASS] §6.1   寫入驅動失敗時，快取保持不變
[PASS] §6.1   Urgent 佇列：寫入先入列，輪詢前依 FIFO 排空
[PASS] §6.2   寫入 ACK 後直接更新快取，回傳值與快取一致
[PASS] §6.2   批次寫入後以單次 Batch Read 回讀校驗 (verify_batch)
[PASS] §6.3   斷線或熔斷期間寫入立即拒絕且不排隊
[PASS] §7     緊湊逾時 50ms 套用至站點驅動的通訊逾時
[PASS] §7     連續 3 次失敗才觸發熔斷
[PASS] §7     熔斷時保留最後已知值並標記為 Bad
[PASS] §7     熔斷期間每 5000ms 放行一次探測
[PASS] §7     站點恢復後品質重新標記為 Good
[PASS] §8.2   死區過濾：死區內不發 COS，超出死區或品質改變才發出
------------------------------------------------------------------
通過 26  失敗 0  不適用 5
```

---

### 3.3 啟動終端互動式 HMI
提供文字終端操作介面，可在命令列中直接調試讀寫、切換視角、觸發熔斷與觀察拼包：
```bash
cargo run -p datasvc-demo -- --interactive
```
可加上 `--config service.toml` 套用速率與逾時設定（見 3.6）。

**常用互動指令**：
- `list`：列出所有點位快照（數值、品質、版本）。
- `get <TAG>`：讀取單一點位（如 `get plc1.D_Cyl1_ExtTime`）。
- `write <TAG> <VALUE>`：寫入點位（如 `write plc1.D_Cyl1_ExtTime 2500`）。
- `enter <VIEW> <TAG>...`：模擬進入畫面視角並訂閱點位。
- `leave <VIEW>`：離開視角（觸發 3 秒退訂冷卻倒數）。
- `blocks`：檢視當前活躍點位自動拼包後的連續讀取區塊。
- `poll`：手動驅動一次輪詢排程（排空 Urgent 寫入佇列並更新快取）。
- `station <ID> fail`：模擬站點逾時故障（連續 3 次觸發熔斷隔離）。
- `audit`：檢視安全審計日誌。
- `quit`：退出。

---

### 3.4 啟動無頭 Daemon 服務
在無螢幕的工廠邊緣運算閘道器（Edge Gateway）中，可啟動無頭背景服務：
```bash
cargo run -p datasvc-demo -- --serve --port 8080
```
服務將於 `http://127.0.0.1:8080` 開啟唯讀 HTTP JSON 介面，背景每秒自動進行 3 秒退訂冷卻與殭屍連線維護。加上 `--config service.toml` 可套用速率與逾時設定（見 3.6）。

---

### 3.5 開啟視覺化工作台 Web DEMO
雙擊或在瀏覽器中直接開啟專案根目錄下的 [`plc_data_service_workbench_demo.html`](file:///mnt/c/Users/wydct/orca/projects/DataService/plc_data_service_workbench_demo.html)：
- 100% 離線運行（無任何外網 CDN 依賴，採用 ISA-101 深色工業風）。
- 提供點位快照、雙軌訂閱模擬、拼包即時視覺化、四大護欄一鍵測試台與 API 契約調試器。

---

### 3.6 設定速率與站點逾時 (service.toml)
`service.toml` 的 `[poll]` 與 `[station.<id>]` 區段決定輪詢速率與各站點的緊湊逾時。TCP 與 RS-485 的通訊時間差異大，請依實際通訊方式設定：
```toml
[poll]
high_ms = 100       # High 層（急停、高頻座標）輪詢間隔 (ms)
normal_ms = 500     # Normal 層（一般運轉、氣缸感測器）輪詢間隔 (ms)
low_ms = 2000       # Low 層（溫度、產能等非即時資料）輪詢間隔 (ms)

[station.plc1]      # 站點 ID 須與機台模型中的站點一致
tight_timeout_ms = 50
```

| 設定 | 省略時的預設值 | 限制 |
| :--- | :--- | :--- |
| `[poll]` `high_ms` / `normal_ms` / `low_ms` | RS-485 預設 `100` / `500` / `2000` ms | 必須大於 0，且 `high_ms ≤ normal_ms ≤ low_ms` |
| `[station.<id>]` `tight_timeout_ms` | `50` ms | 不得低於 `25` ms；`<id>` 須存在於機台模型中 |

- **TCP 建議值**：`high_ms = 50`、`normal_ms = 250`、`low_ms = 1000`（規格下限）。
- **RS-485 建議值**：先使用預設值，再依現場匯流排速率與每輪區塊數調整。
- **目前限制**：速率為整個服務共用一組。若 TCP 與 RS-485 站點接在同一個服務下，兩者會使用相同間隔。
- **`[[tag_override]]`** 仍由 `machine-model` 解析，可與上述區段並存於同一檔案。

啟動時指定設定檔（僅 `--interactive` 與 `--serve` 生效；驗證報告固定使用預設值）：
```bash
cargo run -p datasvc-demo -- --interactive --config service.toml
cargo run -p datasvc-demo -- --serve --port 8080 --config service.toml
```
設定錯誤（檔案不存在、語法錯誤、數值超界或站點不存在）時，程式會以代碼 2 退出並顯示原因。

---

## 4. Rust 程式庫整合指南 (Library Integration)

若您要在自己的 Rust 服務或 Tauri 桌面應用中使用 `plc-data-service`，請參考以下步驟：

### 4.1 初始化服務實例
在您的專案 `Cargo.toml` 加入：
```toml
[dependencies]
plc-data-service = { path = "../DataService/crates/plc-data-service" }
machine-model = { path = "../DataService/crates/machine-model" }
mm-drivers = { path = "../DataService/crates/mm-drivers" }
```

在 Rust 代碼中初始化：
```rust
use plc_data_service::{PlcDataService, config::ServiceConfig, poller::CoalescingPolicy};
use machine_model::resolver::ResolvedMachineModel;

// 1. 取得解析完成的機台模型 (machine.yaml + unit-types)
let resolved_model: ResolvedMachineModel = load_my_machine_model();

// 2. 讀取 service.toml 的速率與站點逾時設定（省略的鍵使用預設值，見 3.6）
let text = std::fs::read_to_string("service.toml").map_err(|e| e.to_string())?;
let config = ServiceConfig::from_toml_str(&text)?;

// 3. 設定相鄰暫存器拼包策略 (RS-485 預設 MaxGap=4 Words, MaxBlock=64 Words)
let policy = CoalescingPolicy::rs485_default();

// 4. 建立服務實例與全域變化事件廣播通道 (rx)；設定與機台模型不一致時回傳錯誤
let (mut svc, mut event_rx) = PlcDataService::from_resolved_model(
    &resolved_model,
    &[], // 可選之 [[tag_override]] 覆寫規則（由 machine-model 解析）
    policy,
    &config,
)?;
```

---

### 4.2 掛載底層硬體通訊驅動
針對不同站點，掛載對應的通訊驅動（支援真實硬體與模擬模式）：
```rust
use mm_drivers::{PlcDriver, UniversalPlcDriver, DriverConfig, DriverKind};

// 建立驅動實例 (例如 Inovance H5U Modbus TCP 站點)
let mut driver = UniversalPlcDriver::new(DriverConfig {
    driver_kind: DriverKind::InovanceModbusTcp,
    endpoint: "192.168.1.100:502".into(),
    timeout_ms: 100,
    ..Default::default()
});
let _ = driver.connect(driver.config().clone());

// 掛載至資料服務層 (自動建立單站熔斷器；緊湊逾時取自 service.toml 的 [station.plc1]，未設定時為 50ms)
svc.attach_driver("plc1", Box::new(driver));
```

---

### 4.3 監聽 COS (Change-of-State) 變化推播
當點位數值超越死區（Deadband）或資料品質（`DataQuality`）發生改變時，`PlcDataService` 透過廣播通道發布事件（Zero-Waste 無變化不推播）：
```rust
tokio::spawn(async move {
    while let Ok(event) = event_rx.recv().await {
        println!(
            "[COS 事件] 點位: {} | 舊值: {:?} -> 新值: {:?} | 品質: {:?} | 版本: v{}",
            event.tag_id, event.old_value, event.new_value, event.quality, event.version
        );
        // 推播至 WebSocket 或前端 Tauri 畫面
    }
});
```

---

### 4.4 驅動排程輪詢循環 (Poller Loop)
在背景執行緒中驅動多速率時間輪排程與拼包讀取：
```rust
use plc_data_service::now_ms;
use std::time::Duration;

tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    loop {
        interval.tick().await;
        let now = now_ms();

        // 1. 維護 3 秒退訂冷卻倒數與清理殭屍連線
        let _expired_views = svc.subscriptions.tick_maintenance(now);

        // 2. 取出當前所有活躍訂閱點位，經演算法合併為最少次數之連續區塊
        let coalesced_blocks = svc.compute_active_coalesced_blocks();

        // 3. 依據時間輪到期時間與 Urgent 插隊佇列執行實體讀寫
        for block in coalesced_blocks {
            // 下發驅動 read_continuous(&block.start_address, block.total_words, ...)
            // 讀回後更新 svc.store.update_tag(...)
        }
    }
});
```
> **注意**：輪詢迴圈的 `interval` 週期必須不大於最快的速率層間隔（`[poll] high_ms`）。若迴圈每 50 ms 才檢查一次，設定低於 50 ms 的高速層實際上會以 50 ms 執行。

---

## 5. 六大核心機制 API 使用手冊

### §2 即時快取與死區過濾 (In-Memory TagStore & Deadband)
- **快取命中讀取**（耗時 `< 1 µs`）：
  ```rust
  // 傳入空陣列表示讀取全有點位快照
  let all_tags = svc.store.get_snapshots(&[]);
  
  // 讀取特定點位
  let tags = svc.store.get_snapshots(&["plc1.D_Cyl1_ExtTime".into()]);
  if let Some(snap) = tags.get("plc1.D_Cyl1_ExtTime") {
      println!("數值: {:?}, 品質: {:?}, 納秒戳記: {}", snap.value, snap.quality, snap.timestamp_ns);
  }
  ```
- **死區過濾規則**：
  - 支援 `DeadbandConfig::Absolute(f64)`（絕對差額）與 `DeadbandConfig::Ratio(f64)`（比例差額）。
  - 當微小數值變動未超越門檻時，快取內部過濾，不消耗 CPU 序列化與網路推播頻寬。

---

### §3 & §6 & §9.3 寫入仲裁、四大護欄與插隊預佔
所有向 PLC 寫入之請求，**嚴禁前端直接發送暫存器指令**，必須透過語意點位調用 `write_tag`：
```rust
use machine_model::model::ScalarValue;

let res = svc.write_tag(
    "plc1.D_Cyl1_ExtTime",           // 語意點位 ID
    ScalarValue::Number(2500.0),    // 寫入數值
    "operator_01",                  // 操作員身分識別 (Audit Trail)
    "192.168.1.50",                 // 客戶端來源 IP
);

match res {
    Ok(ack) => println!("寫入成功，耗時 {} µs，審計版本 v{}", ack.elapsed_us, ack.audit_version),
    Err(e) => eprintln!("寫入被拒絕: {}", e),
}
```

#### 四大寫入護欄判定規則：
1. **護欄 1：唯讀點位攔截**：若點位綁定為感測器輸入點（`final_writable == false`），直接拋出 `ReadOnlyGuardrail`。
2. **護欄 2：安全監控點阻擋**：若點位為 `safety_monitor == true`（如急停按鈕、安全門迴路），直接阻擋寫入。
3. **護欄 3：上下限超界攔截**：若數值小於 `min_val` 或大於 `max_val`，拋出 `OutOfBoundsError`。
4. **護欄 4：安全審計日誌 (Audit Trail)**：無論核准或攔截，每次請求皆記錄至 `svc.audit_trail`（時間戳、操作員、IP、新舊數值、判定結果）。
5. **§6.3 Fail-Fast 離線阻擋**：若目標站點處於斷線或熔斷隔離狀態，立即返回 `DeviceOfflineError`（**嚴禁離線排隊**，防止復歸時突發暴衝）。

---

### §4 雙軌動態訂閱與 3 秒退訂冷卻 (Two-Tier Subscription)
- **軌道 1：Permanent 永久常駐軌**：
  - 急停、安全開關、致命警報點位自動歸入永久軌，**永不退訂**。
- **軌道 2：Viewport 動態視角軌**：
  ```rust
  // 1. 客戶端進入畫面：掛載點位並排入首幀 Burst 突發優先佇列
  svc.subscriptions.enter_viewport(
      "HMI_Panel_1",             // Client ID
      "view_manual",             // Viewport ID
      vec!["plc1.M_Cyl1_ManExt".into(), "plc1.D_Cyl1_ExtTime".into()],
      now_ms(),
  );

  // 2. 客戶端離開畫面：啟動 3 秒退訂冷卻計時器
  svc.subscriptions.leave_viewport("HMI_Panel_1", "view_manual", now_ms());

  // 3. 在 3 秒內重新進入相同畫面：自動取消退訂計時，總線零震盪重組！
  svc.subscriptions.enter_viewport("HMI_Panel_1", "view_manual", vec![...], now_ms());
  ```

---

### §5 間隙容忍相鄰拼包 (Gap-Tolerant Coalescing)
演算法依據 `(station_id, area_prefix)` 自動分組與排序，當兩暫存器之間隙 $A_{k+1} - (A_k + \text{len}_k) \le \text{MaxGap}$ 且合併長度 $\le \text{MaxBlock}$ 時自動拼為單包：
```rust
// 取得當前所有活躍點位拼包結果
let blocks = svc.compute_active_coalesced_blocks();
for b in &blocks {
    println!(
        "站點: {} | 區塊起始: {} | 長度: {} Words | 內含 Tag 數: {} | 順道跨越未訂閱 Gap: {} Words",
        b.station_id, b.start_address, b.total_words, b.member_tags.len(), b.gap_words_bridged
    );
}
```

---

### §7 單站緊湊逾時熔斷器 (Station Circuit Breaker)
每個站點皆具備獨立熔斷器，保護同總線其他正常站點：
- **緊湊逾時**：RS-485 預設通訊逾時設為 `50ms`（理論耗時 + 25ms 餘裕），可於 `service.toml` 的 `[station.<id>]` 以 `tight_timeout_ms` 調整（最低 `25ms`，見 3.6）。
- **3 次逾時熔斷**：連續 3 次通訊逾時或斷線，站點狀態轉為 `TrippedDegraded`。
- **品質自動標記**：熔斷站點的所有點位自動標記為 `DataQuality::Bad`（保留最後已知值）。
- **降頻探測隔離**：熔斷期間每隔 `5000ms` 僅放行一次微探測，不拖垮同通訊埠其他 PLC 站點。

---

### §8.1 Tauri IPC 通訊契約
前端介面（Vue / React / Svelte）透過 `@tauri-apps/api/core` 呼叫：

```javascript
import { invoke } from '@tauri-apps/api/core';

// 1. 讀取快照 (< 1 µs 直讀)
const snapshots = await invoke('datasvc_get_snapshot', {
  tags: ['feeder.spindle.actual_speed', 'system.safety.estop']
});

// 2. 視角切換進入
await invoke('datasvc_enter_view', {
  clientId: 'HMI_Main_Panel',
  viewId: 'view_manual',
  tags: ['feeder.cyl1.man_ext', 'feeder.cyl1.lim_ext']
});

// 3. 視角離開 (啟動 3 秒冷卻)
await invoke('datasvc_leave_view', {
  clientId: 'HMI_Main_Panel',
  viewId: 'view_manual'
});

// 4. 具備四大護欄之語意寫入
const writeResult = await invoke('datasvc_write_tag', {
  tag: 'feeder.cyl1.par_ext_time',
  value: 2400,
  operator: 'op_01@192.168.1.50'
});

// 5. 取得診斷資訊 (雙軌訂閱引用計數與拼包區塊)
const [stats, blocks] = await invoke('datasvc_get_diagnostics');
```

---

## 6. 無頭 Daemon HTTP API 參考

當執行 `cargo run -p datasvc-demo -- --serve --port 8080` 時，提供下列唯讀 REST 端點：

| HTTP 端點 | 說明 | 回傳格式範例 |
| :--- | :--- | :--- |
| `GET /tags` | 取得所有語意點位最新快照 (數值、品質、版本、納秒時間戳) | `{"plc1.D_Cyl1_ExtTime": {"value": 2500, "quality": "Good", "version": 3}}` |
| `GET /stations` | 檢視所有 PLC 站點熔斷器狀態、逾時設定與失敗計數 | `{"plc1": {"state": "Connected", "consecutive_failures": 0, "tight_timeout_ms": 50}}` |
| `GET /subscriptions` | 檢視雙軌訂閱統計、點位引用計數 (`ref_count`) 與 3 秒冷卻名單 | `{"permanent_tags": [...], "active_viewport_tags": {"plc1.M_ManExt": 1}}` |
| `GET /blocks` | 取得目前相鄰拼包演算法輸出之連續區塊清單與跨越 Gap 統計 | `[{"station_id": "plc1", "start_address": "D100", "total_words": 7}]` |
| `GET /audit` | 檢視最新安全寫入審計日誌 (含操作員、IP、新舊值與判定說明) | `[{"operator_id": "op1", "tag_id": "D100", "permitted": true}]` |

---

## 7. 視覺化工作台 DEMO 操作指南

開啟 [`plc_data_service_workbench_demo.html`](file:///mnt/c/Users/wydct/orca/projects/DataService/plc_data_service_workbench_demo.html) 後，介面分為四大面板：

1. **左側「雙軌動態訂閱模擬器」**：
   - 點擊「視角: 機台總覽」或「視角: 氣缸調機」進行畫面切換。
   - 觀察在離開視角時，右上方狀態盒會出現 **`[3秒冷卻中]` 倒數計時**。在 3 秒內切回，冷卻即時重置，點位永不退訂。
   - 點擊「模擬崩潰 (GC)」可測試前端意外關閉時，後端自動回收訂閱點位的防禦機制。
2. **Tab 1: 語意點位即時快取總覽**：
   - 檢視所有 Tag 的目前數值、DataQuality（Good 綠燈、Bad 紅燈、Uncertain 黃燈、Stale 灰燈）。
   - 點擊「注入死區內微小抖動」，觀察計數器累加但 **COS 事件流保持靜默（Zero-Waste）**。
   - 點擊「注入超越死區變化」，即刻觸發 COS 事件推播並更新版本號。
3. **Tab 2: 智慧相鄰拼包排程器**：
   - 切換通訊預設（RS-485、TCP 或關閉拼包）。
   - 調整 `MaxGap`（允許間隙）與 `MaxBlock`，即時檢視下方封包重組情況，並檢視**「封包數縮減百分比」**。
4. **Tab 3: 寫入仲裁與四大護欄測試台**：
   - 提供 6 個一鍵測試按鈕：
     - `合法手動指令寫入` -> ✅ 綠燈通過 (ACK)。
     - `合法製程參數寫入` -> ✅ 綠燈通過 (ACK)。
     - `測試護欄 1: 寫入唯讀感測器` -> 🛑 紅燈攔截 (ReadOnlyGuardrail)。
     - `測試護欄 2: 寫入安全急停點` -> 🛑 紅燈攔截 (SafetyMonitorGuardrail)。
     - `測試護欄 3: 參數超界寫入` -> 🛑 紅燈攔截 (OutOfBoundsError)。
     - `測試配方批次寫入 + 單次 Batch 回讀` -> 📦 批次寫入並自動校驗。
   - 下方表格即時呈現不可篡改之安全審計日誌。
5. **Tab 4: 上層應用通訊契約測試器**：
   - 選擇 API 指令，自動填入範例 JSON Payload，點擊「呼叫 DataService API」直接預覽回傳格式。

---

## 8. 常見問題與故障排除 (FAQ)

### Q1: 為什麼呼叫 `write_tag` 回傳 `DeviceOfflineError`？
**解答**：表示目標 PLC 站點處於斷線或連續 3 次逾時觸發了熔斷隔離（`TrippedDegraded`）。本服務落實 **§6.3 Fail-Fast 哲學**，斷線期間嚴禁將寫入排入離線佇列，以防復歸時大量積壓指令暴衝造成工安事故。請先確認站點連線恢復正常。

### Q2: 為什麼修改點位數值後，前端沒有收到 `tag_changed` 事件？
**解答**：請確認該點位是否有設定死區過濾（`deadband > 0`）。若新舊數值之差額未超越設定門檻，本服務會啟用 **Zero-Waste 機制** 靜默過濾，以避免浪費前端渲染與通訊頻寬。

### Q3: RS-485 通訊總線偶發塞車，該如何調整？
**解答**：
1. 調整 `service.toml` 的 `[poll]`：拉長 `high_ms`、`normal_ms`、`low_ms`，讓一輪讀取完成後才進入下一輪（RS-485 預設 `100` / `500` / `2000` ms，見 3.6）。
2. 檢查拼包策略：確保半雙工總線已啟用拼包（建議 `max_gap_words = 4`, `max_block_words = 64`）。
3. 評估站點逾時：`tight_timeout_ms` 偏小時，正常回應也會被判定為逾時；偏大時，故障熔斷會變慢。
4. 檢查畫面視角：確認離開畫面時有正確呼叫 `datasvc_leave_view`，讓不再觀測的點位在 3 秒冷卻後自動退訂。

### Q4: 如何在沒有實體 PLC 硬體的情況下進行開發調試？
**解答**：
可直接使用 `crates/datasvc-demo`：
- 終端互動模式：`cargo run -p datasvc-demo -- --interactive`（內建模擬器）。
- 網頁視覺化工作台：開啟 `plc_data_service_workbench_demo.html`。
兩者皆能在記憶體中完整模擬通訊、死區、拼包與護欄全流程。
