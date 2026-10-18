// crates/plc-data-service/src/gateway/circuit_breaker.rs
use serde::{Deserialize, Serialize};

use super::StationHealthState;

/// 單一 PLC 站點熔斷器 (§7.2 Station Circuit Breaker)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationCircuitBreaker {
    pub station_id: String,
    pub state: StationHealthState,
    pub consecutive_failures: u32,
    pub trip_threshold: u32,
    /// RS-485 緊湊逾時 (預設理論耗時 + 25ms = 50ms, §7.1)
    pub tight_timeout_ms: u64,
    /// 熔斷隔離後的降頻探測週期 (預設 5000ms, §7.2)
    pub degraded_probe_interval_ms: u64,
    pub last_failure_ms: u64,
    pub total_trips: u64,
}

impl StationCircuitBreaker {
    pub fn new(station_id: impl Into<String>, tight_timeout_ms: u64) -> Self {
        Self {
            station_id: station_id.into(),
            state: StationHealthState::Connected,
            consecutive_failures: 0,
            trip_threshold: 3,
            tight_timeout_ms: tight_timeout_ms.max(25),
            degraded_probe_interval_ms: 5_000,
            last_failure_ms: 0,
            total_trips: 0,
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.state = StationHealthState::Connected;
    }

    /// 記錄通訊逾時或失敗；若連續達 3 次則觸發熔斷 (`TrippedDegraded`)
    pub fn record_failure(&mut self, now_ms: u64) -> bool {
        self.consecutive_failures += 1;
        self.last_failure_ms = now_ms;
        if self.consecutive_failures >= self.trip_threshold
            && self.state != StationHealthState::TrippedDegraded
        {
            self.state = StationHealthState::TrippedDegraded;
            self.total_trips += 1;
            return true; // 剛剛觸發熔斷
        }
        false
    }

    /// 檢查當前時間點是否允許對該站發起輪詢（熔斷期間每 5000ms 僅放行一次探測）
    pub fn should_allow_poll(&mut self, now_ms: u64) -> bool {
        match self.state {
            StationHealthState::Connected => true,
            StationHealthState::Disconnected => false,
            StationHealthState::TrippedDegraded | StationHealthState::BackoffReconnecting => {
                if now_ms.saturating_sub(self.last_failure_ms) >= self.degraded_probe_interval_ms {
                    self.state = StationHealthState::BackoffReconnecting;
                    true
                } else {
                    false
                }
            }
        }
    }
}
