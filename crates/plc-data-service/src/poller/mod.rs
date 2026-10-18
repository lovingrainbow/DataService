// crates/plc-data-service/src/poller/mod.rs
// 3. 智慧相鄰拼包與多速率排程引擎 (poller: §5 Gap-Tolerant Coalescing & Multi-Rate)

pub mod coalescing;

pub use coalescing::{CoalescedReadBlock, CoalescingPolicy, coalesce_register_blocks};
