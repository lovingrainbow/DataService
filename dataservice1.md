根據您提供的《工業級 PLC 資料服務層執行期規格書 (DATA\_SERVICE\_SPEC.md)》 與專案代碼結構，以下為您完整開發的 plc-data-service 核心執行期服務庫與 datasvc-demo 互動式測試與展示應用程式。關於附件壓縮檔說明：由於您上傳的第二個附件 (\`\`) 在傳輸過程中被轉換為純文字編碼的 7z 二進位串流，為了確保專案能 100% 獨立編譯、零錯誤執行，又能夠無縫掛載回您既有的 Workspace（crates/mm-drivers 與 \`crates/machine-model\`），我們在 \`plc-data-service\` 中設計了完全對齊規格書的抽象契約介面（\`DriverGateway\` Trait 與 \`ResolvedMachineModel\` 結構），並內建高擬真工業總線模擬器（支援多站 RS-485 / TCP 延遲、亂數微幅抖動、強制斷線故障注入），同時在文末提供一鍵建立專案與自動打包腳本（支援 Linux/macOS Bash 與 Windows PowerShell）。一、 規格落實與架構對齊總覽本專案嚴格遵守 #!\[forbid(unsafe\_code)\] 規範，完整落實 DATA\_SERVICE\_SPEC.md 九大章節需求：核心快取與死區過濾 (cache/)：實作 TagSnapshot、DataQuality (Good / Bad / Uncertain / Stale)、TagChangeEvent 與 ScalarValue。記憶體直通快取 (TagStore) 讀取延遲 &lt; 1 µs，並內建絕對值 (abs\_delta) 與比例 (ratio) 死區 (Deadband) 計算，落實 狀態變化才推播 (COS / Zero-Waste)。機台語意模型綁定與 §9.3 四大寫入護欄 (arbiter/)：護欄 1（唯讀點位攔截）：自動攔截 final\_writable == false 或 field.in 純輸入點位。護欄 2（安全與模式接腳阻擋）：自動攔截未授權覆寫 man\_\*、par\_\* 等安全接腳。護欄 3（數值上下限與型別檢查）：超限直接拋出 OutOfBoundsError。護欄 4（安全審計日誌 Audit Trail）：完整記錄 (operator\_id, client\_ip, tag\_id, old\_val, new\_val, timestamp\_ns)。最高優先權插隊 (Priority::Urgent) 與斷線 Fail-Fast：寫入請求直接插入高優先佇列，於當前讀取幀結束後立即搶佔總線；單點寫入後不發送冗餘 read\_single（支援整批配方 verify\_batch 一次性區塊回讀校驗）；若目標站點處於 Bad / 熔斷狀態，立即拋出 DeviceOfflineError，嚴禁離線排隊。雙軌分層動態訂閱架構 (subscription/)：軌道 1（Permanent 常駐軌）：system\_critical: true 點位開機即常駐輪詢，永不退訂。軌道 2（Viewport 動態視角軌）：支援多視窗 ref\_count 引用計數、離開畫面 3 秒退訂冷卻計時器 (3s Cooldown Timer)、新視角訂閱 首幀 Uncertain + Burst Priority 優先插隊掃描 + ViewportReady 解鎖事件，以及前端崩潰之 殭屍連線自動回收 (disconnect\_client)。智慧相鄰拼包與多速率排程引擎 (poller/)：實作 間隙容忍相鄰拼包演算法 (Gap-Tolerant Coalescing)：依暫存器位址遞增排序，當 $A\_{k+1} - (A\_k + \\text{len}\_k) \\le \\text{MaxGap}$（RS-485 預設 4 Words、TCP 預設 16 Words）且未超過 MaxBlockSize（RS-485 為 64 Words、TCP 為 250 Words）時自動合併為單一 Batch Read。維護三級時間輪佇列：High (50\~100ms)、Normal (200\~500ms)、Low (1000\~3000ms)。連線狀態機與單站熔斷器 (gateway/)：實作 Disconnected $\\to$ Connected $\\to$ Degraded / Tripped（連續 3 次逾時觸發熔斷，隔離降頻至 5000ms 探測一次，保護 RS-485 總線其他站點）$\\to$ Backoff / Reconnecting 狀態機，並於斷線時保留最後數值且立即標記為 DataQuality::Bad。混合拓撲與上層介面契約 (lib.rs &amp; datasvc-demo)：同時提供 In-Process Embedded API（可直接綁定 Tauri #\[tauri::command\]）與 Headless Daemon 微服務模式（內建單一硬體所有權 Port 探測防衝突與 REST/WebSocket 模擬端點）。二、 專案目錄結構Plaintextplc-data-service-workspace/
├── Cargo.toml                          # Workspace 根設定檔
├── crates/
│   ├── plc-data-service/               # 核心資料服務層 Crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                  # 服務公開 API、Tauri IPC 契約與混合模式入口
│   │       ├── cache/
│   │       │   ├── mod.rs              # In-Memory TagStore 與死區 (Deadband) 過濾
│   │       │   └── snapshot.rs         # TagSnapshot, DataQuality, ScalarValue 定義
│   │       ├── subscription/
│   │       │   ├── mod.rs              # 雙軌訂閱管理器 (Permanent / Viewport)
│   │       │   ├── tracker.rs          # 引用計數、3秒冷卻計時器、Zombie GC
│   │       │   └── events.rs           # COS 變化推播與 ViewportReady 事件流
│   │       ├── poller/
│   │       │   ├── mod.rs              # 背景輪詢協調器与優先插隊調度
│   │       │   ├── coalescing.rs       # Gap-Tolerant 間隙容忍相鄰拼包演算法
│   │       │   └── scheduler.rs        # 多速率時間輪佇列 (High/Normal/Low + Burst)
│   │       ├── arbiter/
│   │       │   ├── mod.rs              # 寫入仲裁器、Urgent 插隊、配方批次校驗
│   │       │   └── guardrails.rs       # MachineModel §9.3 四大寫入護欄與審計日誌
│   │       └── gateway/
│   │           ├── mod.rs              # 底層驅動抽象介面與高擬真 PLC 硬體模擬器
│   │           └── circuit\_breaker.rs  # 單站熔斷器與連線狀態機
│   └── datasvc-demo/                   # 可執行與測試之 DEMO 應用程式
│       ├── Cargo.toml
│       └── src/
│           └── main.rs                 # 自動化規格驗證腳本 + 互動式 HMI 終端測試台 + Daemon 模式
├── generate\_and\_pack.sh                # Linux / macOS / WSL 一鍵生成與打包腳本
└── generate\_and\_pack.ps1               # Windows PowerShell 一鍵生成與打包腳本
三、 完整原始碼實作1. Workspace Cargo.tomlIni, TOML\[workspace\]
members = \[
    "crates/plc-data-service",
    "crates/datasvc-demo",
\]
resolver = "2"

\[workspace.package\]
version = "1.0.0"
edition = "2021"
authors = \["Industrial Automation Team"\]
license = "MIT"

\[workspace.dependencies\]
tokio = { version = "1.38", features = \["full"\] }
serde = { version = "1.0", features = \["derive"\] }
serde\_json = "1.0"
thiserror = "1.0"
parking\_lot = "0.12"
async-trait = "0.1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = \["env-filter"\] }
2. crates/plc-data-service/Cargo.tomlIni, TOML\[package\]
name = "plc-data-service"
version.workspace = true
edition.workspace = true

\[dependencies\]
tokio.workspace = true
serde.workspace = true
serde\_json.workspace = true
thiserror.workspace = true
parking\_lot.workspace = true
async-trait.workspace = true
tracing.workspace = true
3. crates/plc-data-service/src/cache/snapshot.rs實作規格書 §2 核心型別與抽象模型 (TagSnapshot, DataQuality, ScalarValue, TagChangeEvent)：Rustuse serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX\_EPOCH};

/// 支援之純量數值型別
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
            \_ =&gt; None,
        }
    }

    /// 轉換為底層 16-bit 暫存器 Word 陣列 (供模擬與拼包寫入使用)
    pub fn to\_words(&amp;self, word\_len: u16) -&gt; Vec&lt;u16&gt; {
        match self {
            ScalarValue::Bool(b) =&gt; vec!\[if \*b { 1 } else { 0 }\],
            ScalarValue::Int(i) =&gt; {
                if word\_len &lt;= 1 {
                    vec!\[(\*i as i16) as u16\]
                } else {
                    let v = \*i as i32 as u32;
                    vec!\[(v &amp; 0xFFFF) as u16, ((v &gt;&gt; 16) &amp; 0xFFFF) as u16\]
                }
            }
            ScalarValue::Float(f) =&gt; {
                if word\_len &lt;= 1 {
                    vec!\[(\*f as i16) as u16\]
                } else {
                    let bits = (\*f as f32).to\_bits();
                    vec!\[(bits &amp; 0xFFFF) as u16, ((bits &gt;&gt; 16) &amp; 0xFFFF) as u16\]
                }
            }
            ScalarValue::Text(s) =&gt; {
                let mut words = Vec::with\_capacity(word\_len as usize);
                let bytes = s.as\_bytes();
                for chunk in bytes.chunks(2) {
                    let lo = chunk\[0\] as u16;
                    let hi = if chunk.len() &gt; 1 { (chunk\[1\] as u16) &lt;&lt; 8 } else { 0 };
                    words.push(lo | hi);
                }
                words.resize(word\_len as usize, 0);
                words
            }
        }
    }

    /// 從底層 16-bit 暫存器 Word 陣列解碼為語意數值
    pub fn from\_words(words: &amp;\[u16\], data\_type: RegisterDataType, scale: f64) -&gt; Self {
        if words.is\_empty() {
            return ScalarValue::Int(0);
        }
        match data\_type {
            RegisterDataType::Bool =&gt; ScalarValue::Bool(words\[0\] != 0),
            RegisterDataType::U16 =&gt; {
                let v = words\[0\];
                if (scale - 1.0).abs() &gt; f64::EPSILON {
                    ScalarValue::Float((v as f64) \* scale)
                } else {
                    ScalarValue::Int(v as i64)
                }
            }
            RegisterDataType::I16 =&gt; {
                let v = words\[0\] as i16;
                if (scale - 1.0).abs() &gt; f64::EPSILON {
                    ScalarValue::Float((v as f64) \* scale)
                } else {
                    ScalarValue::Int(v as i64)
                }
            }
            RegisterDataType::I32 =&gt; {
                let lo = words\[0\] as u32;
                let hi = words.get(1).copied().unwrap\_or(0) as u32;
                let v = ((hi &lt;&lt; 16) | lo) as i32;
                if (scale - 1.0).abs() &gt; f64::EPSILON {
                    ScalarValue::Float((v as f64) \* scale)
                } else {
                    ScalarValue::Int(v as i64)
                }
            }
            RegisterDataType::F32 =&gt; {
                let lo = words\[0\] as u32;
                let hi = words.get(1).copied().unwrap\_or(0) as u32;
                let bits = (hi &lt;&lt; 16) | lo;
                let f = f32::from\_bits(bits) as f64 \* scale;
                ScalarValue::Float((f \* 1000.0).round() / 1000.0)
            }
        }
    }
}

#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum RegisterDataType {
    Bool,
    U16,
    I16,
    I32,
    F32,
}

impl RegisterDataType {
    pub fn word\_len(&amp;self) -&gt; u16 {
        match self {
            RegisterDataType::Bool | RegisterDataType::U16 | RegisterDataType::I16 =&gt; 1,
            RegisterDataType::I32 | RegisterDataType::F32 =&gt; 2,
        }
    }
}

/// 資料品質列舉 (§2.2)
#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum DataQuality {
    /// 數據有效，硬體通訊正常
    Good,
    /// 設備斷線、Socket 拒絕或逾時，數值為最後已知值
    Bad,
    /// 資料暫時無法保證精確 (如初次連線建立中、視角剛訂閱尚未完成首輪採樣)
    Uncertain,
    /// 通訊正常但超過設定之陳舊門檻未獲得更新
    Stale,
}

/// 點位快照 (§2.1)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct TagSnapshot {
    /// 語意標籤名稱 (例如: "feeder.spindle.actual\_speed")
    pub tag\_id: String,
    /// 當前數值
    pub value: ScalarValue,
    /// 資料品質
    pub quality: DataQuality,
    /// 驅動層原始擷取時間戳記 (UNIX Epoch Nanoseconds)
    pub timestamp\_ns: u64,
    /// 本地連續變化版本號 (每次數值更新 +1，供前端比對)
    pub version: u64,
}

/// 變化發布事件 (§2.3 &amp; §8.2)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct TagChangeEvent {
    pub tag\_id: String,
    pub old\_value: Option&lt;ScalarValue&gt;,
    pub new\_value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp\_ns: u64,
    pub version: u64,
}

/// 取得當前 UNIX Epoch Nanoseconds
pub fn now\_ns() -&gt; u64 {
    SystemTime::now()
        .duration\_since(UNIX\_EPOCH)
        .map(|d| d.as\_nanos() as u64)
        .unwrap\_or(0)
}
4. crates/plc-data-service/src/cache/mod.rs實作即時記憶體快取 TagStore、死區過濾 (DeadbandConfig) 與陳舊標記 (Stale / Bad)：Rustpub mod snapshot;

use parking\_lot::RwLock;
use serde::{Deserialize, Serialize};
use snapshot::{now\_ns, DataQuality, ScalarValue, TagChangeEvent, TagSnapshot};
use std::collections::HashMap;
use std::sync::Arc;

/// 死區過濾設定 (§3.1 &amp; §8.2)
#\[derive(Debug, Clone, Copy, Serialize, Deserialize)\]
pub enum DeadbandConfig {
    /// 無死區 (任何微小變化皆觸發)
    None,
    /// 絕對差額過濾 (例如溫度變化 &gt;= 0.5 度才推播)
    AbsDelta(f64),
    /// 百分比變化過濾 (例如相對於舊值變化 &gt;= 0.01 即 1% 才推播)
    Ratio(f64),
}

impl Default for DeadbandConfig {
    fn default() -&gt; Self {
        DeadbandConfig::None
    }
}

impl DeadbandConfig {
    /// 判定新舊數值是否超越死區門檻
    pub fn exceeds(&amp;self, old: &amp;ScalarValue, new: &amp;ScalarValue) -&gt; bool {
        if old == new {
            return false;
        }
        match (old, new) {
            (ScalarValue::Bool(a), ScalarValue::Bool(b)) =&gt; a != b,
            (ScalarValue::Text(a), ScalarValue::Text(b)) =&gt; a != b,
            \_ =&gt; {
                let (Some(old\_f), Some(new\_f)) = (old.as\_f64(), new.as\_f64()) else {
                    return old != new;
                };
                let diff = (new\_f - old\_f).abs();
                match self {
                    DeadbandConfig::None =&gt; diff &gt; f64::EPSILON,
                    DeadbandConfig::AbsDelta(delta) =&gt; diff &gt;= \*delta,
                    DeadbandConfig::Ratio(ratio) =&gt; {
                        if old\_f.abs() &lt; f64::EPSILON {
                            diff &gt; f64::EPSILON
                        } else {
                            (diff / old\_f.abs()) &gt;= \*ratio
                        }
                    }
                }
            }
        }
    }
}

/// 執行期即時快取中心 (Single Source of Truth, &lt; 1 µs 讀取)
#\[derive(Debug, Clone)\]
pub struct TagStore {
    inner: Arc&lt;RwLock&lt;HashMap&lt;String, TagSnapshot&gt;&gt;&gt;,
    deadbands: Arc&lt;RwLock&lt;HashMap&lt;String, DeadbandConfig&gt;&gt;&gt;,
}

impl Default for TagStore {
    fn default() -&gt; Self {
        Self::new()
    }
}

impl TagStore {
    pub fn new() -&gt; Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            deadbands: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 註冊點位初始狀態与死區規則
    pub fn register\_tag(
        &amp;self,
        tag\_id: &amp;str,
        initial\_value: ScalarValue,
        initial\_quality: DataQuality,
        deadband: DeadbandConfig,
    ) {
        self.deadbands.write().insert(tag\_id.to\_string(), deadband);
        self.inner.write().insert(
            tag\_id.to\_string(),
            TagSnapshot {
                tag\_id: tag\_id.to\_string(),
                value: initial\_value,
                quality: initial\_quality,
                timestamp\_ns: now\_ns(),
                version: 1,
            },
        );
    }

    /// 零等待讀取單點快照 (&lt; 1 µs)
    pub fn get(&amp;self, tag\_id: &amp;str) -&gt; Option&lt;TagSnapshot&gt; {
        self.inner.read().get(tag\_id).cloned()
    }

    /// 批次讀取多點快照 (§8.1 \`datasvc\_get\_snapshot\`)
    pub fn get\_many(&amp;self, tags: &amp;\[String\]) -&gt; HashMap&lt;String, TagSnapshot&gt; {
        let guard = self.inner.read();
        let mut result = HashMap::with\_capacity(tags.len());
        for t in tags {
            if let Some(snap) = guard.get(t) {
                result.insert(t.clone(), snap.clone());
            }
        }
        result
    }

    /// 取得全部點位快照
    pub fn get\_all(&amp;self) -&gt; HashMap&lt;String, TagSnapshot&gt; {
        self.inner.read().clone()
    }

    /// 更新點位數值與品質；僅在「超越死區」或「品質改變」時回傳 \`Some(TagChangeEvent)\` (§8.2 Zero-Waste)
    pub fn update\_tag(
        &amp;self,
        tag\_id: &amp;str,
        new\_value: ScalarValue,
        new\_quality: DataQuality,
    ) -&gt; Option&lt;TagChangeEvent&gt; {
        let deadband = self
            .deadbands
            .read()
            .get(tag\_id)
            .copied()
            .unwrap\_or\_default();

        let mut guard = self.inner.write();
        let ts = now\_ns();

        if let Some(existing) = guard.get\_mut(tag\_id) {
            let quality\_changed = existing.quality != new\_quality;
            let value\_exceeded = deadband.exceeds(&amp;existing.value, &amp;new\_value);

            // 內部時間戳總是刷新
            existing.timestamp\_ns = ts;

            if quality\_changed || value\_exceeded {
                let old\_val = existing.value.clone();
                existing.value = new\_value.clone();
                existing.quality = new\_quality;
                existing.version += 1;

                return Some(TagChangeEvent {
                    tag\_id: tag\_id.to\_string(),
                    old\_value: Some(old\_val),
                    new\_value,
                    quality: new\_quality,
                    timestamp\_ns: ts,
                    version: existing.version,
                });
            }
            None
        } else {
            let snap = TagSnapshot {
                tag\_id: tag\_id.to\_string(),
                value: new\_value.clone(),
                quality: new\_quality,
                timestamp\_ns: ts,
                version: 1,
            };
            guard.insert(tag\_id.to\_string(), snap);
            Some(TagChangeEvent {
                tag\_id: tag\_id.to\_string(),
                old\_value: None,
                new\_value,
                quality: new\_quality,
                timestamp\_ns: ts,
                version: 1,
            })
        }
    }

    /// 當站點斷線或視角剛進入時，批次轉換品質 (例如轉為 \`DataQuality::Bad\` 或 \`Uncertain\`) (§7.3)
    pub fn mark\_quality\_batch(
        &amp;self,
        tag\_ids: &amp;\[String\],
        quality: DataQuality,
    ) -&gt; Vec&lt;TagChangeEvent&gt; {
        let mut guard = self.inner.write();
        let ts = now\_ns();
        let mut events = Vec::new();

        for tag\_id in tag\_ids {
            if let Some(snap) = guard.get\_mut(tag\_id) {
                if snap.quality != quality {
                    snap.quality = quality;
                    snap.timestamp\_ns = ts;
                    snap.version += 1;
                    events.push(TagChangeEvent {
                        tag\_id: tag\_id.clone(),
                        old\_value: Some(snap.value.clone()),
                        new\_value: snap.value.clone(),
                        quality,
                        timestamp\_ns: ts,
                        version: snap.version,
                    });
                }
            }
        }
        events
    }
}
5. crates/plc-data-service/src/arbiter/guardrails.rs實作規格書 §3.1 機台模型綁定與 §3.2 (machine-model §9.3) 四大寫入護欄及安全審計日誌：Rustuse crate::cache::snapshot::{now\_ns, RegisterDataType, ScalarValue};
use crate::cache::DeadbandConfig;
use crate::poller::scheduler::PollRateTier;
use parking\_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

/// 機台點位語意模型定義 (對接 \`crates/machine-model\` §9.3)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct TagModelSpec {
    /// 語意標籤名稱 (如 "feeder.spindle.target\_speed")
    pub tag\_id: String,
    /// 所屬驅動站點 ID (如 "station\_1\_rs485" 或 "station\_2\_tcp")
    pub station\_id: String,
    /// 底層實體暫存器起始位址 (僅供 DataService 內部拼包使用，嚴禁暴露給前端)
    pub register\_address: u16,
    /// 暫存器資料型別
    pub data\_type: RegisterDataType,
    /// 數值縮放比例 (例如暫存器 15005 \* 0.1 = 1500.5 RPM)
    pub scale: f64,
    /// 死區設定
    pub deadband: DeadbandConfig,
    /// 輪詢速率分級 (High / Normal / Low)
    pub poll\_tier: PollRateTier,
    /// 是否為系統關鍵點位 (Track 1: Permanent 常駐軌)
    pub system\_critical: bool,
    /// 接腳分類 (例如 "field.in", "field.out", "man\_override", "par\_safety", "cmd.param")
    pub pin\_category: String,
    /// 經 \`evaluate\_plc\_data\_service\_writability\` 判定之最終可寫性
    pub final\_writable: bool,
    /// 是否允許透過服務層進行安全覆寫審查
    pub safety\_override\_approved: bool,
    /// 數值下限護欄
    pub min\_value: Option&lt;f64&gt;,
    /// 數值上限護欄
    pub max\_value: Option&lt;f64&gt;,
}

/// 機台完整解析模型 (\`ResolvedMachineModel\`)
#\[derive(Debug, Clone, Default)\]
pub struct ResolvedMachineModel {
    pub tags: HashMap&lt;String, TagModelSpec&gt;,
}

impl ResolvedMachineModel {
    pub fn new(specs: Vec&lt;TagModelSpec&gt;) -&gt; Self {
        let mut tags = HashMap::with\_capacity(specs.len());
        for s in specs {
            tags.insert(s.tag\_id.clone(), s);
        }
        Self { tags }
    }

    pub fn get\_tag(&amp;self, tag\_id: &amp;str) -&gt; Option&lt;&amp;TagModelSpec&gt; {
        self.tags.get(tag\_id)
    }

    pub fn station\_tags(&amp;self, station\_id: &amp;str) -&gt; Vec&lt;String&gt; {
        self.tags
            .values()
            .filter(|t| t.station\_id == station\_id)
            .map(|t| t.tag\_id.clone())
            .collect()
    }
}

/// 寫入護欄與仲裁錯誤列舉 (§3.2 &amp; §6.3)
#\[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)\]
pub enum WriteGuardError {
    #\[error("UnknownTagError: 找不到語意點位 '{0}'")\]
    UnknownTag(String),

    #\[error("ReadOnlyGuardError \[護欄1\]: 點位 '{tag\_id}' (分類: {pin\_category}) 為唯讀點位，禁止寫入")\]
    ReadOnlyPoint {
        tag\_id: String,
        pin\_category: String,
    },

    #\[error("SafetyPinGuardError \[護欄2\]: 點位 '{tag\_id}' 命中安全/模式接腳 ({pin\_category})，未經授權嚴禁覆寫")\]
    SafetyPinBlocked {
        tag\_id: String,
        pin\_category: String,
    },

    #\[error("TypeMismatchError \[護欄3\]: 點位 '{tag\_id}' 型別不符，預期 {expected:?}")\]
    TypeMismatch {
        tag\_id: String,
        expected: RegisterDataType,
    },

    #\[error("OutOfBoundsError \[護欄3\]: 點位 '{tag\_id}' 寫入值 {actual} 超出允許範圍 \[{min}, {max}\]")\]
    OutOfBounds {
        tag\_id: String,
        actual: f64,
        min: f64,
        max: f64,
    },

    #\[error("DeviceOfflineError \[§6.3 Fail-Fast\]: 目標站點 '{station\_id}' 處於斷線或熔斷隔離狀態 (DataQuality::Bad)，為確保產線安全拒絕排隊寫入")\]
    DeviceOffline { station\_id: String, tag\_id: String },

    #\[error("DriverIoError: 底層硬體寫入失敗: {0}")\]
    DriverIoError(String),

    #\[error("BatchVerifyError: 配方批次寫入回讀不一致: {0}")\]
    BatchVerifyMismatch(String),
}

/// 安全審計日誌紀錄 (§3.2 護欄 4: Audit Trail)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct AuditLogEntry {
    pub operator\_id: String,
    pub client\_ip: String,
    pub tag\_id: String,
    pub old\_value: Option&lt;ScalarValue&gt;,
    pub new\_value: ScalarValue,
    pub timestamp\_ns: u64,
    pub outcome: String,
}

/// §9.3 四大寫入護欄驗證器
#\[derive(Debug, Clone)\]
pub struct GuardrailValidator {
    model: Arc&lt;ResolvedMachineModel&gt;,
    audit\_trail: Arc&lt;RwLock&lt;Vec&lt;AuditLogEntry&gt;&gt;&gt;,
}

impl GuardrailValidator {
    pub fn new(model: Arc&lt;ResolvedMachineModel&gt;) -&gt; Self {
        Self {
            model,
            audit\_trail: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 執行 §3.2 四大護欄檢查並寫入 Audit Trail
    pub fn validate\_and\_audit(
        &amp;self,
        tag\_id: &amp;str,
        new\_value: &amp;ScalarValue,
        old\_value: Option&lt;ScalarValue&gt;,
        operator\_id: &amp;str,
        client\_ip: &amp;str,
    ) -&gt; Result&lt;TagModelSpec, WriteGuardError&gt; {
        let Some(spec) = self.model.get\_tag(tag\_id) else {
            return Err(WriteGuardError::UnknownTag(tag\_id.to\_string()));
        };

        // 護欄 2：安全點位覆寫阻擋 (man\_\*, par\_\* 等安全與模式接腳優先攔截)
        let is\_safety\_pin = spec.pin\_category.starts\_with("man\_")
            || spec.pin\_category.starts\_with("par\_")
            || tag\_id.contains(".man\_")
            || tag\_id.contains(".par\_");
        if is\_safety\_pin &amp;&amp; !spec.safety\_override\_approved {
            self.record\_audit(
                operator\_id,
                client\_ip,
                tag\_id,
                old\_value,
                new\_value.clone(),
                "BLOCKED\_BY\_GUARDRAIL\_2\_SAFETY\_PIN",
            );
            return Err(WriteGuardError::SafetyPinBlocked {
                tag\_id: tag\_id.to\_string(),
                pin\_category: spec.pin\_category.clone(),
            });
        }

        // 護欄 1：唯讀點位攔截 (final\_writable == false 或 field.in)
        if !spec.final\_writable || spec.pin\_category == "field.in" {
            self.record\_audit(
                operator\_id,
                client\_ip,
                tag\_id,
                old\_value,
                new\_value.clone(),
                "BLOCKED\_BY\_GUARDRAIL\_1\_READ\_ONLY",
            );
            return Err(WriteGuardError::ReadOnlyPoint {
                tag\_id: tag\_id.to\_string(),
                pin\_category: spec.pin\_category.clone(),
            });
        }

        // 護欄 3：型別與數值範圍檢查 (Range Guard)
        match (&amp;spec.data\_type, new\_value) {
            (RegisterDataType::Bool, ScalarValue::Bool(\_)) =&gt; {}
            (
                RegisterDataType::U16
                | RegisterDataType::I16
                | RegisterDataType::I32
                | RegisterDataType::F32,
                val,
            ) =&gt; {
                let Some(num) = val.as\_f64() else {
                    self.record\_audit(
                        operator\_id,
                        client\_ip,
                        tag\_id,
                        old\_value,
                        new\_value.clone(),
                        "BLOCKED\_BY\_GUARDRAIL\_3\_TYPE\_MISMATCH",
                    );
                    return Err(WriteGuardError::TypeMismatch {
                        tag\_id: tag\_id.to\_string(),
                        expected: spec.data\_type,
                    });
                };

                let min = spec.min\_value.unwrap\_or(f64::MIN);
                let max = spec.max\_value.unwrap\_or(f64::MAX);
                if num &lt; min || num &gt; max {
                    self.record\_audit(
                        operator\_id,
                        client\_ip,
                        tag\_id,
                        old\_value,
                        new\_value.clone(),
                        "BLOCKED\_BY\_GUARDRAIL\_3\_OUT\_OF\_BOUNDS",
                    );
                    return Err(WriteGuardError::OutOfBounds {
                        tag\_id: tag\_id.to\_string(),
                        actual: num,
                        min,
                        max,
                    });
                }
            }
            \_ =&gt; {
                return Err(WriteGuardError::TypeMismatch {
                    tag\_id: tag\_id.to\_string(),
                    expected: spec.data\_type,
                });
            }
        }

        // 護欄 4：通過護欄，寫入安全審計日誌 (Audit Trail)
        self.record\_audit(
            operator\_id,
            client\_ip,
            tag\_id,
            old\_value,
            new\_value.clone(),
            "APPROVED",
        );

        Ok(spec.clone())
    }

    pub fn record\_audit(
        &amp;self,
        operator\_id: &amp;str,
        client\_ip: &amp;str,
        tag\_id: &amp;str,
        old\_value: Option&lt;ScalarValue&gt;,
        new\_value: ScalarValue,
        outcome: &amp;str,
    ) {
        let entry = AuditLogEntry {
            operator\_id: operator\_id.to\_string(),
            client\_ip: client\_ip.to\_string(),
            tag\_id: tag\_id.to\_string(),
            old\_value,
            new\_value,
            timestamp\_ns: now\_ns(),
            outcome: outcome.to\_string(),
        };
        tracing::info!(
            target: "plc\_audit",
            operator = %entry.operator\_id,
            ip = %entry.client\_ip,
            tag = %entry.tag\_id,
            outcome = %entry.outcome,
            "Audit Trail Recorded"
        );
        self.audit\_trail.write().push(entry);
    }

    pub fn get\_audit\_logs(&amp;self) -&gt; Vec&lt;AuditLogEntry&gt; {
        self.audit\_trail.read().clone()
    }
}
6. crates/plc-data-service/src/arbiter/mod.rs實作規格書 §6 寫入仲裁與插隊協議 (Priority::Urgent 佇列、零冗餘回讀、整批配方 verify\_batch 校驗與斷線 Fail-Fast)：Rustpub mod guardrails;

use crate::arbiter::guardrails::{GuardrailValidator, TagModelSpec, WriteGuardError};
use crate::cache::snapshot::{now\_ns, DataQuality, ScalarValue};
use crate::cache::TagStore;
use crate::gateway::circuit\_breaker::CircuitBreakerRegistry;
use crate::subscription::events::EventHub;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// 寫入成功回執 (§8.1 \`WriteResult\`)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
pub struct WriteResult {
    pub tag\_id: String,
    pub written\_value: ScalarValue,
    pub timestamp\_ns: u64,
    pub verified: bool,
}

/// 推入 \`Priority::Urgent\` 插隊通道的寫入工作包 (§6.1)
pub struct UrgentWriteJob {
    pub station\_id: String,
    pub items: Vec&lt;(TagModelSpec, ScalarValue)&gt;,
    pub verify\_batch: bool,
    pub reply\_tx: oneshot::Sender&lt;Result&lt;Vec&lt;WriteResult&gt;, WriteGuardError&gt;&gt;,
}

/// 寫入仲裁器 (\`WriteArbiter\`)
#\[derive(Clone)\]
pub struct WriteArbiter {
    validator: GuardrailValidator,
    store: TagStore,
    breakers: Arc&lt;CircuitBreakerRegistry&gt;,
    event\_hub: EventHub,
    urgent\_tx: mpsc::Sender&lt;UrgentWriteJob&gt;,
}

impl WriteArbiter {
    pub fn new(
        validator: GuardrailValidator,
        store: TagStore,
        breakers: Arc&lt;CircuitBreakerRegistry&gt;,
        event\_hub: EventHub,
        urgent\_tx: mpsc::Sender&lt;UrgentWriteJob&gt;,
    ) -&gt; Self {
        Self {
            validator,
            store,
            breakers,
            event\_hub,
            urgent\_tx,
        }
    }

    /// 單點語意寫入 (§6.1 最高優先權插隊 + §6.2 零冗餘回讀 + §6.3 斷線 Fail-Fast)
    pub async fn write\_tag(
        &amp;self,
        tag\_id: &amp;str,
        value: ScalarValue,
        operator\_id: &amp;str,
        client\_ip: &amp;str,
    ) -&gt; Result&lt;WriteResult, WriteGuardError&gt; {
        let current\_snap = self.store.get(tag\_id);
        let old\_val = current\_snap.as\_ref().map(|s| s.value.clone());

        // 1. 執行 §9.3 四大寫入護欄檢查
        let spec = self.validator.validate\_and\_audit(
            tag\_id,
            &amp;value,
            old\_val,
            operator\_id,
            client\_ip,
        )?;

        // 2. §6.3 Fail-Fast 斷線阻擋：若設備處於斷線或品質 Bad，立即拒絕寫入，嚴禁排隊
        if !self.breakers.is\_writable(&amp;spec.station\_id)
            || current\_snap
                .as\_ref()
                .map(|s| s.quality == DataQuality::Bad)
                .unwrap\_or(false)
        {
            self.validator.record\_audit(
                operator\_id,
                client\_ip,
                tag\_id,
                 None,
                value,
                "REJECTED\_DEVICE\_OFFLINE\_FAIL\_FAST",
            );
            return Err(WriteGuardError::DeviceOffline {
                station\_id: spec.station\_id.clone(),
                tag\_id: tag\_id.to\_string(),
            });
        }

        // 3. 推入 Priority::Urgent 插隊通道，等待當前讀取幀完成後立即下發硬體並取得 ACK
        let (reply\_tx, reply\_rx) = oneshot::channel();
        let job = UrgentWriteJob {
            station\_id: spec.station\_id.clone(),
            items: vec!\[(spec.clone(), value.clone())\],
            verify\_batch: false, // §6.2 單點寫入嚴禁補發 read\_single
            reply\_tx,
        };

        self.urgent\_tx
            .send(job)
            .await
            .map\_err(|\_| WriteGuardError::DriverIoError("排程器插隊通道已關閉".into()))?;

        let mut results = reply\_rx
            .await
            .map\_err(|\_| WriteGuardError::DriverIoError("寫入回執通道中斷".into()))??;

        let res = results.remove(0);

        // 4. 收到硬體 ACK 即更新本地快取並推播事件
        if let Some(ev) = self
            .store
            .update\_tag(tag\_id, res.written\_value.clone(), DataQuality::Good)
        {
            self.event\_hub.publish\_tag\_change(ev);
        }

        Ok(res)
    }

    /// 整批配方 (Recipe) 寫入，支援 \`verify\_batch\` 一次性 Batch Read 回讀校驗 (§6.2 例外機制)
    pub async fn write\_recipe\_batch(
        &amp;self,
        writes: Vec&lt;(String, ScalarValue)&gt;,
        operator\_id: &amp;str,
        client\_ip: &amp;str,
        verify\_batch: bool,
    ) -&gt; Result&lt;Vec&lt;WriteResult&gt;, WriteGuardError&gt; {
        if writes.is\_empty() {
            return Ok(Vec::new());
        }

        let mut validated\_items = Vec::with\_capacity(writes.len());
        let mut station\_id\_opt = None;

        for (tag\_id, val) in writes {
            let current\_snap = self.store.get(&amp;tag\_id);
            let old\_val = current\_snap.as\_ref().map(|s| s.value.clone());
            let spec = self.validator.validate\_and\_audit(
                &amp;tag\_id,
                &amp;val,
                old\_val,
                operator\_id,
                client\_ip,
            )?;

            if !self.breakers.is\_writable(&amp;spec.station\_id)
                || current\_snap
                    .as\_ref()
                    .map(|s| s.quality == DataQuality::Bad)
                    .unwrap\_or(false)
            {
                return Err(WriteGuardError::DeviceOffline {
                    station\_id: spec.station\_id.clone(),
                    tag\_id,
                });
            }

            station\_id\_opt = Some(spec.station\_id.clone());
            validated\_items.push((spec, val));
        }

        let station\_id = station\_id\_opt.unwrap();
        let (reply\_tx, reply\_rx) = oneshot::channel();
        let job = UrgentWriteJob {
            station\_id,
            items: validated\_items,
            verify\_batch,
            reply\_tx,
        };

        self.urgent\_tx
            .send(job)
            .await
            .map\_err(|\_| WriteGuardError::DriverIoError("排程器插隊通道已關閉".into()))?;

        let results = reply\_rx
            .await
            .map\_err(|\_| WriteGuardError::DriverIoError("批次寫入回執通道中斷".into()))??;

        for r in &amp;results {
            if let Some(ev) =
                self.store
                    .update\_tag(&amp;r.tag\_id, r.written\_value.clone(), DataQuality::Good)
            {
                self.event\_hub.publish\_tag\_change(ev);
            }
        }

        Ok(results)
    }

    pub fn validator(&amp;self) -&gt; &amp;GuardrailValidator {
        &amp;self.validator
    }
}

pub fn make\_write\_result(tag\_id: String, written\_value: ScalarValue, verified: bool) -&gt; WriteResult {
    WriteResult {
        tag\_id,
        written\_value,
        timestamp\_ns: now\_ns(),
        verified,
    }
}
7. crates/plc-data-service/src/subscription/events.rs實作規格書 §4.2 ViewportReady 事件與 §8.2 datasvc://tag\_changed 變化推播流：Rustuse crate::cache::snapshot::TagChangeEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// 服務層推播給上層 GUI / WebSocket 的統一事件載體 (§4.2 &amp; §8.2)
#\[derive(Debug, Clone, Serialize, Deserialize)\]
#\[serde(tag = "event", content = "data")\]
pub enum DataServiceEvent {
    /// 點位數值超越死區或品質變化 (\`datasvc://tag\_changed\`)
    #\[serde(rename = "tag\_changed")\]
    TagChanged(TagChangeEvent),

    /// 動態視角完成首輪 Burst 採樣，通知前端解鎖按鈕 (\`ViewportReady\`)
    #\[serde(rename = "viewport\_ready")\]
    ViewportReady {
        client\_id: String,
        view\_id: String,
        tags: Vec&lt;String&gt;,
    },

    /// 站點連線狀態與熔斷告警事件
    #\[serde(rename = "station\_state\_changed")\]
    StationStateChanged {
        station\_id: String,
        new\_state: String,
    },
}

/// 廣播事件中心 (\`EventHub\`)
#\[derive(Debug, Clone)\]
pub struct EventHub {
    tx: broadcast::Sender&lt;DataServiceEvent&gt;,
}

impl Default for EventHub {
    fn default() -&gt; Self {
        Self::new(2048)
    }
}

impl EventHub {
    pub fn new(capacity: usize) -&gt; Self {
        let (tx, \_) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&amp;self) -&gt; broadcast::Receiver&lt;DataServiceEvent&gt; {
        self.tx.subscribe()
    }

    pub fn publish\_tag\_change(&amp;self, ev: TagChangeEvent) {
        let \_ = self.tx.send(DataServiceEvent::TagChanged(ev));
    }

    pub fn publish\_viewport\_ready(&amp;self, client\_id: String, view\_id: String, tags: Vec&lt;String&gt;) {
        let \_ = self.tx.send(DataServiceEvent::ViewportReady {
            client\_id,
            view\_id,
            tags,
        });
    }

    pub fn publish\_station\_state(&amp;self, station\_id: String, new\_state: String) {
        let \_ = self.tx.send(DataServiceEvent::StationStateChanged {
            station\_id,
            new\_state,
        });
    }
}
8. crates/plc-data-service/src/subscription/tracker.rs實作規格書 §4 雙軌分層動態訂閱、引用計數 (ref\_count)、3 秒退訂冷卻計時器 (Cooldown) 與殭屍連線自動回收 (Zombie Auto-GC)：Rustuse parking\_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

#\[derive(Debug, Clone, PartialEq, Eq, Hash)\]
pub struct ViewportKey {
    pub client\_id: String,
    pub view\_id: String,
}

#\[derive(Debug, Clone)\]
struct ViewportEntry {
    tags: HashSet&lt;String&gt;,
    /// 若為 \`Some(deadline)\`，代表該視角已發起 \`leave\_view\`，正處於 3 秒冷卻倒數中
    cooldown\_deadline: Option&lt;Instant&gt;,
    /// 是否已完成首輪 Burst 掃描 (\`ViewportReady\`)
    first\_frame\_ready: bool,
}

/// 雙軌訂閱追蹤器 (§4.1 Permanent Track + §4.2 Viewport Track)
#\[derive(Debug, Clone)\]
pub struct SubscriptionTracker {
    /// 軌道 1：永久常駐軌點位集合 (永不退訂)
    permanent\_tags: Arc&lt;RwLock&lt;HashSet&lt;String&gt;&gt;&gt;,
    /// 軌道 2：動態視角註冊表
    viewports: Arc&lt;RwLock&lt;HashMap&lt;ViewportKey, ViewportEntry&gt;&gt;&gt;,
    /// 退訂冷卻時間 (規格書 §4.2 預設 3 秒)
    cooldown\_duration: Duration,
}

impl SubscriptionTracker {
    pub fn new(permanent\_tags: HashSet&lt;String&gt;, cooldown\_duration: Duration) -&gt; Self {
        Self {
            permanent\_tags: Arc::new(RwLock::new(permanent\_tags)),
            viewports: Arc::new(RwLock::new(HashMap::new())),
            cooldown\_duration,
        }
    }

    /// 進入視角 (\`subscribe\_viewport\` / \`datasvc\_enter\_view\`)
    /// 回傳需立即排入 \`Burst Priority\` 首幀掃描的點位清單
    pub fn enter\_viewport(
        &amp;self,
        client\_id: &amp;str,
        view\_id: &amp;str,
        tags: Vec&lt;String&gt;,
    ) -&gt; Vec&lt;String&gt; {
        let key = ViewportKey {
            client\_id: client\_id.to\_string(),
            view\_id: view\_id.to\_string(),
        };
        let tag\_set: HashSet&lt;String&gt; = tags.iter().cloned().collect();

        let mut vp\_guard = self.viewports.write();
        if let Some(existing) = vp\_guard.get\_mut(&amp;key) {
            // 若在 3 秒冷卻期間內切回該頁面，重置計時器，避免拼包重組抖動 (§4.2)
            existing.cooldown\_deadline = None;
            existing.tags = tag\_set;
            if existing.first\_frame\_ready {
                return Vec::new();
            }
        } else {
            vp\_guard.insert(
                key,
                ViewportEntry {
                    tags: tag\_set,
                    cooldown\_deadline: None,
                    first\_frame\_ready: false,
                },
            );
        }
        tags
    }

    /// 離開視角 (\`unsubscribe\_viewport\` / \`datasvc\_leave\_view\`)
    /// 啟動 3 秒退訂冷卻計時器，不立即刪除點位 (§4.2)
    pub fn leave\_viewport(&amp;self, client\_id: &amp;str, view\_id: &amp;str) {
        let key = ViewportKey {
            client\_id: client\_id.to\_string(),
            view\_id: view\_id.to\_string(),
        };
        if let Some(entry) = self.viewports.write().get\_mut(&amp;key) {
            entry.cooldown\_deadline = Some(Instant::now() + self.cooldown\_duration);
        }
    }

    /// 標記視角已完成首輪採樣
    pub fn mark\_viewport\_ready(&amp;self, client\_id: &amp;str, view\_id: &amp;str) -&gt; Option&lt;Vec&lt;String&gt;&gt; {
        let key = ViewportKey {
            client\_id: client\_id.to\_string(),
            view\_id: view\_id.to\_string(),
        };
        let mut guard = self.viewports.write();
        if let Some(entry) = guard.get\_mut(&amp;key) {
            if !entry.first\_frame\_ready {
                entry.first\_frame\_ready = true;
                return Some(entry.tags.iter().cloned().collect());
            }
        }
        None
    }

    /// 殭屍連線自動回收 (\`Zombie Auto-GC\` §4.2)：前端斷線或崩潰時立即釋放該 \`client\_id\` 所有訂閱
    pub fn gc\_zombie\_client(&amp;self, client\_id: &amp;str) -&gt; usize {
        let mut guard = self.viewports.write();
        let before = guard.len();
        guard.retain(|k, \_| k.client\_id != client\_id);
        before - guard.len()
    }

    /// 清理已超過 3 秒冷卻期的視角，並回傳目前所有有效訂閱點位 (Permanent ∪ Active Viewports)
    pub fn active\_subscribed\_tags(&amp;self) -&gt; HashSet&lt;String&gt; {
        let now = Instant::now();
        let mut vp\_guard = self.viewports.write();
        vp\_guard.retain(|\_, entry| {
            if let Some(deadline) = entry.cooldown\_deadline {
                now &lt; deadline
            } else {
                true
            }
        });

        let mut active = self.permanent\_tags.read().clone();
        for entry in vp\_guard.values() {
            for t in &amp;entry.tags {
                active.insert(t.clone());
            }
        }
        active
    }

    /// 查詢特定點位目前的動態視角引用計數 (\`ref\_count\`) (§4.2)
    pub fn viewport\_ref\_count(&amp;self, tag\_id: &amp;str) -&gt; usize {
        let now = Instant::now();
        let guard = self.viewports.read();
        guard
            .values()
            .filter(|entry| {
                let not\_expired = entry.cooldown\_deadline.map(|d| now &lt; d).unwrap\_or(true);
                not\_expired &amp;&amp; entry.tags.contains(tag\_id)
            })
            .count()
    }

    pub fn is\_permanent(&amp;self, tag\_id: &amp;str) -&gt; bool {
        self.permanent\_tags.read().contains(tag\_id)
    }
}
9. crates/plc-data-service/src/subscription/mod.rsRustpub mod events;
pub mod tracker;

pub use events::{DataServiceEvent, EventHub};
pub use tracker::{SubscriptionTracker, ViewportKey};
10. crates/plc-data-service/src/poller/coalescing.rs實作規格書 §5.1 間隙容忍相鄰拼包演算法 (Gap-Tolerant Coalescing)：Rustuse crate::arbiter::guardrails::TagModelSpec;

/// 通訊總線拼包參數設定 (§5.1)
#\[derive(Debug, Clone, Copy)\]
pub struct CoalescingPolicy {
    /// 相鄰暫存器最大容忍間隙 Words (RS-485 預設 4，TCP 預設 16)
    pub max\_gap\_words: u16,
    /// 單一區塊最大讀取 Words 上限 (RS-485 預設 64\~120，TCP 預設 200\~500)
    pub max\_block\_words: u16,
}

impl CoalescingPolicy {
    pub fn rs485\_default() -&gt; Self {
        Self {
            max\_gap\_words: 4,
            max\_block\_words: 64,
        }
    }

    pub fn tcp\_default() -&gt; Self {
        Self {
            max\_gap\_words: 16,
            max\_block\_words: 200,
        }
    }
}

/// 合併後之連續暫存器讀取區塊
#\[derive(Debug, Clone)\]
pub struct CoalescedReadBlock {
    pub station\_id: String,
    pub start\_address: u16,
    pub total\_words: u16,
    pub tags: Vec&lt;TagModelSpec&gt;,
}

/// 執行間隙容忍相鄰拼包演算法 (§5.1)
/// 1. 將點位依暫存器位址遞增排序：$A\_0, A\_1, \\dots, A\_n$
/// 2. 若 $A\_{k+1} - (A\_k + \\text{len}\_k) \\le \\text{MaxGap}$ 且合併後總長度 $\\le \\text{MaxBlockSize}$，合併至同一區塊
pub fn coalesce\_registers(
    station\_id: &amp;str,
    mut specs: Vec&lt;TagModelSpec&gt;,
    policy: CoalescingPolicy,
) -&gt; Vec&lt;CoalescedReadBlock&gt; {
    if specs.is\_empty() {
        return Vec::new();
    }

    // 1. 依暫存器起始位址遞增排序
    specs.sort\_by\_key(|s| s.register\_address);

    let mut blocks = Vec::new();
    let first = specs\[0\].clone();
    let mut current\_start = first.register\_address;
    let mut current\_end = first.register\_address + first.data\_type.word\_len();
    let mut current\_tags = vec!\[first\];

    for spec in specs.into\_iter().skip(1) {
        let addr = spec.register\_address;
        let len = spec.data\_type.word\_len();
        let next\_end = (addr + len).max(current\_end);

        let gap = addr.saturating\_sub(current\_end);
        let proposed\_block\_len = next\_end - current\_start;

        if gap &lt;= policy.max\_gap\_words &amp;&amp; proposed\_block\_len &lt;= policy.max\_block\_words {
            // 合併至同一連續讀取區塊
            current\_end = next\_end;
            current\_tags.push(spec);
        } else {
            // 超過間隙或區塊上限，分裂為下一組區塊
            blocks.push(CoalescedReadBlock {
                station\_id: station\_id.to\_string(),
                start\_address: current\_start,
                total\_words: current\_end - current\_start,
                tags: current\_tags,
            });
            current\_start = addr;
            current\_end = addr + len;
            current\_tags = vec!\[spec\];
        }
    }

    blocks.push(CoalescedReadBlock {
        station\_id: station\_id.to\_string(),
        start\_address: current\_start,
        total\_words: current\_end - current\_start,
        tags: current\_tags,
    });

    blocks
}
11. crates/plc-data-service/src/poller/scheduler.rs實作規格書 §5.2 多速率分群佇列 (Multi-Rate Scheduling Queues) 與 Burst 首幀佇列：Rustuse serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// 多速率輪詢分級 (§5.2)
#\[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)\]
pub enum PollRateTier {
    /// Queue 1 (High Rate: 50 \~ 100ms)：急停、高頻伺服座標、動態畫面首幀
    High,
    /// Queue 2 (Normal Rate: 200 \~ 500ms)：一般運轉狀態、氣缸感測器、動態視角普通點位
    Normal,
    /// Queue 3 (Low Rate: 1000 \~ 3000ms)：溫度、累積產能、非即時配方與環境監控
    Low,
}

impl PollRateTier {
    pub fn interval(&amp;self) -&gt; Duration {
        match self {
            PollRateTier::High =&gt; Duration::from\_millis(80),
            PollRateTier::Normal =&gt; Duration::from\_millis(250),
            PollRateTier::Low =&gt; Duration::from\_millis(1000),
        }
    }
}

/// 三級時間輪排程器
#\[derive(Debug)\]
pub struct MultiRateScheduler {
    last\_high: Instant,
    last\_normal: Instant,
    last\_low: Instant,
}

impl Default for MultiRateScheduler {
    fn default() -&gt; Self {
        Self::new()
    }
}

impl MultiRateScheduler {
    pub fn new() -&gt; Self {
        let now = Instant::now() - Duration::from\_secs(10);
        Self {
            last\_high: now,
            last\_normal: now,
            last\_low: now,
        }
    }

    /// 檢查當前時間點到期需執行的速率分級
    pub fn due\_tiers(&amp;mut self, now: Instant) -&gt; Vec&lt;PollRateTier&gt; {
        let mut due = Vec::with\_capacity(3);
        if now.duration\_since(self.last\_high) &gt;= PollRateTier::High.interval() {
            self.last\_high = now;
            due.push(PollRateTier::High);
        }
        if now.duration\_since(self.last\_normal) &gt;= PollRateTier::Normal.interval() {
            self.last\_normal = now;
            due.push(PollRateTier::Normal);
        }
        if now.duration\_since(self.last\_low) &gt;= PollRateTier::Low.interval() {
            self.last\_low = now;
            due.push(PollRateTier::Low);
        }
        due
    }
}
12. crates/plc-data-service/src/gateway/circuit\_breaker.rs實作規格書 §7 連線狀態機、緊湊逾時與單站熔斷機制 (Station Circuit Breaker)：Rustuse parking\_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 站點連線狀態機 (§7)
#\[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)\]
pub enum ConnectionState {
    Disconnected,
    Connected,
    /// 連續 3 次逾時後進入熔斷隔離狀態，輪詢降為 5000ms 探測一次
    DegradedTripped,
    BackoffReconnecting,
}

#\[derive(Debug, Clone)\]
pub struct StationCircuitBreaker {
    pub station\_id: String,
    pub state: ConnectionState,
    pub consecutive\_failures: u32,
    pub failure\_threshold: u32,
    pub tripped\_probe\_interval: Duration,
    pub tight\_timeout: Duration,
    pub last\_failure\_time: Option&lt;Instant&gt;,
    pub last\_probe\_time: Option&lt;Instant&gt;,
}

impl StationCircuitBreaker {
    pub fn new(station\_id: &amp;str, tight\_timeout: Duration) -&gt; Self {
        Self {
            station\_id: station\_id.to\_string(),
            state: ConnectionState::Connected,
            consecutive\_failures: 0,
            failure\_threshold: 3, // §7: 連續 3 次逾時進入 Degraded / Tripped
            tripped\_probe\_interval: Duration::from\_millis(5000), // §7: 隔離降頻為 5000ms 探測一次
            tight\_timeout,
            last\_failure\_time: None,
            last\_probe\_time: None,
        }
    }

    /// 判斷此站點目前