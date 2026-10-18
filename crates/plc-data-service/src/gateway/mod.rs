// crates/plc-data-service/src/gateway/mod.rs
// 5. 連線狀態機與單站熔斷器 (gateway: §7 Station Circuit Breaker & Tight Timeout)

pub mod circuit_breaker;

pub use circuit_breaker::StationCircuitBreaker;

use serde::{Deserialize, Serialize};

/// §7 連線與熔斷狀態機
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StationHealthState {
    Disconnected,
    Connected,
    /// 連續 3 次逾時觸發熔斷隔離，降頻為 5000ms 探測一次 (§7.2)
    TrippedDegraded,
    BackoffReconnecting,
}

impl StationHealthState {
    pub fn is_online(self) -> bool {
        self == StationHealthState::Connected
    }
}
