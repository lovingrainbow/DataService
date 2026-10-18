// crates/plc-data-service/src/subscription/session.rs
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 視角訂閱鍵 `(client_id, view_id)`
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ViewportKey {
    pub client_id: String,
    pub view_id: String,
}

/// 動態視角狀態
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportSession {
    pub key: ViewportKey,
    pub tags: BTreeSet<String>,
    /// 是否已完成首幀 Burst 採樣就緒 (§4.2 Readiness Guard)
    pub first_frame_ready: bool,
    /// 若已離開視角，記錄退訂冷卻到期時間 (3s Cooldown Timer)
    pub cooldown_until_ms: Option<u64>,
    /// 最後心跳時間 (供 Zombie Auto-GC 檢查)
    pub last_heartbeat_ms: u64,
}

/// 雙軌訂閱快照統計 (供 GUI 監控面板檢視)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionStats {
    pub permanent_tags: Vec<String>,
    pub active_viewport_tags: BTreeMap<String, usize>, // tag_id -> ref_count
    pub cooling_down_views: Vec<(String, String, u64)>, // (client_id, view_id, remain_ms)
    pub pending_burst_tags: Vec<String>,
}
