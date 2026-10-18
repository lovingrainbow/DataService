// crates/plc-data-service/src/poller/mod.rs
// 3. 智慧相鄰拼包與多速率排程引擎 (poller: §5 Gap-Tolerant Coalescing & Multi-Rate)

pub mod coalescing;
pub mod scheduler;

pub use coalescing::{CoalescedReadBlock, CoalescingPolicy, coalesce_register_blocks};
pub use scheduler::{MultiRateScheduler, PollRatePolicy, RateTier};

use crate::cache::TagChangeEvent;

/// 單次輪詢週期的結果摘要
#[derive(Debug, Clone, Default)]
pub struct PollReport {
    /// 本輪開始時先排空的 Urgent 寫入筆數 (寫入優先於讀取)
    pub writes_executed: usize,
    /// 本輪到期的速率層
    pub due_tiers: Vec<RateTier>,
    /// 成功讀取的拼包區塊數
    pub blocks_read: usize,
    /// 因站點熔斷而略過的區塊數
    pub blocks_skipped: usize,
    /// 讀取失敗的區塊數
    pub blocks_failed: usize,
    /// 讀取後產生的變化事件 (COS)
    pub changes: Vec<TagChangeEvent>,
}
