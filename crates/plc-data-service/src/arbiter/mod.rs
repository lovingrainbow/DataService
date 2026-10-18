// crates/plc-data-service/src/arbiter/mod.rs
// 4. 寫入仲裁、插隊預佔與 §9.3 四大護欄 (arbiter: §3.2 & §6 Write Preemption)

pub mod guardrails;

pub use guardrails::{WriteRejectReason, verify_write_guardrails};

use serde::{Deserialize, Serialize};

use machine_model::model::ScalarValue;

/// §6.1 優先權等級
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    /// 最高優先權：操作員單點寫入或配方批次寫入 (§6.1 Urgent Preemption)
    Urgent = 0,
    /// 首幀就緒突發讀取 (§4.2 Readiness Burst)
    Burst = 1,
    /// 高頻佇列 (50~100ms)
    High = 2,
    /// 一般佇列 (200~500ms)
    Normal = 3,
    /// 低頻佇列 (1000~3000ms)
    Low = 4,
}

/// §3.2 護欄 4：安全審計日誌項目 (Audit Trail)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub timestamp_ms: u64,
    pub operator_id: String,
    pub client_ip: String,
    pub tag_id: String,
    pub old_value: Option<ScalarValue>,
    pub new_value: ScalarValue,
    pub permitted: bool,
    pub note: String,
}

/// 寫入請求封包 (排入 Urgent 插隊佇列)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreemptiveWriteCommand {
    pub tag_id: String,
    pub value: ScalarValue,
    pub operator_id: String,
    pub client_ip: String,
    /// 整批配方寫入時可選配一次性 Batch Read 校驗 (§6.2)
    pub verify_batch: bool,
}

/// 寫入執行結果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    pub tag_id: String,
    pub written_value: ScalarValue,
    pub elapsed_us: u64,
    pub preempted_polling: bool,
    pub audit_version: u64,
}
