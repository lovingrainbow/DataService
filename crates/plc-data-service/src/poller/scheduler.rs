// crates/plc-data-service/src/poller/scheduler.rs
//! §5.2 多速率分群佇列：依速率層的間隔決定本輪哪些點位需要輪詢

use std::collections::BTreeMap;

use machine_model::model::ScanRate;
use serde::{Deserialize, Serialize};

/// 速率層 (§5.2 Multi-Rate Scheduling Queues)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RateTier {
    /// 急停、高頻伺服座標
    High,
    /// 一般運轉狀態與氣缸感測器
    Normal,
    /// 溫度、累積產能等非即時資料
    Low,
}

impl RateTier {
    pub const ALL: [RateTier; 3] = [RateTier::High, RateTier::Normal, RateTier::Low];

    /// 點位的掃描速率 (`ScanRate`) 對應到速率層
    pub fn from_scan_rate(rate: ScanRate) -> Self {
        match rate {
            ScanRate::Fast => RateTier::High,
            ScanRate::Normal => RateTier::Normal,
            ScanRate::Slow => RateTier::Low,
        }
    }
}

/// 各速率層的輪詢間隔 (ms)；依通訊方式（TCP / RS-485）選用預設值或自訂
///
/// 單次讀取耗時取決於線路速度：RS-485 半雙工匯流排上每個區塊都要等待應答，
/// 間隔過短會讓同一輪的區塊互相擠壓，因此兩種通訊的預設值不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollRatePolicy {
    /// High 層間隔 (ms)
    pub high_ms: u64,
    /// Normal 層間隔 (ms)
    pub normal_ms: u64,
    /// Low 層間隔 (ms)
    pub low_ms: u64,
}

impl PollRatePolicy {
    /// TCP / Modbus TCP：規格下限 (High 50ms、Normal 250ms、Low 1000ms)
    pub fn tcp_default() -> Self {
        Self {
            high_ms: 50,
            normal_ms: 250,
            low_ms: 1_000,
        }
    }

    /// RS-485：拉長各層間隔，避免一輪讀取尚未完成就進入下一輪 (起始值，請依現場匯流排速率與區塊數調整)
    pub fn rs485_default() -> Self {
        Self {
            high_ms: 100,
            normal_ms: 500,
            low_ms: 2_000,
        }
    }

    /// 檢查間隔為正數，且速率由快到慢 (High ≤ Normal ≤ Low)
    pub fn validate(&self) -> Result<(), String> {
        if self.high_ms == 0 || self.normal_ms == 0 || self.low_ms == 0 {
            return Err("速率層間隔必須大於 0 ms".into());
        }
        if self.high_ms > self.normal_ms || self.normal_ms > self.low_ms {
            return Err(format!(
                "速率層間隔須符合 High ≤ Normal ≤ Low，實際 High={} Normal={} Low={}",
                self.high_ms, self.normal_ms, self.low_ms
            ));
        }
        Ok(())
    }

    pub fn interval_ms(&self, tier: RateTier) -> u64 {
        match tier {
            RateTier::High => self.high_ms,
            RateTier::Normal => self.normal_ms,
            RateTier::Low => self.low_ms,
        }
    }
}

/// 依各速率層的上次執行時間，決定本輪到期的速率層
#[derive(Debug, Clone)]
pub struct MultiRateScheduler {
    rates: PollRatePolicy,
    last_run_ms: BTreeMap<RateTier, u64>,
}

impl MultiRateScheduler {
    pub fn new(rates: PollRatePolicy) -> Self {
        Self {
            rates,
            last_run_ms: BTreeMap::new(),
        }
    }

    /// 回傳 `now_ms` 時已到期的速率層；從未執行過的層一律到期
    pub fn due_tiers(&mut self, now_ms: u64) -> Vec<RateTier> {
        let mut due = Vec::new();
        for tier in RateTier::ALL {
            let is_due = match self.last_run_ms.get(&tier) {
                None => true,
                Some(&last) => now_ms.saturating_sub(last) >= self.rates.interval_ms(tier),
            };
            if is_due {
                self.last_run_ms.insert(tier, now_ms);
                due.push(tier);
            }
        }
        due
    }
}
