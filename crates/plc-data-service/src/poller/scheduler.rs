// crates/plc-data-service/src/poller/scheduler.rs
//! §5.2 多速率分群佇列：依速率層的間隔決定本輪哪些點位需要輪詢

use std::collections::BTreeMap;

use machine_model::model::ScanRate;

/// 速率層 (§5.2 Multi-Rate Scheduling Queues)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RateTier {
    /// 急停、高頻伺服座標 (規格 50~100ms，採用 50ms)
    High,
    /// 一般運轉狀態與氣缸感測器 (規格 200~500ms，採用 250ms)
    Normal,
    /// 溫度、累積產能等非即時資料 (規格 1000~3000ms，採用 1000ms)
    Low,
}

impl RateTier {
    pub const ALL: [RateTier; 3] = [RateTier::High, RateTier::Normal, RateTier::Low];

    pub fn interval_ms(self) -> u64 {
        match self {
            RateTier::High => 50,
            RateTier::Normal => 250,
            RateTier::Low => 1_000,
        }
    }

    /// 點位的掃描速率 (`ScanRate`) 對應到速率層
    pub fn from_scan_rate(rate: ScanRate) -> Self {
        match rate {
            ScanRate::Fast => RateTier::High,
            ScanRate::Normal => RateTier::Normal,
            ScanRate::Slow => RateTier::Low,
        }
    }
}

/// 依各速率層的上次執行時間，決定本輪到期的速率層
#[derive(Debug, Clone, Default)]
pub struct MultiRateScheduler {
    last_run_ms: BTreeMap<RateTier, u64>,
}

impl MultiRateScheduler {
    /// 回傳 `now_ms` 時已到期的速率層；從未執行過的層一律到期
    pub fn due_tiers(&mut self, now_ms: u64) -> Vec<RateTier> {
        let mut due = Vec::new();
        for tier in RateTier::ALL {
            let is_due = match self.last_run_ms.get(&tier) {
                None => true,
                Some(&last) => now_ms.saturating_sub(last) >= tier.interval_ms(),
            };
            if is_due {
                self.last_run_ms.insert(tier, now_ms);
                due.push(tier);
            }
        }
        due
    }
}
