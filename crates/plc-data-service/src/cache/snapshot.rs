// crates/plc-data-service/src/cache/snapshot.rs
use machine_model::model::{ScalarValue, ScanRate};
use mm_drivers::ByteOrder;
use serde::{Deserialize, Serialize};

/// §2.2 資料品質列舉 (DataQuality)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataQuality {
    /// 數據有效，硬體通訊正常
    Good,
    /// 設備斷線、Socket 拒絕、逾時或熔斷隔離，數值為最後已知值
    Bad,
    /// 資料暫時無法保證精確 (如視角初次掛載尚未完成首幀採樣)
    Uncertain,
    /// 通訊正常但超過陳舊門檻未更新
    Stale,
}

/// §2.1 點位快照 (TagSnapshot)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagSnapshot {
    pub tag_id: String,
    pub value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp_ns: u64,
    pub version: u64,
}

/// §2.3 變化發布事件 (TagChangeEvent)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagChangeEvent {
    pub tag_id: String,
    pub old_value: Option<ScalarValue>,
    pub new_value: ScalarValue,
    pub quality: DataQuality,
    pub timestamp_ns: u64,
    pub version: u64,
}

/// §3.1 死區過濾設定 (Deadband)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DeadbandConfig {
    None,
    /// 絕對差額門檻 (|new - old| > abs_delta)
    Absolute(f64),
    /// 百分比門檻 (|new - old| / |old| > ratio)
    Ratio(f64),
}

impl DeadbandConfig {
    pub fn exceeds(&self, old: &ScalarValue, new: &ScalarValue) -> bool {
        match (old, new) {
            (ScalarValue::Bool(a), ScalarValue::Bool(b)) => a != b,
            (ScalarValue::Text(a), ScalarValue::Text(b)) => a != b,
            (ScalarValue::Number(a), ScalarValue::Number(b)) => {
                let diff = (b - a).abs();
                match self {
                    DeadbandConfig::None => diff > f64::EPSILON,
                    DeadbandConfig::Absolute(delta) => diff > *delta,
                    DeadbandConfig::Ratio(ratio) => {
                        let denom = a.abs().max(1e-9);
                        (diff / denom) > *ratio
                    }
                }
            }
            _ => true,
        }
    }
}

/// 語意點位路由與靜態元資料 (§3.1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRouteMeta {
    pub tag_id: String,
    pub station_id: String,
    pub register_address: String,
    pub area_prefix: String,
    pub word_index: u32,
    pub bit_offset: Option<u8>,
    pub word_len: u16,
    pub data_type: String,
    pub byte_order: ByteOrder,
    pub scan_rate: ScanRate,
    pub deadband: DeadbandConfig,
    pub system_critical: bool,
    pub final_writable: bool,
    pub safety_monitor: bool,
    pub min_val: Option<f64>,
    pub max_val: Option<f64>,
}
