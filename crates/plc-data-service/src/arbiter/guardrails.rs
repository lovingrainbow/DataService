// crates/plc-data-service/src/arbiter/guardrails.rs
use serde::{Deserialize, Serialize};

use crate::cache::TagRouteMeta;
use machine_model::model::ScalarValue;

/// §3.2 & §6.3 寫入拒絕或失敗原因
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WriteRejectReason {
    UnknownTag(String),
    /// 護欄 1：唯讀點位攔截 (`final_writable == false`)
    ReadOnlyGuardrail { tag_id: String, address: String },
    /// 護欄 2：安全監控點位阻擋 (`safety_monitor == true`)
    SafetyMonitorGuardrail { tag_id: String },
    /// 護欄 3：超出上下限範圍 (`min` ~ `max`)
    OutOfBoundsError { tag_id: String, val: f64, min: f64, max: f64 },
    /// §6.3 斷線或熔斷隔離期間禁止寫入 (Fail-Fast, No Offline Queue)
    DeviceOfflineError { station_id: String, state: String },
    /// 底層驅動通訊錯誤
    DriverError(String),
}

impl std::fmt::Display for WriteRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteRejectReason::UnknownTag(t) => write!(f, "未知的語意點位 '{}'", t),
            WriteRejectReason::ReadOnlyGuardrail { tag_id, address } => {
                write!(f, "[護欄1 唯讀攔截] 點位 '{}' ({}) 非可寫點位", tag_id, address)
            }
            WriteRejectReason::SafetyMonitorGuardrail { tag_id } => {
                write!(f, "[護欄2 安全阻擋] 點位 '{}' 為 safety_monitor 安全監控點，禁止寫入", tag_id)
            }
            WriteRejectReason::OutOfBoundsError { tag_id, val, min, max } => {
                write!(f, "[護欄3 超界攔截] 點位 '{}' 寫入值 {} 超出允許範圍 [{} ~ {}]", tag_id, val, min, max)
            }
            WriteRejectReason::DeviceOfflineError { station_id, state } => {
                write!(f, "[§6.3 Fail-Fast 離線阻擋] 目標站點 '{}' 處於 {} 狀態，嚴禁離線寫入排隊", station_id, state)
            }
            WriteRejectReason::DriverError(e) => write!(f, "驅動寫入失敗: {}", e),
        }
    }
}

/// §3.2 檢查四大寫入護欄
pub fn verify_write_guardrails(
    route: &TagRouteMeta,
    value: &ScalarValue,
    station_online: bool,
    station_state_desc: &str,
) -> Result<(), WriteRejectReason> {
    // §6.3 Fail-Fast：設備離線或熔斷時立即拋錯，嚴禁離線排隊
    if !station_online {
        return Err(WriteRejectReason::DeviceOfflineError {
            station_id: route.station_id.clone(),
            state: station_state_desc.to_string(),
        });
    }

    // 護欄 2：safety_monitor 點位強制阻擋
    if route.safety_monitor {
        return Err(WriteRejectReason::SafetyMonitorGuardrail {
            tag_id: route.tag_id.clone(),
        });
    }

    // 護欄 1：唯讀點位攔截
    if !route.final_writable {
        return Err(WriteRejectReason::ReadOnlyGuardrail {
            tag_id: route.tag_id.clone(),
            address: route.register_address.clone(),
        });
    }

    // 護欄 3：數值上下限檢查
    if let ScalarValue::Number(n) = value {
        if let (Some(min), Some(max)) = (route.min_val, route.max_val) {
            if *n < min || *n > max {
                return Err(WriteRejectReason::OutOfBoundsError {
                    tag_id: route.tag_id.clone(),
                    val: *n,
                    min,
                    max,
                });
            }
        }
    }

    Ok(())
}
