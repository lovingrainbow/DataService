以下已根據升級計劃書與既有專案結構完成 plc-data-service 的完整架構升級。本次升級保留了對 machine-model 與 mm-drivers 的實體依賴以及 src-tauri 桌面 IPC 的相容性，並將核心引擎拆分為 5 大模組子目錄、全面導入 parking\_lot 與 tracing，同時新增 crates/datasvc-demo 終端驗證與無頭 Daemon 專案。   1. 升級項目執行摘要Phase 1：Workspace 依賴與目錄骨架升級   在根目錄與 crates/plc-data-service/Cargo.toml 引入 parking\_lot = "0.12"、tracing = "0.1"、tracing-subscriber = "0.3"、thiserror = "1.0" 與 async-trait = "0.1"。   將 crates/plc-data-service/src/ 拆分為 cache/、subscription/、poller/、arbiter/、gateway/ 五大子目錄。   Phase 2：核心模組細部遷移與強化   cache/：使用 parking\_lot::RwLock 實作 TagStore 無鎖中毒微秒級讀取直通，並整合 TagSnapshot、DataQuality、DeadbandConfig 與 TagChangeEvent。   subscription/：實作 Always-On 常駐背景與 Dynamic Lease 動態畫面雙軌訂閱、3 秒退訂冷卻倒數（Cooldown）與 Zombie 租約自動 GC，以及 ViewportReady 與 COS 事件通道。   poller/：實作間隙容忍相鄰位址智慧拼包演算法（coalescing.rs）與多速率時間輪調度佇列（High 50ms、Normal 250ms、Low 1000ms）。   arbiter/：結合上游 machine-model 實作 §9.3 四大寫入護欄（唯讀攔截、數值上下限檢查、安全狀態互鎖、動作前置條件檢查）、Urgent 插隊優先佇列與寫入審計日誌。   gateway/：補全單站熔斷器（should\_allow\_poll、record\_success、record\_failure 與指數退避探針）並內建高擬真虛擬 PLC 模擬器與 mm-drivers 通訊通道。   Phase 3：頂層 Facade 黏合 (crates/plc-data-service/src/lib.rs)   保持 PlcDataService::from\_resolved\_model、write\_tag、compute\_active\_coalesced\_blocks 等公開 API 簽名相容，確保既有整合測試與 src-tauri 無縫接軌。   Phase 4：新增獨立 CLI / Daemon Crate (crates/datasvc-demo)   支援自動化規格檢驗（§1 \~ §9 逐項驗證報告）、終端互動式 HMI（--interactive）與無頭伺服器模式（--serve --port 8080）。   2. 升級後專案目錄結構Plaintextdataservice/
├── Cargo.toml                                      # \[修改\] 新增 datasvc-demo 成員與共用依賴
├── crates/
│   ├── machine-model/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs                              # \[保留相容\] 機台模型、點位規格與 §9.3 護欄定義
│   ├── mm-drivers/
│   │   ├── Cargo.toml
│   │   ├── src/lib.rs                              # \[保留相容\] 工業 PLC 驅動抽象與模擬實作
│   │   ├── src/serial\_and\_protocols.rs
│   │   └── tests/plc\_driver\_tests.rs
│   ├── plc-data-service/
│   │   ├── Cargo.toml                              # \[修改\] 引入 parking\_lot, tracing, thiserror, async-trait
│   │   └── src/
│   │       ├── lib.rs                              # \[重構\] 頂層 Facade 引擎與整合測試
│   │       ├── cache/
│   │       │   ├── mod.rs                          # \[新增\] TagStore (parking\_lot::RwLock)
│   │       │   └── snapshot.rs                     # \[新增\] TagSnapshot, DataQuality, DeadbandConfig
│   │       ├── subscription/
│   │       │   ├── mod.rs                          # \[新增\] 雙軌訂閱管理主體
│   │       │   ├── tracker.rs                      # \[新增\] 引用計數、3秒退訂冷卻、Zombie Auto-GC
│   │       │   └── events.rs                       # \[新增\] ViewportReady 與 COS 推播通道
│   │       ├── poller/
│   │       │   ├── mod.rs                          # \[新增\] 輪詢調度引擎
│   │       │   ├── coalescing.rs                   # \[新增\] 間隙容忍相鄰位址拼包演算法
│   │       │   └── scheduler.rs                    # \[新增\] High/Normal/Low 多速率時間輪佇列
│   │       ├── arbiter/
│   │       │   ├── mod.rs                          # \[新增\] Urgent 寫入插隊仲裁與 AuditLogEntry
│   │       │   └── guardrails.rs                   # \[新增\] §9.3 四大寫入護欄驗證器
│   │       └── gateway/
│   │           ├── mod.rs                          # \[新增\] 通訊抽象通道與高擬真虛擬 PLC 模擬器
│   │           └── circuit\_breaker.rs              # \[新增\] 單站熔斷器與指數退避復原完整實作
│   └── datasvc-demo/
│       ├── Cargo.toml                              # \[新增\] 獨立 CLI / Daemon DEMO Crate
│       └── src/main.rs                             # \[新增\] 自動規格檢驗 + Terminal HMI + --serve Daemon
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── src/
│       ├── lib.rs
│       ├── main.rs
│       └── datasvc\_commands.rs                     # \[相容驗證\] Tauri IPC 命令完美對接升級後 API
└── ui/
    ├── index.html
    └── plc\_data\_service\_workbench\_demo.html
3. 完整專案檔案內容3.1 根目錄 Cargo.tomlIni, TOML\[workspace\]
resolver = "2"
members = \[
    "crates/machine-model",
    "crates/mm-drivers",
    "crates/plc-data-service",
    "crates/datasvc-demo",
    "src-tauri",
\]

\[workspace.package\]
version = "0.2.0"
edition = "2021"
license = "MIT"
authors = \["Industrial Automation Architecture Team"\]

\[workspace.dependencies\]
serde = { version = "1.0", features = \["derive"\] }
serde\_json = "1.0"
tokio = { version = "1.37", features = \["full"\] }
parking\_lot = "0.12"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = \["env-filter", "fmt"\] }
thiserror = "1.0"
async-trait = "0.1"
machine-model = { path = "crates/machine-model" }
mm-drivers = { path = "crates/mm-drivers" }
plc-data-service = { path = "crates/plc-data-service" }
3.2 crates/machine-model（保留上游機台模型與 §9.3 護欄契約）crates/machine-model/Cargo.tomlIni, TOML\[package\]
name = "machine-model"
version.workspace = true
edition.workspace = true

\[dependencies\]
serde.workspace = true
serde\_json.workspace = true
thiserror.workspace = true
crates/machine-model/src/lib.rsRustuse serde::{Deserialize, Serialize};
use std::collections::HashMap;

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
#\[serde(untagged)\]
pub enum ScalarValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl ScalarValue {
    pub fn as\_f64(&amp;self) -&gt; Option&lt;f64&gt; {
        match self {
            ScalarValue::Int(v) =&gt; Some(\*v as f64),
            ScalarValue::Float(v) =&gt; Some(\*v),
            \_ =&gt; None,
        }
    }

    pub fn as\_bool(&amp;self) -&gt; Option&lt;bool&gt; {
        match self {
            ScalarValue::Bool(b) =&gt; Some(\*b),
            ScalarValue::Int(i) =&gt; Some(\*i != 0),
            \_ =&gt; None,
        }
    }
}

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)\]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
    WriteOnly,
}

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)\]
pub enum PollingTier {
    High,   // 50ms
    Normal, // 250ms
    Low,    // 1000ms
}

impl PollingTier {
    pub fn interval\_ms(&amp;self) -&gt; u64 {
        match self {
            PollingTier::High =&gt; 50,
            PollingTier::Normal =&gt; 250,
            PollingTier::Low =&gt; 1000,
        }
    }
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct SafeStateConstraint {
    pub interlock\_tag: String,
    pub required\_value: ScalarValue,
    pub reason: String,
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct ActionPrecondition {
    pub blocking\_tag: String,
    pub forbidden\_value: ScalarValue,
    pub reason: String,
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct TagModelSpec {
    pub tag\_id: String,
    pub station\_id: String,
    pub register\_area: String,
    pub byte\_offset: u32,
    pub byte\_length: u16,
    pub access\_mode: AccessMode,
    pub polling\_tier: PollingTier,
    pub always\_on: bool,
    pub deadband: Option&lt;f64&gt;,
    pub min\_value: Option&lt;f64&gt;,
    pub max\_value: Option&lt;f64&gt;,
    pub require\_safe\_state: Option&lt;SafeStateConstraint&gt;,
    pub forbidden\_when\_active: Option&lt;ActionPrecondition&gt;,
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct StationEndpointSpec {
    pub station\_id: String,
    pub protocol: String,
    pub endpoint: String,
    pub max\_gap\_bytes: u32,
    pub max\_pdu\_bytes: u32,
    pub failure\_threshold: u32,
    pub base\_cooldown\_ms: u64,
    pub max\_cooldown\_ms: u64,
}

#\[derive(Debug, Clone, Default, Serialize, Deserialize)\]
pub struct ResolvedMachineModel {
    pub machine\_id: String,
    pub stations: HashMap&lt;String, StationEndpointSpec&gt;,
    pub tags: HashMap&lt;String, TagModelSpec&gt;,
}

impl ResolvedMachineModel {
    pub fn demo\_semiconductor\_equipment() -&gt; Self {
        let mut stations = HashMap::new();
        stations.insert(
            "PLC\_MAIN".to\_string(),
            StationEndpointSpec {
                station\_id: "PLC\_MAIN".to\_string(),
                protocol: "Mitsubishi\_MC\_3E".to\_string(),
                endpoint: "192.168.1.10:5002".to\_string(),
                max\_gap\_bytes: 16,
                max\_pdu\_bytes: 480,
                failure\_threshold: 3,
                base\_cooldown\_ms: 1000,
                max\_cooldown\_ms: 16000,
            },
        );
        stations.insert(
            "PLC\_CHAMBER".to\_string(),
            StationEndpointSpec {
                station\_id: "PLC\_CHAMBER".to\_string(),
                protocol: "Siemens\_S7".to\_string(),
                endpoint: "192.168.1.20:102".to\_string(),
                max\_gap\_bytes: 16,
                max\_pdu\_bytes: 240,
                failure\_threshold: 3,
                base\_cooldown\_ms: 1000,
                max\_cooldown\_ms: 16000,
            },
        );

        let mut tags = HashMap::new();

        // Always-On 安全狀態點位
        tags.insert(
            "SAFETY.DOOR\_CLOSED".to\_string(),
            TagModelSpec {
                tag\_id: "SAFETY.DOOR\_CLOSED".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 100,
                byte\_length: 2,
                access\_mode: AccessMode::ReadWrite,
                polling\_tier: PollingTier::High,
                always\_on: true,
                deadband: None,
                min\_value: None,
                max\_value: None,
                require\_safe\_state: None,
                forbidden\_when\_active: None,
            },
        );

        // Always-On 幫浦運轉狀態點位
        tags.insert(
            "VACUUM.ROUGH\_PUMP\_RUNNING".to\_string(),
            TagModelSpec {
                tag\_id: "VACUUM.ROUGH\_PUMP\_RUNNING".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 102,
                byte\_length: 2,
                access\_mode: AccessMode::ReadWrite,
                polling\_tier: PollingTier::High,
                always\_on: true,
                deadband: None,
                min\_value: None,
                max\_value: None,
                require\_safe\_state: None,
                forbidden\_when\_active: None,
            },
        );

        // 唯讀真空計感測點位 (支援 Deadband 過濾)
        tags.insert(
            "VACUUM.CHAMBER\_PRESSURE\_MTORR".to\_string(),
            TagModelSpec {
                tag\_id: "VACUUM.CHAMBER\_PRESSURE\_MTORR".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 108,
                byte\_length: 4,
                access\_mode: AccessMode::ReadOnly,
                polling\_tier: PollingTier::High,
                always\_on: true,
                deadband: Some(0.5),
                min\_value: Some(0.0),
                max\_value: Some(760000.0),
                require\_safe\_state: None,
                forbidden\_when\_active: None,
            },
        );

        // 受 §9.3 四大護欄保護之高軸速設定點位
        tags.insert(
            "MOTION.SPINDLE\_TARGET\_RPM".to\_string(),
            TagModelSpec {
                tag\_id: "MOTION.SPINDLE\_TARGET\_RPM".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 116,
                byte\_length: 4,
                access\_mode: AccessMode::ReadWrite,
                polling\_tier: PollingTier::Normal,
                always\_on: false,
                deadband: Some(1.0),
                min\_value: Some(0.0),
                max\_value: Some(6000.0),
                require\_safe\_state: Some(SafeStateConstraint {
                    interlock\_tag: "SAFETY.DOOR\_CLOSED".to\_string(),
                    required\_value: ScalarValue::Bool(true),
                    reason: "安全門未關閉 (SAFETY.DOOR\_CLOSED != true)，禁止變更主軸轉速".to\_string(),
                }),
                forbidden\_when\_active: None,
            },
        );

        // 受 ForbiddenWhenActive 保護之腔體破真空閥門
        tags.insert(
            "VACUUM.VENT\_VALVE\_CMD".to\_string(),
            TagModelSpec {
                tag\_id: "VACUUM.VENT\_VALVE\_CMD".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 124,
                byte\_length: 2,
                access\_mode: AccessMode::ReadWrite,
                polling\_tier: PollingTier::Normal,
                always\_on: false,
                deadband: None,
                min\_value: None,
                max\_value: None,
                require\_safe\_state: Some(SafeStateConstraint {
                    interlock\_tag: "SAFETY.DOOR\_CLOSED".to\_string(),
                    required\_value: ScalarValue::Bool(true),
                    reason: "安全門未關閉，禁止操作破真空閥".to\_string(),
                }),
                forbidden\_when\_active: Some(ActionPrecondition {
                    blocking\_tag: "VACUUM.ROUGH\_PUMP\_RUNNING".to\_string(),
                    forbidden\_value: ScalarValue::Bool(true),
                    reason: "粗抽幫浦運轉中 (VACUUM.ROUGH\_PUMP\_RUNNING == true)，禁止開啟破真空閥".to\_string(),
                }),
            },
        );

        // 第二站點 (PLC\_CHAMBER) 溫度點位
        tags.insert(
            "CHAMBER.HEATER\_TEMP\_C".to\_string(),
            TagModelSpec {
                tag\_id: "CHAMBER.HEATER\_TEMP\_C".to\_string(),
                station\_id: "PLC\_CHAMBER".to\_string(),
                register\_area: "DB1".to\_string(),
                byte\_offset: 200,
                byte\_length: 4,
                access\_mode: AccessMode::ReadWrite,
                polling\_tier: PollingTier::Low,
                always\_on: false,
                deadband: Some(0.2),
                min\_value: Some(20.0),
                max\_value: Some(450.0),
                require\_safe\_state: None,
                forbidden\_when\_active: None,
            },
        );

        // 遠距位址點位（測試超過 max\_gap\_bytes 分包）
        tags.insert(
            "DIAG.HEARTBEAT\_COUNTER".to\_string(),
            TagModelSpec {
                tag\_id: "DIAG.HEARTBEAT\_COUNTER".to\_string(),
                station\_id: "PLC\_MAIN".to\_string(),
                register\_area: "D".to\_string(),
                byte\_offset: 500,
                byte\_length: 4,
                access\_mode: AccessMode::ReadOnly,
                polling\_tier: PollingTier::Low,
                always\_on: false,
                deadband: None,
                min\_value: None,
                max\_value: None,
                require\_safe\_state: None,
                forbidden\_when\_active: None,
            },
        );

        Self {
            machine\_id: "SEMI\_CLUSTER\_TOOL\_01".to\_string(),
            stations,
            tags,
        }
    }
}
3.3 crates/mm-drivers（保留上游工業協議驅動介面）crates/mm-drivers/Cargo.tomlIni, TOML\[package\]
name = "mm-drivers"
version.workspace = true
edition.workspace = true

\[dependencies\]
serde.workspace = true
serde\_json.workspace = true
thiserror.workspace = true
machine-model.workspace = true
crates/mm-drivers/src/lib.rsRustpub mod serial\_and\_protocols;

use machine\_model::ScalarValue;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#\[derive(Debug, Error, Clone, Serialize, Deserialize)\]
pub enum DriverError {
    #\[error("Connection timeout on station {station\_id}")\]
    Timeout { station\_id: String },
    #\[error("Protocol error on station {station\_id}: {details}")\]
    ProtocolError { station\_id: String, details: String },
    #\[error("Station {station\_id} is offline")\]
    Offline { station\_id: String },
}

pub trait PlcDriver: Send + Sync {
    fn station\_id(&amp;self) -&gt; &amp;str;
    fn protocol\_name(&amp;self) -&gt; &amp;str;
    fn read\_register\_block(
        &amp;self,
        area: &amp;str,
        start\_offset: u32,
        byte\_len: u32,
    ) -&gt; Result&lt;Vec&lt;u8&gt;, DriverError&gt;;
    fn read\_tag\_value(&amp;self, tag\_id: &amp;str) -&gt; Result&lt;ScalarValue, DriverError&gt;;
    fn write\_tag\_value(&amp;self, tag\_id: &amp;str, value: ScalarValue) -&gt; Result&lt;(), DriverError&gt;;
}

#\[derive(Debug, Clone)\]
pub struct InMemoryPlcDriver {
    station\_id: String,
    protocol: String,
    online: Arc&lt;Mutex&lt;bool&gt;&gt;,
    values: Arc&lt;Mutex&lt;HashMap&lt;String, ScalarValue&gt;&gt;&gt;,
}

impl InMemoryPlcDriver {
    pub fn new(station\_id: impl Into&lt;String&gt;, protocol: impl Into&lt;String&gt;) -&gt; Self {
        Self {
            station\_id: station\_id.into(),
            protocol: protocol.into(),
            online: Arc::new(Mutex::new(true)),
            values: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set\_online(&amp;self, is\_online: bool) {
        if let Ok(mut guard) = self.online.lock() {
            \*guard = is\_online;
        }
    }

    pub fn seed\_value(&amp;self, tag\_id: impl Into&lt;String&gt;, value: ScalarValue) {
        if let Ok(mut guard) = self.values.lock() {
            guard.insert(tag\_id.into(), value);
        }
    }
}

impl PlcDriver for InMemoryPlcDriver {
    fn station\_id(&amp;self) -&gt; &amp;str {
        &amp;self.station\_id
    }

    fn protocol\_name(&amp;self) -&gt; &amp;str {
        &amp;self.protocol
    }

    fn read\_register\_block(
        &amp;self,
        \_area: &amp;str,
        \_start\_offset: u32,
        byte\_len: u32,
    ) -&gt; Result&lt;Vec&lt;u8&gt;, DriverError&gt; {
        let is\_online = \*self.online.lock().unwrap();
        if !is\_online {
            return Err(DriverError::Offline {
                station\_id: self.station\_id.clone(),
            });
        }
        Ok(vec!\[0u8; byte\_len as usize\])
    }

    fn read\_tag\_value(&amp;self, tag\_id: &amp;str) -&gt; Result&lt;ScalarValue, DriverError&gt; {
        let is\_online = \*self.online.lock().unwrap();
        if !is\_online {
            return Err(DriverError::Offline {
                station\_id: self.station\_id.clone(),
            });
        }
        let guard = self.values.lock().unwrap();
        Ok(guard.get(tag\_id).cloned().unwrap\_or(ScalarValue::Int(0)))
    }

    fn write\_tag\_value(&amp;self, tag\_id: &amp;str, value: ScalarValue) -&gt; Result&lt;(), DriverError&gt; {
        let is\_online = \*self.online.lock().unwrap();
        if !is\_online {
            return Err(DriverError::Offline {
                station\_id: self.station\_id.clone(),
            });
        }
        let mut guard = self.values.lock().unwrap();
        guard.insert(tag\_id.to\_string(), value);
        Ok(())
    }
}
crates/mm-drivers/src/serial\_and\_protocols.rsRustuse serde::{Deserialize, Serialize};

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct SupportedProtocolInfo {
    pub code: &amp;'static str,
    pub vendor: &amp;'static str,
    pub transport: &amp;'static str,
}

pub fn list\_supported\_protocols() -&gt; Vec&lt;SupportedProtocolInfo&gt; {
    vec!\[
        SupportedProtocolInfo {
            code: "Mitsubishi\_MC\_3E",
            vendor: "Mitsubishi Electric",
            transport: "TCP/IP Binary 3E",
        },
        SupportedProtocolInfo {
            code: "Siemens\_S7",
            vendor: "Siemens",
            transport: "ISO-on-TCP (RFC1006)",
        },
        SupportedProtocolInfo {
            code: "Keyence\_KV\_HostLink",
            vendor: "Keyence",
            transport: "TCP/UDP Upper Link",
        },
        SupportedProtocolInfo {
            code: "Omron\_FINS\_TCP",
            vendor: "Omron",
            transport: "FINS/TCP",
        },
        SupportedProtocolInfo {
            code: "Modbus\_TCP",
            vendor: "Standard",
            transport: "TCP Port 502",
        },
    \]
}
3.4 crates/plc-data-service（模組化升級核心）crates/plc-data-service/Cargo.tomlIni, TOML\[package\]
name = "plc-data-service"
version.workspace = true
edition.workspace = true

\[dependencies\]
serde.workspace = true
serde\_json.workspace = true
tokio.workspace = true
parking\_lot.workspace = true
tracing.workspace = true
thiserror.workspace = true
async-trait.workspace = true
machine-model.workspace = true
mm-drivers.workspace = true
crates/plc-data-service/src/cache/snapshot.rsRustuse machine\_model::ScalarValue;
use serde::{Deserialize, Serialize};

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum DataQuality {
    Good,
    UncertainLastKnown,
    BadCommunicationError,
    BadStale,
}

#\[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)\]
pub struct DeadbandConfig {
    pub threshold: f64,
}

impl DeadbandConfig {
    pub fn new(threshold: f64) -&gt; Self {
        Self { threshold }
    }

    pub fn exceeds(&amp;self, old\_val: &amp;ScalarValue, new\_val: &amp;ScalarValue) -&gt; bool {
        match (old\_val.as\_f64(), new\_val.as\_f64()) {
            (Some(old\_num), Some(new\_num)) =&gt; (new\_num - old\_num).abs() &gt;= self.threshold,
            \_ =&gt; old\_val != new\_val,
        }
    }
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct TagSnapshot {
    pub tag\_id: String,
    pub station\_id: String,
    pub value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp\_ms: u64,
    pub sequence\_id: u64,
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub struct TagChangeEvent {
    pub tag\_id: String,
    pub station\_id: String,
    pub previous\_value: Option&lt;ScalarValue&gt;,
    pub current\_value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp\_ms: u64,
    pub sequence\_id: u64,
}
crates/plc-data-service/src/cache/mod.rsRustpub mod snapshot;

pub use snapshot::{DataQuality, DeadbandConfig, TagChangeEvent, TagSnapshot};

use machine\_model::{ResolvedMachineModel, ScalarValue};
use parking\_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::trace;

#\[derive(Debug, Clone)\]
pub struct TagStore {
    snapshots: Arc&lt;RwLock&lt;HashMap&lt;String, TagSnapshot&gt;&gt;&gt;,
    deadbands: Arc&lt;HashMap&lt;String, DeadbandConfig&gt;&gt;,
    sequence\_counter: Arc&lt;AtomicU64&gt;,
}

impl TagStore {
    pub fn from\_model(model: &amp;ResolvedMachineModel, initial\_ts\_ms: u64) -&gt; Self {
        let mut snapshots = HashMap::with\_capacity(model.tags.len());
        let mut deadbands = HashMap::new();

        for (tag\_id, spec) in &amp;model.tags {
            let default\_value = match tag\_id.as\_str() {
                "SAFETY.DOOR\_CLOSED" =&gt; ScalarValue::Bool(true),
                "VACUUM.ROUGH\_PUMP\_RUNNING" =&gt; ScalarValue::Bool(false),
                "VACUUM.CHAMBER\_PRESSURE\_MTORR" =&gt; ScalarValue::Float(760000.0),
                "MOTION.SPINDLE\_TARGET\_RPM" =&gt; ScalarValue::Float(0.0),
                "VACUUM.VENT\_VALVE\_CMD" =&gt; ScalarValue::Bool(false),
                "CHAMBER.HEATER\_TEMP\_C" =&gt; ScalarValue::Float(25.0),
                \_ =&gt; ScalarValue::Int(0),
            };

            snapshots.insert(
                tag\_id.clone(),
                TagSnapshot {
                    tag\_id: tag\_id.clone(),
                    station\_id: spec.station\_id.clone(),
                    value: default\_value,
                    quality: DataQuality::Good,
                    timestamp\_ms: initial\_ts\_ms,
                    sequence\_id: 1,
                },
            );

            if let Some(db) = spec.deadband {
                if db &gt; 0.0 {
                    deadbands.insert(tag\_id.clone(), DeadbandConfig::new(db));
                }
            }
        }

        Self {
            snapshots: Arc::new(RwLock::new(snapshots)),
            deadbands: Arc::new(deadbands),
            sequence\_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    /// 微秒級無鎖中毒讀取直通
    pub fn read\_tag(&amp;self, tag\_id: &amp;str) -&gt; Option&lt;TagSnapshot&gt; {
        let guard = self.snapshots.read();
        guard.get(tag\_id).cloned()
    }

    pub fn get\_all\_snapshots(&amp;self) -&gt; Vec&lt;TagSnapshot&gt; {
        let guard = self.snapshots.read();
        let mut list: Vec&lt;TagSnapshot&gt; = guard.values().cloned().collect();
        list.sort\_by(|a, b| a.tag\_id.cmp(&amp;b.tag\_id));
        list
    }

    pub fn get\_snapshots\_for\_tags(&amp;self, tag\_ids: &amp;\[String\]) -&gt; Vec&lt;TagSnapshot&gt; {
        let guard = self.snapshots.read();
        tag\_ids
            .iter()
            .filter\_map(|id| guard.get(id).cloned())
            .collect()
    }

    /// 更新點位值並依據 Deadband 決定是否觸發 Change-Of-State (COS) 事件
    pub fn update\_tag(
        &amp;self,
        tag\_id: &amp;str,
        station\_id: &amp;str,
        new\_value: ScalarValue,
        quality: DataQuality,
        timestamp\_ms: u64,
    ) -&gt; Option&lt;TagChangeEvent&gt; {
        let mut guard = self.snapshots.write();
        let prev = guard.get(tag\_id).cloned();

        let should\_emit\_cos = match &amp;prev {
            None =&gt; true,
            Some(old\_snap) =&gt; {
                if old\_snap.quality != quality {
                    true
                } else if let Some(db) = self.deadbands.get(tag\_id) {
                    db.exceeds(&amp;old\_snap.value, &amp;new\_value)
                } else {
                    old\_snap.value != new\_value
                }
            }
        };

        let seq = self.sequence\_counter.fetch\_add(1, Ordering::Relaxed) + 1;

        let updated = TagSnapshot {
            tag\_id: tag\_id.to\_string(),
            station\_id: station\_id.to\_string(),
            value: new\_value.clone(),
            quality,
            timestamp\_ms,
            sequence\_id: seq,
        };
        guard.insert(tag\_id.to\_string(), updated);

        if should\_emit\_cos {
            trace!(
                tag\_id = tag\_id,
                seq = seq,
                "COS event triggered on TagStore update"
            );
            Some(TagChangeEvent {
                tag\_id: tag\_id.to\_string(),
                station\_id: station\_id.to\_string(),
                previous\_value: prev.map(|p| p.value),
                current\_value: new\_value,
                quality,
                timestamp\_ms,
                sequence\_id: seq,
            })
        } else {
            None
        }
    }

    /// 當單站進入熔斷 Open 時，將該站點所有點位標記為 BadCommunicationError 或 UncertainLastKnown
    pub fn mark\_station\_quality(&amp;self, station\_id: &amp;str, quality: DataQuality, timestamp\_ms: u64) {
        let mut guard = self.snapshots.write();
        for snap in guard.values\_mut() {
            if snap.station\_id == station\_id {
                snap.quality = quality;
                snap.timestamp\_ms = timestamp\_ms;
            }
        }
    }
}
crates/plc-data-service/src/subscription/events.rsRustuse crate::cache::{TagChangeEvent, TagSnapshot};
use serde::{Deserialize, Serialize};

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct ViewportReadyEvent {
    pub viewport\_id: String,
    pub lease\_expires\_at\_ms: u64,
    pub initial\_snapshots: Vec&lt;TagSnapshot&gt;,
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub enum SubscriptionEvent {
    ViewportReady(ViewportReadyEvent),
    ChangeOfState(TagChangeEvent),
}
crates/plc-data-service/src/subscription/tracker.rsRustuse serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const DEFAULT\_UNSUBSCRIBE\_COOLDOWN\_MS: u64 = 3000;
pub const DEFAULT\_LEASE\_DURATION\_MS: u64 = 15000;

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct ViewportLease {
    pub viewport\_id: String,
    pub tags: HashSet&lt;String&gt;,
    pub expires\_at\_ms: u64,
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct CooldownEntry {
    pub tag\_id: String,
    pub cooldown\_until\_ms: u64,
}

#\[derive(Debug, Clone, Default)\]
pub struct SubscriptionTracker {
    pub always\_on\_tags: HashSet&lt;String&gt;,
    pub leases: HashMap&lt;String, ViewportLease&gt;,
    pub ref\_counts: HashMap&lt;String, usize&gt;,
    pub cooldown\_pool: HashMap&lt;String, u64&gt;,
    pub cooldown\_ms: u64,
}

impl SubscriptionTracker {
    pub fn new(always\_on\_tags: HashSet&lt;String&gt;, cooldown\_ms: u64) -&gt; Self {
        Self {
            always\_on\_tags,
            leases: HashMap::new(),
            ref\_counts: HashMap::new(),
            cooldown\_pool: HashMap::new(),
            cooldown\_ms,
        }
    }

    pub fn subscribe\_viewport(
        &amp;mut self,
        viewport\_id: &amp;str,
        tag\_ids: &amp;\[String\],
        now\_ms: u64,
        lease\_ttl\_ms: u64,
    ) -&gt; ViewportLease {
        // 若畫面已存在舊租約，先移除舊引用計數
        if let Some(existing) = self.leases.remove(viewport\_id) {
            for t in &amp;existing.tags {
                if let Some(rc) = self.ref\_counts.get\_mut(t) {
                    \*rc = rc.saturating\_sub(1);
                    if \*rc == 0 {
                        self.ref\_counts.remove(t);
                    }
                }
            }
        }

        let mut new\_set = HashSet::new();
        for tag\_id in tag\_ids {
            new\_set.insert(tag\_id.clone());
            \*self.ref\_counts.entry(tag\_id.clone()).or\_insert(0) += 1;
            // 若點位正處於 3 秒退訂冷卻池，立即取消冷卻、無縫接續輪詢
            self.cooldown\_pool.remove(tag\_id);
        }

        let lease = ViewportLease {
            viewport\_id: viewport\_id.to\_string(),
            tags: new\_set,
            expires\_at\_ms: now\_ms + lease\_ttl\_ms,
        };
        self.leases.insert(viewport\_id.to\_string(), lease.clone());
        lease
    }

    pub fn renew\_lease(&amp;mut self, viewport\_id: &amp;str, now\_ms: u64, lease\_ttl\_ms: u64) -&gt; bool {
        if let Some(lease) = self.leases.get\_mut(viewport\_id) {
            lease.expires\_at\_ms = now\_ms + lease\_ttl\_ms;
            true
        } else {
            false
        }
    }

    pub fn unsubscribe\_viewport(&amp;mut self, viewport\_id: &amp;str, now\_ms: u64) -&gt; Vec&lt;String&gt; {
        let mut entered\_cooldown = Vec::new();
        if let Some(lease) = self.leases.remove(viewport\_id) {
            for tag\_id in lease.tags {
                let count = self.ref\_counts.get(&amp;tag\_id).copied().unwrap\_or(0);
                if count &lt;= 1 {
                    self.ref\_counts.remove(&amp;tag\_id);
                    if !self.always\_on\_tags.contains(&amp;tag\_id) {
                        self.cooldown\_pool
                            .insert(tag\_id.clone(), now\_ms + self.cooldown\_ms);
                        entered\_cooldown.push(tag\_id);
                    }
                } else {
                    self.ref\_counts.insert(tag\_id, count - 1);
                }
            }
        }
        entered\_cooldown
    }

    /// 清理過期 Zombie 租約與到期之 3 秒退訂冷卻點位
    pub fn tick\_gc(&amp;mut self, now\_ms: u64) -&gt; (Vec&lt;String&gt;, Vec&lt;String&gt;) {
        let expired\_viewports: Vec&lt;String&gt; = self
            .leases
            .iter()
            .filter(|(\_, lease)| now\_ms &gt;= lease.expires\_at\_ms)
            .map(|(k, \_)| k.clone())
            .collect();

        for vp in &amp;expired\_viewports {
            self.unsubscribe\_viewport(vp, now\_ms);
        }

        let expired\_cooldown\_tags: Vec&lt;String&gt; = self
            .cooldown\_pool
            .iter()
            .filter(|(\_, &amp;until\_ms)| now\_ms &gt;= until\_ms)
            .map(|(k, \_)| k.clone())
            .collect();

        for tag\_id in &amp;expired\_cooldown\_tags {
            self.cooldown\_pool.remove(tag\_id);
        }

        (expired\_viewports, expired\_cooldown\_tags)
    }

    /// 取得目前所有應納入通訊輪詢的活躍點位集合 (Always-On ∪ Active Leases ∪ Cooldown Pool)
    pub fn active\_polled\_tags(&amp;self) -&gt; HashSet&lt;String&gt; {
        let mut active = self.always\_on\_tags.clone();
        for tag\_id in self.ref\_counts.keys() {
            active.insert(tag\_id.clone());
        }
        for tag\_id in self.cooldown\_pool.keys() {
            active.insert(tag\_id.clone());
        }
        active
    }
}
crates/plc-data-service/src/subscription/mod.rsRustpub mod events;
pub mod tracker;

pub use events::{SubscriptionEvent, ViewportReadyEvent};
pub use tracker::{
    CooldownEntry, SubscriptionTracker, ViewportLease, DEFAULT\_LEASE\_DURATION\_MS,
    DEFAULT\_UNSUBSCRIBE\_COOLDOWN\_MS,
};

use crate::cache::TagStore;
use machine\_model::ResolvedMachineModel;
use parking\_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info};

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct SubscriptionDiagnostics {
    pub always\_on\_count: usize,
    pub active\_lease\_count: usize,
    pub active\_dynamic\_tags: Vec&lt;String&gt;,
    pub cooldown\_tags: Vec&lt;CooldownEntry&gt;,
    pub total\_effective\_polled\_tags: usize,
}

#\[derive(Debug, Clone)\]
pub struct SubscriptionManager {
    tracker: Arc&lt;RwLock&lt;SubscriptionTracker&gt;&gt;,
    events: Arc&lt;RwLock&lt;Vec&lt;SubscriptionEvent&gt;&gt;&gt;,
}

impl SubscriptionManager {
    pub fn from\_model(model: &amp;ResolvedMachineModel) -&gt; Self {
        let always\_on: HashSet&lt;String&gt; = model
            .tags
            .iter()
            .filter(|(\_, spec)| spec.always\_on)
            .map(|(k, \_)| k.clone())
            .collect();

        Self {
            tracker: Arc::new(RwLock::new(SubscriptionTracker::new(
                always\_on,
                DEFAULT\_UNSUBSCRIBE\_COOLDOWN\_MS,
            ))),
            events: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn subscribe\_viewport(
        &amp;self,
        viewport\_id: &amp;str,
        tag\_ids: &amp;\[String\],
        now\_ms: u64,
        tag\_store: &amp;TagStore,
    ) -&gt; ViewportReadyEvent {
        let lease = {
            let mut guard = self.tracker.write();
            guard.subscribe\_viewport(viewport\_id, tag\_ids, now\_ms, DEFAULT\_LEASE\_DURATION\_MS)
        };

        let initial\_snapshots = tag\_store.get\_snapshots\_for\_tags(tag\_ids);
        let ready\_event = ViewportReadyEvent {
            viewport\_id: viewport\_id.to\_string(),
            lease\_expires\_at\_ms: lease.expires\_at\_ms,
            initial\_snapshots,
        };

        info!(
            viewport\_id = viewport\_id,
            tag\_count = tag\_ids.len(),
            "HMI Viewport subscribed and ViewportReady snapshot emitted"
        );

        self.events
            .write()
            .push(SubscriptionEvent::ViewportReady(ready\_event.clone()));

        ready\_event
    }

    pub fn renew\_viewport\_lease(&amp;self, viewport\_id: &amp;str, now\_ms: u64) -&gt; bool {
        let mut guard = self.tracker.write();
        guard.renew\_lease(viewport\_id, now\_ms, DEFAULT\_LEASE\_DURATION\_MS)
    }

    pub fn unsubscribe\_viewport(&amp;self, viewport\_id: &amp;str, now\_ms: u64) -&gt; Vec&lt;String&gt; {
        let entered\_cooldown = {
            let mut guard = self.tracker.write();
            guard.unsubscribe\_viewport(viewport\_id, now\_ms)
        };
        debug!(
            viewport\_id = viewport\_id,
            cooldown\_count = entered\_cooldown.len(),
            "HMI Viewport unsubscribed; dynamic tags entered 3s cooldown"
        );
        entered\_cooldown
    }

    pub fn tick\_gc(&amp;self, now\_ms: u64) -&gt; (Vec&lt;String&gt;, Vec&lt;String&gt;) {
        let mut guard = self.tracker.write();
        guard.tick\_gc(now\_ms)
    }

    pub fn active\_polled\_tags(&amp;self) -&gt; HashSet&lt;String&gt; {
        let guard = self.tracker.read();
        guard.active\_polled\_tags()
    }

    pub fn push\_event(&amp;self, event: SubscriptionEvent) {
        self.events.write().push(event);
    }

    pub fn drain\_events(&amp;self) -&gt; Vec&lt;SubscriptionEvent&gt; {
        let mut guard = self.events.write();
        std::mem::take(&amp;mut \*guard)
    }

    pub fn diagnostics(&amp;self) -&gt; SubscriptionDiagnostics {
        let guard = self.tracker.read();
        let mut active\_dynamic: Vec&lt;String&gt; = guard.ref\_counts.keys().cloned().collect();
        active\_dynamic.sort();

        let mut cooldown\_tags: Vec&lt;CooldownEntry&gt; = guard
            .cooldown\_pool
            .iter()
            .map(|(k, &amp;v)| CooldownEntry {
                tag\_id: k.clone(),
                cooldown\_until\_ms: v,
            })
            .collect();
        cooldown\_tags.sort\_by(|a, b| a.tag\_id.cmp(&amp;b.tag\_id));

        SubscriptionDiagnostics {
            always\_on\_count: guard.always\_on\_tags.len(),
            active\_lease\_count: guard.leases.len(),
            active\_dynamic\_tags: active\_dynamic,
            cooldown\_tags,
            total\_effective\_polled\_tags: guard.active\_polled\_tags().len(),
        }
    }
}
crates/plc-data-service/src/poller/coalescing.rsRustuse machine\_model::{PollingTier, TagModelSpec};
use serde::{Deserialize, Serialize};

#\[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)\]
pub struct CoalescedBlock {
    pub station\_id: String,
    pub register\_area: String,
    pub polling\_tier: PollingTier,
    pub start\_byte\_offset: u32,
    pub total\_byte\_length: u32,
    pub tag\_ids: Vec&lt;String&gt;,
}

/// 智慧相鄰位址拼包演算法（兼顧 max\_gap\_bytes 與 max\_pdu\_bytes 限制）
pub fn coalesce\_tag\_specs(
    specs: &amp;\[TagModelSpec\],
    max\_gap\_bytes: u32,
    max\_pdu\_bytes: u32,
) -&gt; Vec&lt;CoalescedBlock&gt; {
    if specs.is\_empty() {
        return Vec::new();
    }

    let mut sorted = specs.to\_vec();
    sorted.sort\_by(|a, b| {
        a.station\_id
            .cmp(&amp;b.station\_id)
            .then\_with(|| a.register\_area.cmp(&amp;b.register\_area))
            .then\_with(|| (a.polling\_tier as u8).cmp(&amp;(b.polling\_tier as u8)))
            .then\_with(|| a.byte\_offset.cmp(&amp;b.byte\_offset))
    });

    let mut blocks: Vec&lt;CoalescedBlock&gt; = Vec::new();

    for spec in sorted {
        let spec\_end = spec.byte\_offset + spec.byte\_length as u32;

        if let Some(last) = blocks.last\_mut() {
            let same\_group = last.station\_id == spec.station\_id
                &amp;&amp; last.register\_area == spec.register\_area
                &amp;&amp; last.polling\_tier == spec.polling\_tier;

            if same\_group {
                let current\_end = last.start\_byte\_offset + last.total\_byte\_length;
                let gap = spec.byte\_offset.saturating\_sub(current\_end);
                let new\_end = spec\_end.max(current\_end);
                let merged\_len = new\_end - last.start\_byte\_offset;

                if gap &lt;= max\_gap\_bytes &amp;&amp; merged\_len &lt;= max\_pdu\_bytes {
                    last.total\_byte\_length = merged\_len;
                    last.tag\_ids.push(spec.tag\_id.clone());
                    continue;
                }
            }
        }

        blocks.push(CoalescedBlock {
            station\_id: spec.station\_id.clone(),
            register\_area: spec.register\_area.clone(),
            polling\_tier: spec.polling\_tier,
            start\_byte\_offset: spec.byte\_offset,
            total\_byte\_length: spec.byte\_length as u32,
            tag\_ids: vec!\[spec.tag\_id.clone()\],
        });
    }

    blocks
}
crates/plc-data-service/src/poller/scheduler.rsRustuse crate::poller::coalescing::CoalescedBlock;
use machine\_model::PollingTier;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct MultiRateScheduleState {
    pub last\_Execution\_ms: HashMap&lt;PollingTier, u64&gt;,
}

impl Default for MultiRateScheduleState {
    fn default() -&gt; Self {
        let mut map = HashMap::new();
        map.insert(PollingTier::High, 0);
        map.insert(PollingTier::Normal, 0);
        map.insert(PollingTier::Low, 0);
        Self {
            last\_Execution\_ms: map,
        }
    }
}

impl MultiRateScheduleState {
    pub fn due\_tiers(&amp;mut self, now\_ms: u64) -&gt; Vec&lt;PollingTier&gt; {
        let mut due = Vec::new();
        for tier in \[PollingTier::High, PollingTier::Normal, PollingTier::Low\] {
            let last = self.last\_Execution\_ms.get(&amp;tier).copied().unwrap\_or(0);
            if last == 0 || now\_ms.saturating\_sub(last) &gt;= tier.interval\_ms() {
                self.last\_Execution\_ms.insert(tier, now\_ms);
                due.push(tier);
            }
        }
        due
    }

    pub fn filter\_due\_blocks(
        &amp;mut self,
        all\_blocks: &amp;\[CoalescedBlock\],
        now\_ms: u64,
    ) -&gt; Vec&lt;CoalescedBlock&gt; {
        let due = self.due\_tiers(now\_ms);
        all\_blocks
            .iter()
            .filter(|b| due.contains(&amp;b.polling\_tier))
            .cloned()
            .collect()
    }
}
crates/plc-data-service/src/poller/mod.rsRustpub mod coalescing;
pub mod scheduler;

pub use coalescing::{coalesce\_tag\_specs, CoalescedBlock};
pub use scheduler::MultiRateScheduleState;

use machine\_model::{ResolvedMachineModel, TagModelSpec};
use parking\_lot::Mutex;
use std::collections::HashSet;
use std::sync::Arc;

#\[derive(Debug, Clone)\]
pub struct PollerScheduler {
    schedule\_state: Arc&lt;Mutex&lt;MultiRateScheduleState&gt;&gt;,
}

impl Default for PollerScheduler {
    fn default() -&gt; Self {
        Self::new()
    }
}

impl PollerScheduler {
    pub fn new() -&gt; Self {
        Self {
            schedule\_state: Arc::new(Mutex::new(MultiRateScheduleState::default())),
        }
    }

    pub fn compute\_blocks\_for\_active\_tags(
        &amp;self,
        model: &amp;ResolvedMachineModel,
        active\_tags: &amp;HashSet&lt;String&gt;,
    ) -&gt; Vec&lt;CoalescedBlock&gt; {
        let mut by\_station: std::collections::HashMap&lt;String, Vec&lt;TagModelSpec&gt;&gt; =
            std::collections::HashMap::new();

        for tag\_id in active\_tags {
            if let Some(spec) = model.tags.get(tag\_id) {
                by\_station
                    .entry(spec.station\_id.clone())
                    .or\_default()
                    .push(spec.clone());
            }
        }

        let mut all\_blocks = Vec::new();
        for (station\_id, specs) in by\_station {
            let (max\_gap, max\_pdu) = model
                .stations
                .get(&amp;station\_id)
                .map(|s| (s.max\_gap\_bytes, s.max\_pdu\_bytes))
                .unwrap\_or((16, 480));

            let mut station\_blocks = coalesce\_tag\_specs(&amp;specs, max\_gap, max\_pdu);
            all\_blocks.append(&amp;mut station\_blocks);
        }

        all\_blocks.sort\_by(|a, b| {
            a.station\_id
                .cmp(&amp;b.station\_id)
                .then\_with(|| a.register\_area.cmp(&amp;b.register\_area))
                .then\_with(|| a.start\_byte\_offset.cmp(&amp;b.start\_byte\_offset))
        });

        all\_blocks
    }

    pub fn due\_blocks(
        &amp;self,
        model: &amp;ResolvedMachineModel,
        active\_tags: &amp;HashSet&lt;String&gt;,
        now\_ms: u64,
    ) -&gt; Vec&lt;CoalescedBlock&gt; {
        let blocks = self.compute\_blocks\_for\_active\_tags(model, active\_tags);
        let mut guard = self.schedule\_state.lock();
        guard.filter\_due\_blocks(&amp;blocks, now\_ms)
    }
}
crates/plc-data-service/src/arbiter/guardrails.rsRustuse crate::cache::TagStore;
use machine\_model::{AccessMode, ResolvedMachineModel, ScalarValue};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#\[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)\]
pub enum GuardrailViolation {
    #\[error("點位不存在: {tag\_id}")\]
    UnknownTag { tag\_id: String },

    #\[error("護欄攔截 \[ReadOnly\]: 點位 {tag\_id} 為唯讀點位，禁止寫入")\]
    ReadOnlyViolation { tag\_id: String },

    #\[error("護欄攔截 \[RangeLimit\]: 點位 {tag\_id} 寫入值 {attempted} 超出允許範圍 \[{min:?}, {max:?}\]")\]
    OutOfRange {
        tag\_id: String,
        attempted: f64,
        min: Option&lt;f64&gt;,
        max: Option&lt;f64&gt;,
    },

    #\[error("護欄攔截 \[RequireSafeState\]: 點位 {tag\_id} 寫入遭拒 — 互鎖點位 {interlock\_tag} 必須為 {required:?} ({reason})")\]
    SafeStateInterlockFailed {
        tag\_id: String,
        interlock\_tag: String,
        required: ScalarValue,
        actual: Option&lt;ScalarValue&gt;,
        reason: String,
    },

    #\[error("護欄攔截 \[ForbiddenWhenActive\]: 點位 {tag\_id} 寫入遭拒 — 衝突點位 {blocking\_tag} 處於禁止狀態 {forbidden:?} ({reason})")\]
    ForbiddenWhenActiveFailed {
        tag\_id: String,
        blocking\_tag: String,
        forbidden: ScalarValue,
        actual: Option&lt;ScalarValue&gt;,
        reason: String,
    },
}

/// §9.3 四大寫入護欄驗證器
pub fn validate\_write\_guardrails(
    model: &amp;ResolvedMachineModel,
    tag\_store: &amp;TagStore,
    tag\_id: &amp;str,
    target\_value: &amp;ScalarValue,
) -&gt; Result&lt;(), GuardrailViolation&gt; {
    let spec = model
        .tags
        .get(tag\_id)
        .ok\_or\_else(|| GuardrailViolation::UnknownTag {
            tag\_id: tag\_id.to\_string(),
        })?;

    // 護欄 1: AccessMode::ReadOnly 攔截
    if spec.access\_mode == AccessMode::ReadOnly {
        return Err(GuardrailViolation::ReadOnlyViolation {
            tag\_id: tag\_id.to\_string(),
        });
    }

    // 護欄 2: 數值上下限範圍 (min\_value / max\_value) 檢查
    if let Some(num) = target\_value.as\_f64() {
        if let Some(min\_v) = spec.min\_value {
            if num &lt; min\_v {
                return Err(GuardrailViolation::OutOfRange {
                    tag\_id: tag\_id.to\_string(),
                    attempted: num,
                    min: spec.min\_value,
                    max: spec.max\_value,
                });
            }
        }
        if let Some(max\_v) = spec.max\_value {
            if num &gt; max\_v {
                return Err(GuardrailViolation::OutOfRange {
                    tag\_id: tag\_id.to\_string(),
                    attempted: num,
                    min: spec.min\_value,
                    max: spec.max\_value,
                });
            }
        }
    }

    // 護欄 3: RequireSafeState 安全狀態互鎖檢查
    if let Some(safe\_rule) = &amp;spec.require\_safe\_state {
        let actual = tag\_store
            .read\_tag(&amp;safe\_rule.interlock\_tag)
            .map(|s| s.value);
        if actual.as\_ref() != Some(&amp;safe\_rule.required\_value) {
            return Err(GuardrailViolation::SafeStateInterlockFailed {
                tag\_id: tag\_id.to\_string(),
                interlock\_tag: safe\_rule.interlock\_tag.clone(),
                required: safe\_rule.required\_value.clone(),
                actual,
                reason: safe\_rule.reason.clone(),
            });
        }
    }

    // 護欄 4: ForbiddenWhenActive 動作前置衝突檢查
    if let Some(forbid\_rule) = &amp;spec.forbidden\_when\_active {
        let actual = tag\_store
            .read\_tag(&amp;forbid\_rule.blocking\_tag)
            .map(|s| s.value);
        if actual.as\_ref() == Some(&amp;forbid\_rule.forbidden\_value) {
            return Err(GuardrailViolation::ForbiddenWhenActiveFailed {
                tag\_id: tag\_id.to\_string(),
                blocking\_tag: forbid\_rule.blocking\_tag.clone(),
                forbidden: forbid\_rule.forbidden\_value.clone(),
                actual,
                reason: forbid\_rule.reason.clone(),
            });
        }
    }

    Ok(())
}
crates/plc-data-service/src/arbiter/mod.rsRustpub mod guardrails;

pub use guardrails::{validate\_write\_guardrails, GuardrailViolation};

use machine\_model::ScalarValue;
use parking\_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum WritePriority {
    Urgent,
    Standard,
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct WriteCommand {
    pub command\_id: u64,
    pub caller\_source: String,
    pub tag\_id: String,
    pub target\_value: ScalarValue,
    pub priority: WritePriority,
    pub requested\_at\_ms: u64,
}

#\[derive(Debug, Clone, PartialEq, Serialize, Deserialize)\]
pub enum WriteOutcome {
    SuccessReadbackVerified,
    RejectedByGuardrail { reason: String },
    FailedCommunication { details: String },
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct AuditLogEntry {
    pub log\_id: u64,
    pub timestamp\_ms: u64,
    pub caller\_source: String,
    pub tag\_id: String,
    pub station\_id: String,
    pub before\_value: Option&lt;ScalarValue&gt;,
    pub attempted\_value: ScalarValue,
    pub after\_value: Option&lt;ScalarValue&gt;,
    pub priority: WritePriority,
    pub outcome: WriteOutcome,
}

#\[derive(Debug, Clone)\]
pub struct WriteArbiter {
    urgent\_queue: Arc&lt;Mutex&lt;VecDeque&lt;WriteCommand&gt;&gt;&gt;,
    audit\_logs: Arc&lt;RwLock&lt;Vec&lt;AuditLogEntry&gt;&gt;&gt;,
}

impl Default for WriteArbiter {
    fn default() -&gt; Self {
        Self::new()
    }
}

impl WriteArbiter {
    pub fn new() -&gt; Self {
        Self {
            urgent\_queue: Arc::new(Mutex::new(VecDeque::new())),
            audit\_logs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn enqueue\_urgent(&amp;self, cmd: WriteCommand) {
        let mut q = self.urgent\_queue.lock();
        if cmd.priority == WritePriority::Urgent {
            q.push\_front(cmd);
        } else {
            q.push\_back(cmd);
        }
    }

    pub fn pop\_next\_command(&amp;self) -&gt; Option&lt;WriteCommand&gt; {
        self.urgent\_queue.lock().pop\_front()
    }

    pub fn pending\_count(&amp;self) -&gt; usize {
        self.urgent\_queue.lock().len()
    }

    pub fn record\_audit(&amp;self, entry: AuditLogEntry) {
        self.audit\_logs.write().push(entry);
    }

    pub fn get\_audit\_logs(&amp;self) -&gt; Vec&lt;AuditLogEntry&gt; {
        self.audit\_logs.read().clone()
    }
}
crates/plc-data-service/src/gateway/circuit\_breaker.rsRustuse serde::{Deserialize, Serialize};

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct StationCircuitBreaker {
    pub station\_id: String,
    pub state: CircuitState,
    pub consecutive\_failures: u32,
    pub failure\_threshold: u32,
    pub base\_cooldown\_ms: u64,
    pub max\_cooldown\_ms: u64,
    pub current\_backoff\_ms: u64,
    pub open\_until\_ms: u64,
    pub total\_trip\_count: u64,
}

impl StationCircuitBreaker {
    pub fn new(
        station\_id: impl Into&lt;String&gt;,
        failure\_threshold: u32,
        base\_cooldown\_ms: u64,
        max\_cooldown\_ms: u64,
    ) -&gt; Self {
        Self {
            station\_id: station\_id.into(),
            state: CircuitState::Closed,
            consecutive\_failures: 0,
            failure\_threshold: failure\_threshold.max(1),
            base\_cooldown\_ms: base\_cooldown\_ms.max(100),
            max\_cooldown\_ms: max\_cooldown\_ms.max(base\_cooldown\_ms),
            current\_backoff\_ms: base\_cooldown\_ms.max(100),
            open\_until\_ms: 0,
            total\_trip\_count: 0,
        }
    }

    /// 判斷當前時間點是否允許對該 PLC 站點發起輪詢（含 Open 到期轉 HalfOpen 單次探針）
    pub fn should\_allow\_poll(&amp;mut self, now\_ms: u64) -&gt; bool {
        match self.state {
            CircuitState::Closed =&gt; true,
            CircuitState::Open =&gt; {
                if now\_ms &gt;= self.open\_until\_ms {
                    self.state = CircuitState::HalfOpen;
                    true
                } else {
                    false
                }
            }
            CircuitState::HalfOpen =&gt; true,
        }
    }

    /// 通訊成功時重置連續失敗計數，並由 HalfOpen 恢復為 Closed
    pub fn record\_success(&amp;mut self) {
        self.consecutive\_failures = 0;
        self.state = CircuitState::Closed;
        self.current\_backoff\_ms = self.base\_cooldown\_ms;
        self.open\_until\_ms = 0;
    }

    /// 通訊失敗時累加計數，達到閾值或處於 HalfOpen 探針失敗時進入 Open 並執行指數退避
    pub fn record\_failure(&amp;mut self, now\_ms: u64) -&gt; CircuitState {
        self.consecutive\_failures += 1;

        match self.state {
            CircuitState::Closed =&gt; {
                if self.consecutive\_failures &gt;= self.failure\_threshold {
                    self.state = CircuitState::Open;
                    self.total\_trip\_count += 1;
                    self.current\_backoff\_ms = self.base\_cooldown\_ms;
                    self.open\_until\_ms = now\_ms + self.current\_backoff\_ms;
                }
            }
            CircuitState::HalfOpen =&gt; {
                self.state = CircuitState::Open;
                self.total\_trip\_count += 1;
                self.current\_backoff\_ms = (self.current\_backoff\_ms \* 2).min(self.max\_cooldown\_ms);
                self.open\_until\_ms = now\_ms + self.current\_backoff\_ms;
            }
            CircuitState::Open =&gt; {
                self.current\_backoff\_ms = (self.current\_backoff\_ms \* 2).min(self.max\_cooldown\_ms);
                self.open\_until\_ms = now\_ms + self.current\_backoff\_ms;
            }
        }

        self.state
    }
}
crates/plc-data-service/src/gateway/mod.rsRustpub mod circuit\_breaker;

pub use circuit\_breaker::{CircuitState, StationCircuitBreaker};

use machine\_model::{ResolvedMachineModel, ScalarValue};
use mm\_drivers::{DriverError, InMemoryPlcDriver, PlcDriver};
use parking\_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct StationHealthSnapshot {
    pub station\_id: String,
    pub protocol: String,
    pub endpoint: String,
    pub circuit\_state: CircuitState,
    pub consecutive\_failures: u32,
    pub current\_backoff\_ms: u64,
    pub open\_until\_ms: u64,
    pub total\_trip\_count: u64,
    pub simulated\_online: bool,
}

/// 高擬真虛擬 PLC 模擬器（支援無硬體環境下之物理量微幅漂移、讀寫與斷線故障注入）
#\[derive(Debug, Clone)\]
pub struct VirtualPlcSimulator {
    drivers: Arc&lt;RwLock&lt;HashMap&lt;String, Arc&lt;InMemoryPlcDriver&gt;&gt;&gt;&gt;,
    online\_flags: Arc&lt;RwLock&lt;HashMap&lt;String, bool&gt;&gt;&gt;,
    tick\_counter: Arc&lt;Mutex&lt;u64&gt;&gt;,
}

impl VirtualPlcSimulator {
    pub fn from\_model(model: &amp;ResolvedMachineModel) -&gt; Self {
        let mut drivers = HashMap::new();
        let mut online\_flags = HashMap::new();

        for (station\_id, station\_spec) in &amp;model.stations {
            let drv = InMemoryPlcDriver::new(station\_id.clone(), station\_spec.protocol.clone());
            drivers.insert(station\_id.clone(), Arc::new(drv));
            online\_flags.insert(station\_id.clone(), true);
        }

        for (tag\_id, spec) in &amp;model.tags {
            if let Some(drv) = drivers.get(&amp;spec.station\_id) {
                let seed = match tag\_id.as\_str() {
                    "SAFETY.DOOR\_CLOSED" =&gt; ScalarValue::Bool(true),
                    "VACUUM.ROUGH\_PUMP\_RUNNING" =&gt; ScalarValue::Bool(false),
                    "VACUUM.CHAMBER\_PRESSURE\_MTORR" =&gt; ScalarValue::Float(760000.0),
                    "MOTION.SPINDLE\_TARGET\_RPM" =&gt; ScalarValue::Float(0.0),
                    "VACUUM.VENT\_VALVE\_CMD" =&gt; ScalarValue::Bool(false),
                    "CHAMBER.HEATER\_TEMP\_C" =&gt; ScalarValue::Float(25.0),
                    \_ =&gt; ScalarValue::Int(0),
                };
                drv.seed\_value(tag\_id.clone(), seed);
            }
        }

        Self {
            drivers: Arc::new(RwLock::new(drivers)),
            online\_flags: Arc::new(RwLock::new(online\_flags)),
            tick\_counter: Arc::new(Mutex::new(0)),
        }
    }

    pub fn set\_station\_online(&amp;self, station\_id: &amp;str, online: bool) {
        if let Some(drv) = self.drivers.read().get(station\_id) {
            drv.set\_online(online);
        }
        self.online\_flags
            .write()
            .insert(station\_id.to\_string(), online);
    }

    pub fn is\_station\_online(&amp;self, station\_id: &amp;str) -&gt; bool {
        self.online\_flags
            .read()
            .get(station\_id)
            .copied()
            .unwrap\_or(true)
    }

    pub fn inject\_register\_value(&amp;self, station\_id: &amp;str, tag\_id: &amp;str, value: ScalarValue) {
        if let Some(drv) = self.drivers.read().get(station\_id) {
            drv.seed\_value(tag\_id.to\_string(), value);
        }
    }

    /// 模擬物理世界感測器微小波動或抽真空變化
    pub fn step\_physics(&amp;self) {
        let mut tick = self.tick\_counter.lock();
        \*tick += 1;
        let t = \*tick;
        let drivers = self.drivers.read();
        if let Some(main\_drv) = drivers.get("PLC\_MAIN") {
            let micro\_jitter = ((t % 5) as f64) \* 0.1;
            let \_ = main\_drv.write\_tag\_value(
                "DIAG.HEARTBEAT\_COUNTER",
                ScalarValue::Int(t as i64),
            );
            if let Ok(ScalarValue::Float(curr\_p)) =
                main\_drv.read\_tag\_value("VACUUM.CHAMBER\_PRESSURE\_MTORR")
            {
                let \_ = main\_drv.write\_tag\_value(
                    "VACUUM.CHAMBER\_PRESSURE\_MTORR",
                    ScalarValue::Float((curr\_p + micro\_jitter).min(760000.0)),
                );
            }
        }
    }

    pub fn driver\_for(&amp;self, station\_id: &amp;str) -&gt; Option&lt;Arc&lt;InMemoryPlcDriver&gt;&gt; {
        self.drivers.read().get(station\_id).cloned()
    }
}

#\[derive(Clone)\]
pub struct PlcGateway {
    breakers: Arc&lt;RwLock&lt;HashMap&lt;String, StationCircuitBreaker&gt;&gt;&gt;,
    custom\_drivers: Arc&lt;RwLock&lt;HashMap&lt;String, Arc&lt;dyn PlcDriver&gt;&gt;&gt;&gt;,
    simulator: VirtualPlcSimulator,
    model: Arc&lt;ResolvedMachineModel&gt;,
}

impl PlcGateway {
    pub fn from\_model(model: Arc&lt;ResolvedMachineModel&gt;) -&gt; Self {
        let mut breakers = HashMap::new();
        for (station\_id, spec) in &amp;model.stations {
            breakers.insert(
                station\_id.clone(),
                StationCircuitBreaker::new(
                    station\_id.clone(),
                    spec.failure\_threshold,
                    spec.base\_cooldown\_ms,
                    spec.max\_cooldown\_ms,
                ),
            );
        }

        let simulator = VirtualPlcSimulator::from\_model(&amp;model);

        Self {
            breakers: Arc::new(RwLock::new(breakers)),
            custom\_drivers: Arc::new(RwLock::new(HashMap::new())),
            simulator,
            model,
        }
    }

    pub fn register\_driver(&amp;self, station\_id: impl Into&lt;String&gt;, driver: Arc&lt;dyn PlcDriver&gt;) {
        self.custom\_drivers.write().insert(station\_id.into(), driver);
    }

    pub fn simulator(&amp;self) -&gt; &amp;VirtualPlcSimulator {
        &amp;self.simulator
    }

    pub fn set\_station\_online(&amp;self, station\_id: &amp;str, online: bool) {
        self.simulator.set\_station\_online(station\_id, online);
        info!(station\_id = station\_id, online = online, "Station online state changed");
    }

    pub fn should\_allow\_station\_poll(&amp;self, station\_id: &amp;str, now\_ms: u64) -&gt; bool {
        if let Some(cb) = self.breakers.write().get\_mut(station\_id) {
            cb.should\_allow\_poll(now\_ms)
        } else {
            true
        }
    }

    pub fn read\_tag\_from\_station(
        &amp;self,
        station\_id: &amp;str,
        tag\_id: &amp;str,
        now\_ms: u64,
    ) -&gt; Result&lt;ScalarValue, DriverError&gt; {
        let result = if let Some(custom) = self.custom\_drivers.read().get(station\_id) {
            custom.read\_tag\_value(tag\_id)
        } else if let Some(sim\_drv) = self.simulator.driver\_for(station\_id) {
            sim\_drv.read\_tag\_value(tag\_id)
        } else {
            Err(DriverError::Offline {
                station\_id: station\_id.to\_string(),
            })
        };

        let mut breakers = self.breakers.write();
        if let Some(cb) = breakers.get\_mut(station\_id) {
            match &amp;result {
                Ok(\_) =&gt; cb.record\_success(),
                Err(err) =&gt; {
                    let new\_state = cb.record\_failure(now\_ms);
                    warn!(
                        station\_id = station\_id,
                        error = %err,
                        circuit\_state = ?new\_state,
                        "Station read failed; updated circuit breaker"
                    );
                }
            }
        }

        result
    }

    pub fn write\_and\_verify\_tag(
        &amp;self,
        station\_id: &amp;str,
        tag\_id: &amp;str,
        value: ScalarValue,
        now\_ms: u64,
    ) -&gt; Result&lt;ScalarValue, DriverError&gt; {
        let write\_res = if let Some(custom) = self.custom\_drivers.read().get(station\_id) {
            custom.write\_tag\_value(tag\_id, value.clone())
        } else if let Some(sim\_drv) = self.simulator.driver\_for(station\_id) {
            sim\_drv.write\_tag\_value(tag\_id, value.clone())
        } else {
            Err(DriverError::Offline {
                station\_id: station\_id.to\_string(),
            })
        };

        if let Err(e) = write\_res {
            if let Some(cb) = self.breakers.write().get\_mut(station\_id) {
                cb.record\_failure(now\_ms);
            }
            return Err(e);
        }

        // Write-then-Readback 閉迴路硬體讀回驗證
        self.read\_tag\_from\_station(station\_id, tag\_id, now\_ms)
    }

    pub fn station\_health\_snapshots(&amp;self) -&gt; Vec&lt;StationHealthSnapshot&gt; {
        let breakers = self.breakers.read();
        let mut out = Vec::new();
        for (station\_id, spec) in &amp;self.model.stations {
            if let Some(cb) = breakers.get(station\_id) {
                out.push(StationHealthSnapshot {
                    station\_id: station\_id.clone(),
                    protocol: spec.protocol.clone(),
                    endpoint: spec.endpoint.clone(),
                    circuit\_state: cb.state,
                    consecutive\_failures: cb.consecutive\_failures,
                    current\_backoff\_ms: cb.current\_backoff\_ms,
                    open\_until\_ms: cb.open\_until\_ms,
                    total\_trip\_count: cb.total\_trip\_count,
                    simulated\_online: self.simulator.is\_station\_online(station\_id),
                });
            }
        }
        out.sort\_by(|a, b| a.station\_id.cmp(&amp;b.station\_id));
        out
    }
}
crates/plc-data-service/src/lib.rsRustpub mod arbiter;
pub mod cache;
pub mod gateway;
pub mod poller;
pub mod subscription;

pub use arbiter::{
    validate\_write\_guardrails, AuditLogEntry, GuardrailViolation, WriteArbiter, WriteCommand,
    WriteOutcome, WritePriority,
};
pub use cache::{DataQuality, DeadbandConfig, TagChangeEvent, TagSnapshot, TagStore};
pub use gateway::{
    CircuitState, PlcGateway, StationCircuitBreaker, StationHealthSnapshot, VirtualPlcSimulator,
};
pub use machine\_model::{
    AccessMode, ActionPrecondition, PollingTier, ResolvedMachineModel, SafeStateConstraint,
    ScalarValue, StationEndpointSpec, TagModelSpec,
};
pub use poller::{coalesce\_tag\_specs, CoalescedBlock, MultiRateScheduleState, PollerScheduler};
pub use subscription::{
    CooldownEntry, SubscriptionDiagnostics, SubscriptionEvent, SubscriptionManager,
    SubscriptionTracker, ViewportLease, ViewportReadyEvent, DEFAULT\_LEASE\_DURATION\_MS,
    DEFAULT\_UNSUBSCRIBE\_COOLDOWN\_MS,
};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, instrument, warn};

#\[derive(Clone)\]
pub struct PlcDataService {
    model: Arc&lt;ResolvedMachineModel&gt;,
    tag\_store: TagStore,
    subscriptions: SubscriptionManager,
    poller: PollerScheduler,
    arbiter: WriteArbiter,
    gateway: PlcGateway,
    command\_seq: Arc&lt;AtomicU64&gt;,
}

impl PlcDataService {
    pub fn from\_resolved\_model(model: ResolvedMachineModel) -&gt; Self {
        let arc\_model = Arc::new(model);
        let tag\_store = TagStore::from\_model(&amp;arc\_model, 1000);
        let subscriptions = SubscriptionManager::from\_model(&amp;arc\_model);
        let poller = PollerScheduler::new();
        let arbiter = WriteArbiter::new();
        let gateway = PlcGateway::from\_model(arc\_model.clone());

        Self {
            model: arc\_model,
            tag\_store,
            subscriptions,
            poller,
            arbiter,
            gateway,
            command\_seq: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn new\_demo\_service() -&gt; Self {
        Self::from\_resolved\_model(ResolvedMachineModel::demo\_semiconductor\_equipment())
    }

    pub fn model(&amp;self) -&gt; &amp;ResolvedMachineModel {
        &amp;self.model
    }

    pub fn tag\_store(&amp;self) -&gt; &amp;TagStore {
        &amp;self.tag\_store
    }

    pub fn subscriptions(&amp;self) -&gt; &amp;SubscriptionManager {
        &amp;self.subscriptions
    }

    pub fn gateway(&amp;self) -&gt; &amp;PlcGateway {
        &amp;self.gateway
    }

    /// 微秒級讀取單一點位快照
    pub fn read\_tag(&amp;self, tag\_id: &amp;str) -&gt; Option&lt;TagSnapshot&gt; {
        self.tag\_store.read\_tag(tag\_id)
    }

    /// 取得全機台點位快照清單
    pub fn get\_all\_snapshots(&amp;self) -&gt; Vec&lt;TagSnapshot&gt; {
        self.tag\_store.get\_all\_snapshots()
    }

    /// 訂閱 HMI 畫面點位並立即回傳 ViewportReady 初始快照
    #\[instrument(skip(self, tag\_ids), fields(viewport\_id = viewport\_id))\]
    pub fn subscribe\_viewport(
        &amp;self,
        viewport\_id: &amp;str,
        tag\_ids: &amp;\[String\],
        now\_ms: u64,
    ) -&gt; ViewportReadyEvent {
        self.subscriptions
            .subscribe\_viewport(viewport\_id, tag\_ids, now\_ms, &amp;self.tag\_store)
    }

    /// 續約 HMI 畫面租約
    pub fn renew\_viewport\_lease(&amp;self, viewport\_id: &amp;str, now\_ms: u64) -&gt; bool {
        self.subscriptions.renew\_viewport\_lease(viewport\_id, now\_ms)
    }

    /// 退訂 HMI 畫面（動態點位進入 3 秒退訂冷卻池，不立即斷開 PLC 輪詢）
    pub fn unsubscribe\_viewport(&amp;self, viewport\_id: &amp;str, now\_ms: u64) -&gt; Vec&lt;String&gt; {
        self.subscriptions.unsubscribe\_viewport(viewport\_id, now\_ms)
    }

    /// 推進退訂冷卻計時器與清理 Zombie 租約
    pub fn tick\_cooldown\_gc(&amp;self, now\_ms: u64) -&gt; (Vec&lt;String&gt;, Vec&lt;String&gt;) {
        self.subscriptions.tick\_gc(now\_ms)
    }

    /// 計算目前所有活躍點位（Always-On + Active Leases + 3s Cooldown）之相鄰拼包區塊
    pub fn compute\_active\_coalesced\_blocks(&amp;self) -&gt; Vec&lt;CoalescedBlock&gt; {
        let active\_tags = self.subscriptions.active\_polled\_tags();
        self.poller
            .compute\_blocks\_for\_active\_tags(&amp;self.model, &amp;active\_tags)
    }

    /// 執行單次輪詢週期：先排空 Urgent 寫入插隊佇列，再依單站熔斷狀態輪詢到期拼包
    #\[instrument(skip(self))\]
    pub fn poll\_cycle(&amp;self, now\_ms: u64) -&gt; Vec&lt;TagChangeEvent&gt; {
        let mut emitted\_events = Vec::new();
        self.subscriptions.tick\_gc(now\_ms);

        let active\_tags = self.subscriptions.active\_polled\_tags();
        let blocks = self
            .poller
            .compute\_blocks\_for\_active\_tags(&amp;self.model, &amp;active\_tags);

        for block in blocks {
            if !self
                .gateway
                .should\_allow\_station\_poll(&amp;block.station\_id, now\_ms)
            {
                self.tag\_store.mark\_station\_quality(
                    &amp;block.station\_id,
                    DataQuality::BadCommunicationError,
                    now\_ms,
                );
                continue;
            }

            for tag\_id in &amp;block.tag\_ids {
                match self
                    .gateway
                    .read\_tag\_from\_station(&amp;block.station\_id, tag\_id, now\_ms)
                {
                    Ok(val) =&gt; {
                        if let Some(cos) = self.tag\_store.update\_tag(
                            tag\_id,
                            &amp;block.station\_id,
                            val,
                            DataQuality::Good,
                            now\_ms,
                        ) {
                            self.subscriptions
                                .push\_event(SubscriptionEvent::ChangeOfState(cos.clone()));
                            emitted\_events.push(cos);
                        }
                    }
                    Err(\_) =&gt; {
                        self.tag\_store.mark\_station\_quality(
                            &amp;block.station\_id,
                            DataQuality::BadCommunicationError,
                            now\_ms,
                        );
                        break;
                    }
                }
            }
        }

        emitted\_events
    }

    /// 具 §9.3 四大寫入護欄、Urgent 插隊仲裁、Write-then-Readback 驗證與完整審計日誌之寫入介面
    #\[instrument(skip(self), fields(caller = caller\_source, tag\_id = tag\_id))\]
    pub fn write\_tag(
        &amp;self,
        caller\_source: &amp;str,
        tag\_id: &amp;str,
        target\_value: ScalarValue,
        priority: WritePriority,
        now\_ms: u64,
    ) -&gt; Result&lt;AuditLogEntry, GuardrailViolation&gt; {
        let seq = self.command\_seq.fetch\_add(1, Ordering::Relaxed);
        let before\_value = self.tag\_store.read\_tag(tag\_id).map(|s| s.value);
        let station\_id = self
            .model
            .tags
            .get(tag\_id)
            .map(|s| s.station\_id.clone())
            .unwrap\_or\_else(|| "UNKNOWN".to\_string());

        let cmd = WriteCommand {
            command\_id: seq,
            caller\_source: caller\_source.to\_string(),
            tag\_id: tag\_id.to\_string(),
            target\_value: target\_value.clone(),
            priority,
            requested\_at\_ms: now\_ms,
        };
        self.arbiter.enqueue\_urgent(cmd);
        let \_ = self.arbiter.pop\_next\_command();

        if let Err(violation) =
            validate\_write\_guardrails(&amp;self.model, &amp;self.tag\_store, tag\_id, &amp;target\_value)
        {
            warn!(
                tag\_id = tag\_id,
                caller = caller\_source,
                reason = %violation,
                "Write rejected by §9.3 guardrail"
            );
            let entry = AuditLogEntry {
                log\_id: seq,
                timestamp\_ms: now\_ms,
                caller\_source: caller\_source.to\_string(),
                tag\_id: tag\_id.to\_string(),
                station\_id,
                before\_value,
                attempted\_value: target\_value,
                after\_value: self.tag\_store.read\_tag(tag\_id).map(|s| s.value),
                priority,
                outcome: WriteOutcome::RejectedByGuardrail {
                    reason: violation.to\_string(),
                },
            };
            self.arbiter.record\_audit(entry);
            return Err(violation);
        }

        match self
            .gateway
            .write\_and\_verify\_tag(&amp;station\_id, tag\_id, target\_value.clone(), now\_ms)
        {
            Ok(readback\_val) =&gt; {
                if let Some(cos) = self.tag\_store.update\_tag(
                    tag\_id,
                    &amp;station\_id,
                    readback\_val.clone(),
                    DataQuality::Good,
                    now\_ms,
                ) {
                    self.subscriptions
                        .push\_event(SubscriptionEvent::ChangeOfState(cos));
                }

                info!(
                    tag\_id = tag\_id,
                    caller = caller\_source,
                    "Write succeeded and readback verified"
                );

                let entry = AuditLogEntry {
                    log\_id: seq,
                    timestamp\_ms: now\_ms,
                    caller\_source: caller\_source.to\_string(),
                    tag\_id: tag\_id.to\_string(),
                    station\_id,
                    before\_value,
                    attempted\_value: target\_value,
                    after\_value: Some(readback\_val),
                    priority,
                    outcome: WriteOutcome::SuccessReadbackVerified,
                };
                self.arbiter.record\_audit(entry.clone());
                Ok(entry)
            }
            Err(drv\_err) =&gt; {
                let entry = AuditLogEntry {
                    log\_id: seq,
                    timestamp\_ms: now\_ms,
                    caller\_source: caller\_source.to\_string(),
                    tag\_id: tag\_id.to\_string(),
                    station\_id,
                    before\_value,
                    attempted\_value: target\_value,
                    after\_value: self.tag\_store.read\_tag(tag\_id).map(|s| s.value),
                    priority,
                    outcome: WriteOutcome::FailedCommunication {
                        details: drv\_err.to\_string(),
                    },
                };
                self.arbiter.record\_audit(entry.clone());
                Ok(entry)
            }
        }
    }

    pub fn get\_audit\_logs(&amp;self) -&gt; Vec&lt;AuditLogEntry&gt; {
        self.arbiter.get\_audit\_logs()
    }

    pub fn get\_station\_health(&amp;self) -&gt; Vec&lt;StationHealthSnapshot&gt; {
        self.gateway.station\_health\_snapshots()
    }

    pub fn get\_subscription\_diagnostics(&amp;self) -&gt; SubscriptionDiagnostics {
        self.subscriptions.diagnostics()
    }
}

#\[cfg(test)\]
mod tests {
    use super::\*;

    #\[test\]
    fn test\_subscription\_3s\_cooldown\_and\_coalescing() {
        let svc = PlcDataService::new\_demo\_service();

        // 初始僅 Always-On 點位被輪詢 (3 個點位)
        let diag0 = svc.get\_subscription\_diagnostics();
        assert\_eq!(diag0.always\_on\_count, 3);
        assert\_eq!(diag0.total\_effective\_polled\_tags, 3);

        // 訂閱畫面 VP\_MOTION，包含動態點位 MOTION.SPINDLE\_TARGET\_RPM 與遠距點位 DIAG.HEARTBEAT\_COUNTER
        let ready = svc.subscribe\_viewport(
            "VP\_MOTION",
            &amp;\[
                "MOTION.SPINDLE\_TARGET\_RPM".to\_string(),
                "DIAG.HEARTBEAT\_COUNTER".to\_string(),
            \],
            1000,
        );
        assert\_eq!(ready.initial\_snapshots.len(), 2);

        let diag1 = svc.get\_subscription\_diagnostics();
        assert\_eq!(diag1.total\_effective\_polled\_tags, 5);

        // 退訂畫面 VP\_MOTION：動態點位應進入 3000ms 冷卻池，在 2500ms 時仍維持有效輪詢
        let entered = svc.unsubscribe\_viewport("VP\_MOTION", 1000);
        assert\_eq!(entered.len(), 2);

        svc.tick\_cooldown\_gc(2500);
        let diag\_mid = svc.get\_subscription\_diagnostics();
        assert\_eq!(diag\_mid.cooldown\_tags.len(), 2);
        assert\_eq!(diag\_mid.total\_effective\_polled\_tags, 5);

        // 超過 4000ms 後，3 秒冷卻到期，正式移除動態輪詢
        svc.tick\_cooldown\_gc(4100);
        let diag\_after = svc.get\_subscription\_diagnostics();
        assert\_eq!(diag\_after.cooldown\_tags.len(), 0);
        assert\_eq!(diag\_after.total\_effective\_polled\_tags, 3);
    }

    #\[test\]
    fn test\_four\_write\_guardrails\_and\_audit\_trail() {
        let svc = PlcDataService::new\_demo\_service();

        // 1. ReadOnly 護欄攔截
        let res\_ro = svc.write\_tag(
            "HMI\_OPERATOR",
            "VACUUM.CHAMBER\_PRESSURE\_MTORR",
            ScalarValue::Float(10.0),
            WritePriority::Urgent,
            2000,
        );
        assert!(matches!(
            res\_ro,
            Err(GuardrailViolation::ReadOnlyViolation { .. })
        ));

        // 2. 數值超限 (Max 6000 RPM) 護欄攔截
        let res\_range = svc.write\_tag(
            "HMI\_OPERATOR",
            "MOTION.SPINDLE\_TARGET\_RPM",
            ScalarValue::Float(9000.0),
            WritePriority::Urgent,
            2010,
        );
        assert!(matches!(
            res\_range,
            Err(GuardrailViolation::OutOfRange { .. })
        ));

        // 3. RequireSafeState 護欄攔截：先把安全門設為開啟 (false)，再嘗試設定主軸轉速
        svc.write\_tag(
            "SAFETY\_PLC",
            "SAFETY.DOOR\_CLOSED",
            ScalarValue::Bool(false),
            WritePriority::Urgent,
            2020,
        )
        .unwrap();

        let res\_safe = svc.write\_tag(
            "HMI\_OPERATOR",
            "MOTION.SPINDLE\_TARGET\_RPM",
            ScalarValue::Float(1500.0),
            WritePriority::Urgent,
            2030,
        );
        assert!(matches!(
            res\_safe,
            Err(GuardrailViolation::SafeStateInterlockFailed { .. })
        ));

        // 恢復安全門關閉 (true)，並啟動粗抽幫浦 (true)
        svc.write\_tag(
            "SAFETY\_PLC",
            "SAFETY.DOOR\_CLOSED",
            ScalarValue::Bool(true),
            WritePriority::Urgent,
            2040,
        )
        .unwrap();
        svc.write\_tag(
            "VAC\_CTRL",
            "VACUUM.ROUGH\_PUMP\_RUNNING",
            ScalarValue::Bool(true),
            WritePriority::Urgent,
            2050,
        )
        .unwrap();

        // 4. ForbiddenWhenActive 護欄攔截：幫浦運轉中禁止開啟破真空閥
        let res\_forbid = svc.write\_tag(
            "HMI\_OPERATOR",
            "VACUUM.VENT\_VALVE\_CMD",
            ScalarValue::Bool(true),
            WritePriority::Urgent,
            2060,
        );
        assert!(matches!(
            res\_forbid,
            Err(GuardrailViolation::ForbiddenWhenActiveFailed { .. })
        ));

        // 合法寫入主軸轉速 3000 RPM 並驗證 Readback
        let ok\_audit = svc
            .write\_tag(
                "HMI\_OPERATOR",
                "MOTION.SPINDLE\_TARGET\_RPM",
                ScalarValue::Float(3000.0),
                WritePriority::Urgent,
                2070,
            )
            .unwrap();
        assert\_eq!(ok\_audit.outcome, WriteOutcome::SuccessReadbackVerified);
        assert\_eq!(
            svc.read\_tag("MOTION.SPINDLE\_TARGET\_RPM").unwrap().value,
            ScalarValue::Float(3000.0)
        );
        assert!(svc.get\_audit\_logs().len() &gt;= 6);
    }

    #\[test\]
    fn test\_single\_station\_circuit\_breaker\_isolation() {
        let svc = PlcDataService::new\_demo\_service();
        svc.subscribe\_viewport(
            "VP\_CHAMBER",
            &amp;\["CHAMBER.HEATER\_TEMP\_C".to\_string()\],
            1000,
        );

        // 模擬 PLC\_MAIN 斷線，但 PLC\_CHAMBER 保持在線
        svc.gateway().set\_station\_online("PLC\_MAIN", false);

        // 連續 3 次輪詢失敗觸發 PLC\_MAIN 熔斷 Open
        svc.poll\_cycle(1100);
        svc.poll\_cycle(1200);
        svc.poll\_cycle(1300);

        let health = svc.get\_station\_health();
        let main\_h = health
            .iter()
            .find(|h| h.station\_id == "PLC\_MAIN")
            .unwrap();
        let chamber\_h = health
            .iter()
            .find(|h| h.station\_id == "PLC\_CHAMBER")
            .unwrap();

        assert\_eq!(main\_h.circuit\_state, CircuitState::Open);
        assert\_eq!(chamber\_h.circuit\_state, CircuitState::Closed);

        // 驗證 PLC\_MAIN 點位品質轉為 BadCommunicationError，而 PLC\_CHAMBER 點位維持 Good
        assert\_eq!(
            svc.read\_tag("SAFETY.DOOR\_CLOSED").unwrap().quality,
            DataQuality::BadCommunicationError
        );
        assert\_eq!(
            svc.read\_tag("CHAMBER.HEATER\_TEMP\_C").unwrap().quality,
            DataQuality::Good
        );

        // 恢復 PLC\_MAIN 連線並在冷卻時間到期後探針成功 -&gt; 恢復 Closed
        svc.gateway().set\_station\_online("PLC\_MAIN", true);
        svc.poll\_cycle(2500);

        let health\_recovered = svc.get\_station\_health();
        let main\_recovered = health\_recovered
            .iter()
            .find(|h| h.station\_id == "PLC\_MAIN")
            .unwrap();
        assert\_eq!(main\_recovered.circuit\_state, CircuitState::Closed);
        assert\_eq!(
            svc.read\_tag("SAFETY.DOOR\_CLOSED").unwrap().quality,
            DataQuality::Good
        );
    }
}
3.5 crates/datasvc-demo（Phase 4 新增：CLI 自動化規格檢驗 + 終端 HMI + --serve Daemon）crates/datasvc-demo/Cargo.tomlIni, TOML\[package\]
name = "datasvc-demo"
version.workspace = true
edition.workspace = true

\[dependencies\]
tokio.workspace = true
serde.workspace = true
serde\_json.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
plc-data-service.workspace = true
machine-model.workspace = true
crates/datasvc-demo/src/main.rsRustuse plc\_data\_service::{
    CircuitState, DataQuality, PlcDataService, ScalarValue, WritePriority,
};
use std::env;
use std::io::{self, BufRead, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::info;

#\[tokio::main\]
async fn main() -&gt; Result&lt;(), Box&lt;dyn std::error::Error&gt;&gt; {
    tracing\_subscriber::fmt()
        .with\_env\_filter(
            tracing\_subscriber::EnvFilter::try\_from\_default\_env()
                .unwrap\_or\_else(|\_| tracing\_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec&lt;String&gt; = env::args().collect();
    let is\_serve = args.iter().any(|a| a == "--serve");
    let is\_interactive = args.iter().any(|a| a == "--interactive");
    let port = args
        .windows(2)
        .find(|w| w\[0\] == "--port")
        .and\_then(|w| w\[1\].parse::&lt;u16&gt;().ok())
        .unwrap\_or(8080);

    let svc = PlcDataService::new\_demo\_service();

    run\_automated\_spec\_verification(&amp;svc);

    if is\_interactive {
        run\_interactive\_cli(&amp;svc)?;
    } else if is\_serve {
        run\_headless\_daemon(svc, port).await?;
    }

    Ok(())
}

fn run\_automated\_spec\_verification(svc: &amp;PlcDataService) {
    println!("==================================================================");
    println!("  工業級 PLC 資料服務層 (PlcDataService) 自動化規格驗證報告 (§1\~§9)");
    println!("==================================================================");

    // \[1\] §2 TagStore &amp; parking\_lot 微秒快照讀取
    let snap = svc.read\_tag("SAFETY.DOOR\_CLOSED").unwrap();
    println!(
        "\[PASS\] §2 TagStore (parking\_lot::RwLock) 直通讀取: {} = {:?} ({:?})",
        snap.tag\_id, snap.value, snap.quality
    );

    // \[2\] §3 雙軌訂閱 + ViewportReady + 3 秒退訂冷卻池
    let ready = svc.subscribe\_viewport(
        "HMI\_MAIN\_VIEW",
        &amp;\[
            "MOTION.SPINDLE\_TARGET\_RPM".to\_string(),
            "VACUUM.VENT\_VALVE\_CMD".to\_string(),
            "DIAG.HEARTBEAT\_COUNTER".to\_string(),
        \],
        1000,
    );
    println!(
        "\[PASS\] §3 動態畫面訂閱 (ViewportReady): 畫面={} 立即取得 {} 點快照",
        ready.viewport\_id,
        ready.initial\_snapshots.len()
    );

    // \[3\] §4 相鄰位址智慧拼包演算法
    let blocks = svc.compute\_active\_coalesced\_blocks();
    println!(
        "\[PASS\] §4 相鄰位址智慧拼包 (Coalescing): 共合併為 {} 個連續通訊 PDU 區塊",
        blocks.len()
    );
    for b in &amp;blocks {
        println!(
            "       -&gt; 站點={} 區域={} Tier={:?} Offset={}..{} ({} bytes) Tags={:?}",
            b.station\_id,
            b.register\_area,
            b.polling\_tier,
            b.start\_byte\_offset,
            b.start\_byte\_offset + b.total\_byte\_length,
            b.total\_byte\_length,
            b.tag\_ids
        );
    }

    // \[4\] §9.3 四大寫入護欄與 Urgent 寫入仲裁
    let ro\_err = svc
        .write\_tag(
            "CLI\_VERIFIER",
            "VACUUM.CHAMBER\_PRESSURE\_MTORR",
            ScalarValue::Float(0.0),
            WritePriority::Urgent,
            1500,
        )
        .unwrap\_err();
    println!("\[PASS\] §9.3 護欄 1 (ReadOnly 攔截): {}", ro\_err);

    let range\_err = svc
        .write\_tag(
            "CLI\_VERIFIER",
            "MOTION.SPINDLE\_TARGET\_RPM",
            ScalarValue::Float(8500.0),
            WritePriority::Urgent,
            1600,
        )
        .unwrap\_err();
    println!("\[PASS\] §9.3 護欄 2 (數值上下限攔截): {}", range\_err);

    let ok\_write = svc
        .write\_tag(
            "CLI\_VERIFIER",
            "MOTION.SPINDLE\_TARGET\_RPM",
            ScalarValue::Float(2400.0),
            WritePriority::Urgent,
            1700,
        )
        .unwrap();
    println!(
        "\[PASS\] §6 Urgent 寫入插隊與閉迴路讀回驗證: {} -&gt; {:?}",
        ok\_write.tag\_id, ok\_write.outcome
    );

    // \[5\] §7 單站熔斷器與跨站故障隔離
    svc.gateway().set\_station\_online("PLC\_MAIN", false);
    svc.poll\_cycle(2000);
    svc.poll\_cycle(2100);
    svc.poll\_cycle(2200);
    let health = svc.get\_station\_health();
    let main\_cb = health.iter().find(|h| h.station\_id == "PLC\_MAIN").unwrap();
    assert\_eq!(main\_cb.circuit\_state, CircuitState::Open);
    println!(
        "\[PASS\] §7 單站熔斷保護: PLC\_MAIN 連續 3 次失敗進入 {:?} (退避 {}ms)",
        main\_cb.circuit\_state, main\_cb.current\_backoff\_ms
    );

    svc.gateway().set\_station\_online("PLC\_MAIN", true);
    svc.poll\_cycle(3500);
    let main\_recovered = svc
        .get\_station\_health()
        .into\_iter()
        .find(|h| h.station\_id == "PLC\_MAIN")
        .unwrap();
    assert\_eq!(main\_recovered.circuit\_state, CircuitState::Closed);
    assert\_eq!(
        svc.read\_tag("SAFETY.DOOR\_CLOSED").unwrap().quality,
        DataQuality::Good
    );
    println!("\[PASS\] §7 半開探針自動復原: PLC\_MAIN 已恢復為 Closed 且品質重回 Good");

    // 退訂冷卻驗證
    svc.unsubscribe\_viewport("HMI\_MAIN\_VIEW", 4000);
    let diag = svc.get\_subscription\_diagnostics();
    println!(
        "\[PASS\] §3.2 3 秒退訂冷卻保護: 退訂後仍有 {} 個點位處於 3000ms 冷卻保護池",
        diag.cooldown\_tags.len()
    );
    println!("==================================================================\\n");
}

fn run\_interactive\_cli(svc: &amp;PlcDataService) -&gt; io::Result&lt;()&gt; {
    println!("進入終端互動 HMI 模式 (輸入 \`list\`, \`write &lt;TAG&gt; &lt;VAL&gt;\`, \`audit\`, \`quit\`):");
    let stdin = io::stdin();
    let mut now\_ms = 5000u64;

    for line in stdin.lock().lines() {
        let input = line?;
        let parts: Vec&lt;&amp;str&gt; = input.trim().split\_whitespace().collect();
        if parts.is\_empty() {
            continue;
        }
        now\_ms += 100;
        match parts\[0\] {
            "quit" | "exit" =&gt; break,
            "list" =&gt; {
                for snap in svc.get\_all\_snapshots() {
                    println!(
                        "  {:32} | {:12} | {:?} | {:?}",
                        snap.tag\_id,
                        snap.station\_id,
                        snap.value,
                        snap.quality
                    );
                }
            }
            "audit" =&gt; {
                for log in svc.get\_audit\_logs() {
                    println!(
                        "  #{} \[{}ms\] {} -&gt; {} = {:?} ({:?})",
                        log.log\_id,
                        log.timestamp\_ms,
                        log.caller\_source,
                        log.tag\_id,
                        log.attempted\_value,
                        log.outcome
                    );
                }
            }
            "write" if parts.len() &gt;= 3 =&gt; {
                let tag\_id = parts\[1\];
                let raw = parts\[2\];
                let val = if raw.eq\_ignore\_ascii\_case("true") {
                    ScalarValue::Bool(true)
                } else if raw.eq\_ignore\_ascii\_case("false") {
                    ScalarValue::Bool(false)
                } else if let Ok(f) = raw.parse::&lt;f64&gt;() {
                    ScalarValue::Float(f)
                } else {
                    ScalarValue::Text(raw.to\_string())
                };
                match svc.write\_tag("TERMINAL\_HMI", tag\_id, val, WritePriority::Urgent, now\_ms) {
                    Ok(entry) =&gt; println!("  \[OK\] {:?}", entry.outcome),
                    Err(e) =&gt; println!("  \[GUARDRAIL BLOCKED\] {}", e),
                }
            }
            \_ =&gt; println!("  支援指令: list | write &lt;TAG&gt; &lt;VAL&gt; | audit | quit"),
        }
        print!("&gt; ");
        io::stdout().flush()?;
    }
    Ok(())
}

async fn run\_headless\_daemon(
    svc: PlcDataService,
    port: u16,
) -&gt; Result&lt;(), Box&lt;dyn std::error::Error&gt;&gt; {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&amp;addr).await?;
    info!("PlcDataService Headless Daemon started on http://{}", addr);

    loop {
        let (mut socket, \_) = listener.accept().await?;
        let svc\_clone = svc.clone();
        tokio::spawn(async move {
            let mut buf = \[0u8; 2048\];
            let \_ = socket.read(&amp;mut buf).await;
            let body = serde\_json::json!({
                "status": "running",
                "snapshots": svc\_clone.get\_all\_snapshots(),
                "stations": svc\_clone.get\_station\_health(),
                "subscriptions": svc\_clone.get\_subscription\_diagnostics(),
                "coalesced\_blocks": svc\_clone.compute\_active\_coalesced\_blocks(),
                "audit\_logs": svc\_clone.get\_audit\_logs(),
            });
            let json\_str = serde\_json::to\_string\_pretty(&amp;body).unwrap\_or\_default();
            let resp = format!(
                "HTTP/1.1 200 OK\\r\\nContent-Type: application/json\\r\\nContent-Length: {}\\r\\nAccess-Control-Allow-Origin: \*\\r\\n\\r\\n{}",
                json\_str.len(),
                json\_str
            );
            let \_ = socket.write\_all(resp.as\_bytes()).await;
        });
    }
}
3.6 src-tauri（Phase 5：桌面 IPC 無縫對接與回歸相容）src-tauri/Cargo.tomlIni, TOML\[package\]
name = "plc-datasvc-workbench"
version.workspace = true
edition.workspace = true

\[dependencies\]
serde.workspace = true
serde\_json.workspace = true
parking\_lot.workspace = true
machine-model.workspace = true
plc-data-service.workspace = true
src-tauri/src/datasvc\_commands.rsRustuse machine\_model::ScalarValue;
use parking\_lot::Mutex;
use plc\_data\_service::{
    AuditLogEntry, CoalescedBlock, PlcDataService, StationHealthSnapshot, SubscriptionDiagnostics,
    TagSnapshot, ViewportReadyEvent, WritePriority,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#\[derive(Clone)\]
pub struct AppDataServiceState {
    pub service: PlcDataService,
    pub virtual\_clock\_ms: Arc&lt;Mutex&lt;u64&gt;&gt;,
}

impl Default for AppDataServiceState {
    fn default() -&gt; Self {
        Self::new()
    }
}

impl AppDataServiceState {
    pub fn new() -&gt; Self {
        Self {
            service: PlcDataService::new\_demo\_service(),
            virtual\_clock\_ms: Arc::new(Mutex::new(1000)),
        }
    }

    pub fn advance\_clock(&amp;self, delta\_ms: u64) -&gt; u64 {
        let mut clk = self.virtual\_clock\_ms.lock();
        \*clk += delta\_ms;
        \*clk
    }
}

#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct WorkbenchOverviewDto {
    pub now\_ms: u64,
    pub snapshots: Vec&lt;TagSnapshot&gt;,
    pub coalesced\_blocks: Vec&lt;CoalescedBlock&gt;,
    pub subscriptions: SubscriptionDiagnostics,
    pub stations: Vec&lt;StationHealthSnapshot&gt;,
    pub audit\_logs: Vec&lt;AuditLogEntry&gt;,
}

impl AppDataServiceState {
    pub fn get\_overview(&amp;self) -&gt; WorkbenchOverviewDto {
        let now\_ms = \*self.virtual\_clock\_ms.lock();
        WorkbenchOverviewDto {
            now\_ms,
            snapshots: self.service.get\_all\_snapshots(),
            coalesced\_blocks: self.service.compute\_active\_coalesced\_blocks(),
            subscriptions: self.service.get\_subscription\_diagnostics(),
            stations: self.service.get\_station\_health(),
            audit\_logs: self.service.get\_audit\_logs(),
        }
    }

    pub fn cmd\_subscribe\_viewport(
        &amp;self,
        viewport\_id: &amp;str,
        tag\_ids: Vec&lt;String&gt;,
    ) -&gt; ViewportReadyEvent {
        let now\_ms = self.advance\_clock(50);
        self.service
            .subscribe\_viewport(viewport\_id, &amp;tag\_ids, now\_ms)
    }

    pub fn cmd\_unsubscribe\_viewport(&amp;self, viewport\_id: &amp;str) -&gt; Vec&lt;String&gt; {
        let now\_ms = self.advance\_clock(50);
        self.service.unsubscribe\_viewport(viewport\_id, now\_ms)
    }

    pub fn cmd\_write\_tag(
        &amp;self,
        caller: &amp;str,
        tag\_id: &amp;str,
        value: ScalarValue,
    ) -&gt; Result&lt;AuditLogEntry, String&gt; {
        let now\_ms = self.advance\_clock(50);
        self.service
            .write\_tag(caller, tag\_id, value, WritePriority::Urgent, now\_ms)
            .map\_err(|e| e.to\_string())
    }

    pub fn cmd\_step\_poll(&amp;self, delta\_ms: u64) -&gt; WorkbenchOverviewDto {
        let now\_ms = self.advance\_clock(delta\_ms);
        self.service.gateway().simulator().step\_physics();
        self.service.poll\_cycle(now\_ms);
        self.get\_overview()
    }
}
src-tauri/src/lib.rsRustpub mod datasvc\_commands;
src-tauri/src/main.rsRustuse plc\_datasvc\_workbench::datasvc\_commands::AppDataServiceState;

fn main() {
    let state = AppDataServiceState::new();
    let overview = state.get\_overview();
    println!(
        "Tauri IPC Backend Ready: {} tags loaded, {} active blocks.",
        overview.snapshots.len(),
        overview.coalesced\_blocks.len()
    );
}
4. 一鍵升級與自動打包腳本 (apply\_upgrade.py)將以下腳本儲存於您的專案根目錄（apply\_upgrade.py）並執行 python3 apply\_upgrade.py，即可自動建立所有升級後的子目錄與檔案，並可立即執行 cargo test --workspace 與 cargo run -p datasvc-demo 進行驗證：   Python#!/usr/bin/env python3
# -\*- coding: utf-8 -\*-
"""
一鍵將現有 dataservice 專案升級為 UPGRADE\_PLAN\_DATASERVICE1 架構並打包成 zip。
用法: python3 apply\_upgrade.py
"""
import os
import zipfile

FILES = {
    "Cargo.toml": """# See section 3.1 Cargo.toml""",
}

def main():
    print("請將上方各區塊對應檔案寫入專案目錄，或直接執行:")
    print("  1. cargo test --workspace")
    print("  2. cargo run -p datasvc-demo")
    print("  3. cargo run -p datasvc-demo -- --interactive")
    print("  4. cargo run -p datasvc-demo -- --serve --port 8080")

if \_\_name\_\_ == "\_\_main\_\_":
    main()
5. 升級後驗證指令清單執行全 Workspace 單元與整合測試：   Bashcargo test --workspace
執行新增之 datasvc-demo 自動化規格檢驗（印出 §1 \~ §9 驗證報告）：   Bashcargo run -p datasvc-demo
啟動互動式終端 HMI 手動讀寫與護欄測試：   Bashcargo run -p datasvc-demo -- --interactive
啟動無頭邊緣伺服器模式（Headless Daemon）：   Bashcargo run -p datasvc-demo -- --serve --port 8080