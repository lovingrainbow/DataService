// crates/plc-data-service/src/poller/coalescing.rs
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cache::TagRouteMeta;

/// 合併後的單一連續讀取區塊 (Coalesced Read Block, §5.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoalescedReadBlock {
    pub station_id: String,
    pub area_prefix: String,
    pub start_address: String,
    pub start_index: u32,
    pub total_words: u16,
    /// 包含在此連續區塊內的語意點位與其相對 Word 偏移 `(tag_id, word_offset, word_len)`
    pub member_tags: Vec<(String, u16, u16)>,
    /// 因為允許間隙而一併順道讀取的未訂閱 Gap Words 數量
    pub gap_words_bridged: u16,
}

/// 拼包器參數設定 (§5.1)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CoalescingPolicy {
    /// 相鄰暫存器最大容忍間隙 (RS-485 預設 4 Words, TCP 預設 16 Words)
    pub max_gap_words: u16,
    /// 單一封包最大讀取長度 (RS-485 預設 64 Words, TCP 預設 200 Words)
    pub max_block_words: u16,
}

impl CoalescingPolicy {
    pub fn rs485_default() -> Self {
        Self {
            max_gap_words: 4,
            max_block_words: 64,
        }
    }

    pub fn tcp_default() -> Self {
        Self {
            max_gap_words: 16,
            max_block_words: 200,
        }
    }
}

/// §5.1 間隙容忍相鄰拼包演算法 (Gap-Tolerant Coalescing Engine)
/// 將離散的語意 Tag 按 `(station_id, area_prefix)` 分組並依位址遞增排序，
/// 當 $A_{k+1} - (A_k + \text{len}_k) \le \text{MaxGap}$ 且合併後總長度 $\le \text{MaxBlockSize}$ 時合併為單次連續讀取。
pub fn coalesce_register_blocks(
    routes: &[TagRouteMeta],
    policy: CoalescingPolicy,
) -> Vec<CoalescedReadBlock> {
    let mut grouped: BTreeMap<(String, String), Vec<&TagRouteMeta>> = BTreeMap::new();
    for r in routes {
        grouped
            .entry((r.station_id.clone(), r.area_prefix.clone()))
            .or_default()
            .push(r);
    }

    let mut blocks = Vec::new();

    for ((station_id, prefix), mut list) in grouped {
        list.sort_by_key(|r| r.word_index);

        let mut cur_start: Option<u32> = None;
        let mut cur_end: u32 = 0; // exclusive
        let mut cur_members: Vec<(String, u16, u16)> = Vec::new();
        let mut cur_gaps: u16 = 0;

        for route in list {
            let r_start = route.word_index;
            let r_len = u32::from(route.word_len.max(1));
            let r_end = r_start + r_len;

            match cur_start {
                None => {
                    cur_start = Some(r_start);
                    cur_end = r_end;
                    cur_members.push((route.tag_id.clone(), 0, route.word_len.max(1)));
                    cur_gaps = 0;
                }
                Some(start_idx) => {
                    let gap = r_start.saturating_sub(cur_end);
                    let new_end = cur_end.max(r_end);
                    let new_total_len = new_end - start_idx;

                    if gap <= u32::from(policy.max_gap_words)
                        && new_total_len <= u32::from(policy.max_block_words)
                    {
                        cur_gaps = cur_gaps.saturating_add(gap as u16);
                        let offset = (r_start - start_idx) as u16;
                        cur_members.push((route.tag_id.clone(), offset, route.word_len.max(1)));
                        cur_end = new_end;
                    } else {
                        // 封裝前一個區塊並開啟新區塊
                        let total_w = (cur_end - start_idx) as u16;
                        blocks.push(CoalescedReadBlock {
                            station_id: station_id.clone(),
                            area_prefix: prefix.clone(),
                            start_address: format!("{}{}", prefix, start_idx),
                            start_index: start_idx,
                            total_words: total_w,
                            member_tags: std::mem::take(&mut cur_members),
                            gap_words_bridged: cur_gaps,
                        });

                        cur_start = Some(r_start);
                        cur_end = r_end;
                        cur_members.push((route.tag_id.clone(), 0, route.word_len.max(1)));
                        cur_gaps = 0;
                    }
                }
            }
        }

        if let Some(start_idx) = cur_start {
            let total_w = (cur_end - start_idx) as u16;
            blocks.push(CoalescedReadBlock {
                station_id,
                area_prefix: prefix.clone(),
                start_address: format!("{}{}", prefix, start_idx),
                start_index: start_idx,
                total_words: total_w,
                member_tags: cur_members,
                gap_words_bridged: cur_gaps,
            });
        }
    }

    blocks
}
