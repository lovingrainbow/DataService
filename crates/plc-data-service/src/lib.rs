// crates/plc-data-service/src/lib.rs
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use machine_model::datasvc::{TagOverrideEntry, evaluate_plc_data_service_writability};
use machine_model::model::{ScanRate, ScalarValue};
use machine_model::resolver::ResolvedMachineModel;
use mm_drivers::{ByteOrder, PlcDriver, RandomReadItem, SingleReadResult, TagQuality, parse_universal_plc_address};
use tokio::sync::broadcast;
use tracing::{info, warn};

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn now_ms() -> u64 {
    now_ns() / 1_000_000
}

pub mod arbiter;
pub mod cache;
pub mod gateway;
pub mod poller;
pub mod subscription;

// ============================================================================
// 6. PlcDataService 頂層執行期引擎 (整合快取、雙軌訂閱、拼包排程、插隊寫入與熔斷)
// ============================================================================

use arbiter::{
    AuditLogEntry, BatchVerification, BatchWriteReport, PreemptiveWriteCommand, QueuedWrite, VerifyMismatch,
    WriteRejectReason, WriteResult, verify_write_guardrails,
};
use cache::{DataQuality, DeadbandConfig, TagChangeEvent, TagRouteMeta, TagStore};
use gateway::{StationCircuitBreaker, StationHealthState};
use poller::{CoalescedReadBlock, CoalescingPolicy, MultiRateScheduler, PollReport, RateTier, coalesce_register_blocks};
use subscription::SubscriptionManager;

/// 一次輪詢讀到的點位值與品質
type PollValue = (String, ScalarValue, DataQuality);

/// 拼包區塊成員的路由資訊（讀取時使用）
struct MemberRoute {
    tag_id: String,
    /// 成員在區塊內的相對偏移（字或位元）
    offset: u16,
    address: String,
    data_type: String,
    word_len: u16,
}

/// 單一拼包區塊的讀取結果
enum BlockOutcome {
    Read(Vec<TagChangeEvent>),
    Skipped,
    Failed,
}

/// 連續讀取的逐字（或逐位元）值依成員偏移切回各點位；`int16` 以有號數解讀
fn decode_coalesced(members: &[MemberRoute], values: &[ScalarValue]) -> Result<Vec<(String, ScalarValue)>, String> {
    members
        .iter()
        .map(|m| -> Result<(String, ScalarValue), String> {
            let raw = values
                .get(usize::from(m.offset))
                .ok_or_else(|| format!("點位 {} 缺少回傳值", m.tag_id))?;
            let value = match (raw, m.data_type.as_str()) {
                (ScalarValue::Number(n), "int16") => ScalarValue::Number(f64::from(*n as u16 as i16)),
                (other, _) => other.clone(),
            };
            Ok((m.tag_id.clone(), value))
        })
        .collect()
}

/// 隨機讀取的結果依序對應到成員；項目數不符視為讀取失敗
fn decode_random(members: &[MemberRoute], items: &[SingleReadResult]) -> Result<Vec<PollValue>, String> {
    if members.len() != items.len() {
        return Err(format!("回讀項目數 {} 與請求 {} 不符", items.len(), members.len()));
    }
    Ok(members
        .iter()
        .zip(items)
        .map(|(m, item)| (m.tag_id.clone(), item.value.clone(), quality_from_driver(item.quality)))
        .collect())
}

fn quality_from_driver(quality: TagQuality) -> DataQuality {
    match quality {
        TagQuality::Good => DataQuality::Good,
        TagQuality::Uncertain => DataQuality::Uncertain,
        TagQuality::Bad => DataQuality::Bad,
    }
}

pub struct PlcDataService {
    pub store: TagStore,
    pub subscriptions: SubscriptionManager,
    pub coalescing_policy: CoalescingPolicy,
    pub drivers: BTreeMap<String, Box<dyn PlcDriver>>,
    pub breakers: BTreeMap<String, StationCircuitBreaker>,
    /// §6.1 Urgent 寫入佇列 (FIFO)，由 `drain_urgent_writes` 或 `poll_cycle` 排空
    pub urgent_write_queue: VecDeque<QueuedWrite>,
    pub audit_trail: VecDeque<AuditLogEntry>,
    completed_writes: BTreeMap<u64, Result<WriteResult, WriteRejectReason>>,
    next_ticket: u64,
    scheduler: MultiRateScheduler,
}

impl PlcDataService {
    /// 由 `ResolvedMachineModel` 與 `service.toml` 覆寫表建構 `PlcDataService` (§3.1 & §3.2)
    pub fn from_resolved_model(
        resolved: &ResolvedMachineModel,
        overrides: &[TagOverrideEntry],
        policy: CoalescingPolicy,
    ) -> (Self, broadcast::Receiver<TagChangeEvent>) {
        let write_report = evaluate_plc_data_service_writability(resolved, overrides);
        let writable_map: BTreeMap<&str, bool> = write_report
            .items
            .iter()
            .map(|i| (i.tag_id.as_str(), i.final_writable))
            .collect();

        let (mut store, rx) = TagStore::new(2048);
        let mut permanent_tags = Vec::new();
        let mut breakers = BTreeMap::new();

        for eff in &resolved.effective_tags {
            let station_id = eff
                .id
                .split('.')
                .next()
                .unwrap_or("plc1")
                .to_string();

            breakers
                .entry(station_id.clone())
                .or_insert_with(|| StationCircuitBreaker::new(&station_id, 50));

            let (prefix, idx, bit_opt, _) =
                parse_universal_plc_address(&eff.address).unwrap_or(("D".into(), 0, None, false));

            let word_len = if eff.data_type.contains("32") || eff.data_type == "float32" {
                2
            } else if eff.data_type.contains("64") || eff.data_type == "float64" {
                4
            } else {
                1
            };

            let deadband = match eff.deadband {
                Some(d) if d > 0.0 => DeadbandConfig::Absolute(d),
                _ => DeadbandConfig::None,
            };

            // 急停、安全監控與警報點位自動歸入「軌道 1：永久常駐軌」(§4.1)
            let is_alarm = eff.bindings.iter().any(|b| b.category == "error");
            let is_critical = eff.safety_monitor || is_alarm || eff.scan == ScanRate::Fast;
            if is_critical {
                permanent_tags.push(eff.id.clone());
            }

            let final_writable = *writable_map.get(eff.id.as_str()).unwrap_or(&eff.write_candidate);

            let route = TagRouteMeta {
                tag_id: eff.id.clone(),
                station_id,
                register_address: eff.address.clone(),
                area_prefix: prefix,
                word_index: idx,
                bit_offset: bit_opt,
                word_len,
                data_type: eff.data_type.clone(),
                byte_order: ByteOrder::CDAB,
                scan_rate: eff.scan,
                deadband,
                system_critical: is_critical,
                final_writable,
                safety_monitor: eff.safety_monitor,
                min_val: eff.min,
                max_val: eff.max,
            };

            let init_val = eff.default.clone().unwrap_or_else(|| {
                if eff.data_type == "bool" {
                    ScalarValue::Bool(false)
                } else {
                    ScalarValue::Number(0.0)
                }
            });

            let init_quality = if is_critical {
                DataQuality::Good
            } else {
                DataQuality::Uncertain
            };
            store.register_tag(route, init_val, init_quality);
        }

        info!(
            tag_count = resolved.effective_tags.len(),
            permanent_tag_count = permanent_tags.len(),
            station_count = breakers.len(),
            "PlcDataService built from resolved model"
        );
        let sub_mgr = SubscriptionManager::new(permanent_tags);

        (
            Self {
                store,
                subscriptions: sub_mgr,
                coalescing_policy: policy,
                drivers: BTreeMap::new(),
                breakers,
                urgent_write_queue: VecDeque::new(),
                completed_writes: BTreeMap::new(),
                next_ticket: 1,
                scheduler: MultiRateScheduler::default(),
                audit_trail: VecDeque::new(),
            },
            rx,
        )
    }

    /// 附掛站點驅動；驅動的通訊逾時與站點熔斷器的緊湊逾時 (§7.1) 同步
    pub fn attach_driver(&mut self, station_id: impl Into<String>, mut driver: Box<dyn PlcDriver>) {
        let sid = station_id.into();
        let breaker = self
            .breakers
            .entry(sid.clone())
            .or_insert_with(|| StationCircuitBreaker::new(&sid, 50));
        driver.set_timeout_ms(breaker.tight_timeout_ms);
        info!(station_id = %sid, tight_timeout_ms = breaker.tight_timeout_ms, "PLC driver attached");
        self.drivers.insert(sid, driver);
    }

    /// 計算目前活躍訂閱點位經相鄰拼包後的連續讀取區塊清單 (§5.1)
    pub fn compute_active_coalesced_blocks(&self) -> Vec<CoalescedReadBlock> {
        let active_tags = self.subscriptions.active_polled_tags();
        let active_routes: Vec<TagRouteMeta> = active_tags
            .iter()
            .filter_map(|tid| self.store.get_route(tid).cloned())
            .collect();
        coalesce_register_blocks(&active_routes, self.coalescing_policy)
    }

    /// §6.1 將寫入請求排入 Urgent 佇列並回傳票號；實際下發於 `drain_urgent_writes` 時執行
    pub fn submit_write(&mut self, command: PreemptiveWriteCommand) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        self.urgent_write_queue.push_back(QueuedWrite { ticket, command });
        ticket
    }

    /// §6.1 依 FIFO 排空 Urgent 佇列；每筆結果保存於票號下，回傳執行筆數
    pub fn drain_urgent_writes(&mut self) -> usize {
        let mut executed = 0;
        while let Some(QueuedWrite { ticket, command }) = self.urgent_write_queue.pop_front() {
            let result = self.execute_write(command);
            self.completed_writes.insert(ticket, result);
            executed += 1;
        }
        executed
    }

    /// 取回票號對應的寫入結果（取回後即移除）
    pub fn take_write_result(&mut self, ticket: u64) -> Option<Result<WriteResult, WriteRejectReason>> {
        self.completed_writes.remove(&ticket)
    }

    /// §6.1 & §3.2 執行具備四大護欄與插隊預佔之語意寫入 (`datasvc_write_tag`)：入列後立即排空
    pub fn write_tag(
        &mut self,
        tag_id: &str,
        value: ScalarValue,
        operator_id: &str,
        client_ip: &str,
    ) -> Result<WriteResult, WriteRejectReason> {
        let ticket = self.submit_write(PreemptiveWriteCommand {
            tag_id: tag_id.to_string(),
            value,
            operator_id: operator_id.to_string(),
            client_ip: client_ip.to_string(),
            verify_batch: false,
        });
        self.drain_urgent_writes();
        self.take_write_result(ticket)
            .unwrap_or_else(|| Err(WriteRejectReason::DriverError("寫入結果遺失".into())))
    }

    /// §6.1 & §3.2 單筆寫入的實際執行：護欄、驅動下發、快取更新與審計 (由佇列排空時呼叫)
    fn execute_write(&mut self, cmd: PreemptiveWriteCommand) -> Result<WriteResult, WriteRejectReason> {
        let tag_id = cmd.tag_id.as_str();
        let operator_id = cmd.operator_id.as_str();
        let client_ip = cmd.client_ip.as_str();
        let value = cmd.value;
        let t0 = Instant::now();
        let route = self
            .store
            .get_route(tag_id)
            .cloned()
            .ok_or_else(|| WriteRejectReason::UnknownTag(tag_id.to_string()))?;

        let breaker = self.breakers.get(&route.station_id);
        let station_online = breaker.map(|b| b.state.is_online()).unwrap_or(true);
        let state_desc = breaker
            .map(|b| format!("{:?}", b.state))
            .unwrap_or_else(|| "Connected".into());

        let old_val = self
            .store
            .get_snapshots(&[tag_id.to_string()])
            .get(tag_id)
            .map(|s| s.value.clone());

        // 1. 執行 §9.3 四大護欄與 §6.3 Fail-Fast 離線檢查
        if let Err(reject) = verify_write_guardrails(&route, &value, station_online, &state_desc) {
            self.audit_trail.push_front(AuditLogEntry {
                timestamp_ms: now_ms(),
                operator_id: operator_id.to_string(),
                client_ip: client_ip.to_string(),
                tag_id: tag_id.to_string(),
                old_value: old_val,
                new_value: value,
                permitted: false,
                note: reject.to_string(),
            });
            warn!(tag_id, operator = operator_id, client_ip, reason = %reject, "write rejected by guardrail");
            return Err(reject);
        }

        // 2. 下發底層驅動 (Priority::Urgent 插隊執行，§6.2 單點寫入收到 ACK 即更新快取，不發冗餘回讀)
        if let Some(drv) = self.drivers.get_mut(&route.station_id) {
            if let Err(e) = drv.write_single(&route.register_address, &route.data_type, value.clone()) {
                warn!(tag_id, station_id = %route.station_id, error = %e, "driver write failed");
                return Err(WriteRejectReason::DriverError(e.to_string()));
            }
        }

        // 3. 更新 TagStore 快取並觸發 COS 事件
        let ev = self.store.update_tag(tag_id, value.clone(), DataQuality::Good);
        let ver = ev.map(|e| e.version).unwrap_or(1);

        self.audit_trail.push_front(AuditLogEntry {
            timestamp_ms: now_ms(),
            operator_id: operator_id.to_string(),
            client_ip: client_ip.to_string(),
            tag_id: tag_id.to_string(),
            old_value: old_val,
            new_value: value.clone(),
            permitted: true,
            note: "Urgent Preemptive Write ACK OK (Zero Redundant Read)".into(),
        });

        let elapsed_us = t0.elapsed().as_micros().max(1) as u64;
        info!(tag_id, operator = operator_id, client_ip, elapsed_us, audit_version = ver, "urgent write committed");

        Ok(WriteResult {
            tag_id: tag_id.to_string(),
            written_value: value,
            elapsed_us,
            preempted_polling: true,
            audit_version: ver,
        })
    }

    /// 模擬或通知某站點通訊結果（更新熔斷器並自動切換 `DataQuality::Bad` / `Good`）
    pub fn report_station_comm_result(&mut self, station_id: &str, success: bool, now_ms: u64) -> StationHealthState {
        let breaker = self
            .breakers
            .entry(station_id.to_string())
            .or_insert_with(|| StationCircuitBreaker::new(station_id, 50));

        let prev_state = breaker.state;
        if success {
            breaker.record_success();
            self.store.mark_station_quality(station_id, DataQuality::Good);
            if prev_state != StationHealthState::Connected {
                info!(station_id, previous_state = ?prev_state, "station recovered; quality restored to Good");
            }
        } else {
            let tripped = breaker.record_failure(now_ms);
            if tripped {
                warn!(
                    station_id,
                    consecutive_failures = breaker.consecutive_failures,
                    total_trips = breaker.total_trips,
                    "station circuit tripped; quality marked Bad"
                );
            }
            if tripped || breaker.state == StationHealthState::TrippedDegraded {
                self.store.mark_station_quality(station_id, DataQuality::Bad);
            }
        }
        breaker.state
    }

    /// §5.2 只拼包指定速率層的活躍點位
    pub fn compute_blocks_for_tier(&self, tier: RateTier) -> Vec<CoalescedReadBlock> {
        let active_tags = self.subscriptions.active_polled_tags();
        let routes: Vec<TagRouteMeta> = active_tags
            .iter()
            .filter_map(|tid| self.store.get_route(tid).cloned())
            .filter(|route| RateTier::from_scan_rate(route.scan_rate.clone()) == tier)
            .collect();
        coalesce_register_blocks(&routes, self.coalescing_policy)
    }

    /// 單次輪詢週期：先排空 Urgent 寫入（寫入優先於讀取），再依到期的速率層讀取各站區塊 (§5.2 & §6.1)
    pub fn poll_cycle(&mut self, now_ms: u64) -> PollReport {
        let writes_executed = self.drain_urgent_writes();
        let due = self.scheduler.due_tiers(now_ms);
        let mut report = PollReport {
            writes_executed,
            due_tiers: due.clone(),
            ..PollReport::default()
        };
        for tier in due {
            for block in self.compute_blocks_for_tier(tier) {
                match self.poll_block(&block, now_ms) {
                    BlockOutcome::Read(changes) => {
                        report.blocks_read += 1;
                        report.changes.extend(changes);
                    }
                    BlockOutcome::Skipped => report.blocks_skipped += 1,
                    BlockOutcome::Failed => report.blocks_failed += 1,
                }
            }
        }
        report
    }

    /// 讀取單一拼包區塊並更新快取。站點熔斷期間略過；讀取失敗時交由熔斷器計數
    fn poll_block(&mut self, block: &CoalescedReadBlock, now_ms: u64) -> BlockOutcome {
        let station = block.station_id.as_str();
        let allowed = self
            .breakers
            .get_mut(station)
            .map_or(true, |breaker| breaker.should_allow_poll(now_ms));
        if !allowed {
            return BlockOutcome::Skipped;
        }

        let members: Vec<MemberRoute> = block
            .member_tags
            .iter()
            .filter_map(|(tag_id, offset, _)| {
                self.store.get_route(tag_id).map(|route| MemberRoute {
                    tag_id: tag_id.clone(),
                    offset: *offset,
                    address: route.register_address.clone(),
                    data_type: route.data_type.clone(),
                    word_len: route.word_len,
                })
            })
            .collect();
        let all_single_word = members.iter().all(|m| m.word_len == 1);
        let all_bits = members.iter().all(|m| m.data_type == "bool");
        let all_words = members.iter().all(|m| m.data_type != "bool");

        // 單字元點位走連續區塊讀取；混合型別或多字元點位改由驅動逐點解碼
        let read: Result<Vec<PollValue>, String> = {
            let Some(driver) = self.drivers.get_mut(station) else {
                return BlockOutcome::Skipped;
            };
            if all_single_word && (all_bits || all_words) {
                let data_type = if all_bits { "bool" } else { "word" };
                driver
                    .read_continuous(&block.start_address, block.total_words, data_type)
                    .map_err(|e| e.to_string())
                    .and_then(|res| decode_coalesced(&members, &res.values))
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|(tag, value)| (tag, value, DataQuality::Good))
                            .collect()
                    })
            } else {
                let items: Vec<RandomReadItem> = members
                    .iter()
                    .map(|m| RandomReadItem {
                        address: m.address.clone(),
                        data_type: m.data_type.clone(),
                    })
                    .collect();
                driver
                    .read_random(&items)
                    .map_err(|e| e.to_string())
                    .and_then(|res| decode_random(&members, &res.items))
            }
        };

        match read {
            Ok(values) => {
                if let Some(breaker) = self.breakers.get_mut(station) {
                    breaker.record_success();
                }
                let changes = values
                    .into_iter()
                    .filter_map(|(tag, value, quality)| self.store.update_tag(&tag, value, quality))
                    .collect();
                BlockOutcome::Read(changes)
            }
            Err(reason) => {
                warn!(station_id = station, start = %block.start_address, error = %reason, "block read failed");
                self.report_station_comm_result(station, false, now_ms);
                BlockOutcome::Failed
            }
        }
    }

    /// §6.2 批次寫入：全部依序經 Urgent 佇列下發；標記 `verify_batch` 且寫入成功的點位，於全批完成後以單次 Batch Read 校驗
    pub fn write_batch(&mut self, commands: Vec<PreemptiveWriteCommand>) -> BatchWriteReport {
        let mut submitted = Vec::with_capacity(commands.len());
        for command in commands {
            let (tag_id, verify) = (command.tag_id.clone(), command.verify_batch);
            let ticket = self.submit_write(command);
            submitted.push((tag_id, ticket, verify));
        }
        self.drain_urgent_writes();

        let mut results = Vec::with_capacity(submitted.len());
        let mut to_verify = Vec::new();
        for (tag_id, ticket, verify) in submitted {
            let result = self
                .take_write_result(ticket)
                .unwrap_or_else(|| Err(WriteRejectReason::DriverError("寫入結果遺失".into())));
            if let (true, Ok(written)) = (verify, &result) {
                to_verify.push((tag_id.clone(), written.written_value.clone()));
            }
            results.push((tag_id, result));
        }

        let verification = if to_verify.is_empty() {
            None
        } else {
            Some(self.verify_readback(&to_verify))
        };
        BatchWriteReport { results, verification }
    }

    /// §6.2 批次校驗：每站以一次隨機讀取回讀；回讀值與寫入值不符時，快取更新為實際值
    fn verify_readback(&mut self, written: &[(String, ScalarValue)]) -> BatchVerification {
        let mut verification = BatchVerification::default();

        let mut by_station: BTreeMap<String, Vec<(String, String, String, ScalarValue)>> = BTreeMap::new();
        for (tag_id, expected) in written {
            if let Some(route) = self.store.get_route(tag_id) {
                by_station.entry(route.station_id.clone()).or_default().push((
                    tag_id.clone(),
                    route.register_address.clone(),
                    route.data_type.clone(),
                    expected.clone(),
                ));
            }
        }

        for (station, entries) in by_station {
            let items: Vec<RandomReadItem> = entries
                .iter()
                .map(|(_, address, data_type, _)| RandomReadItem {
                    address: address.clone(),
                    data_type: data_type.clone(),
                })
                .collect();
            let read = match self.drivers.get_mut(&station) {
                Some(driver) => driver.read_random(&items).map_err(|e| e.to_string()),
                None => Err(format!("站點 {station} 沒有附掛驅動")),
            };
            match read {
                Ok(res) if res.items.len() == entries.len() => {
                    for ((tag_id, _, _, expected), item) in entries.iter().zip(&res.items) {
                        verification.checked += 1;
                        if item.value != *expected {
                            verification.mismatches.push(VerifyMismatch {
                                tag_id: tag_id.clone(),
                                expected: expected.clone(),
                                actual: item.value.clone(),
                            });
                        }
                        self.store.update_tag(tag_id, item.value.clone(), DataQuality::Good);
                    }
                }
                Ok(res) => {
                    verification.read_errors.push(format!(
                        "站點 {station} 回讀項目數 {} 與請求 {} 不符",
                        res.items.len(),
                        entries.len()
                    ));
                    self.report_station_comm_result(&station, false, now_ms());
                }
                Err(reason) => {
                    verification
                        .read_errors
                        .push(format!("站點 {station} 回讀失敗: {reason}"));
                    self.report_station_comm_result(&station, false, now_ms());
                }
            }
        }
        verification
    }
}

// ============================================================================
// 7. 單元與整合測試 (驗證雙軌訂閱、3s 冷卻、相鄰拼包、四大護欄與熔斷器)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use machine_model::family::PlcFamily;
    use machine_model::model::{MachineModelBuilder, UnitInstanceBuilder};
    use machine_model::resolver::resolve_machine_model;
    use machine_model::unit_type::TypeCatalog;
    use mm_drivers::{
        ContinuousReadResult, DriverConfig, DriverError, DriverTrace, LatencyReport, RandomReadResult, RandomWriteItem,
        SimMode, StressTestConfig, StressTestReport, TagUpdate, WriteAck,
    };
    use std::sync::{Arc, Mutex as StdMutex};

    fn sample_resolved_model() -> ResolvedMachineModel {
        let mf = MachineModelBuilder::new("demo_line")
            .add_plc("plc1", PlcFamily::InovanceEasy)
            .add_simple_tag("plc1.X_Cyl1_Ext_LS", "X010", "bool")
            .add_simple_tag("plc1.Y_Cyl1_Ext", "Y010", "bool")
            .add_simple_tag("plc1.M_Cyl1_ManExt", "M100", "bool")
            .add_simple_tag("plc1.M_Cyl1_ErrExt", "M200", "bool")
            .add_simple_tag("plc1.M_Cyl1_ErrRet", "M201", "bool")
            .add_simple_tag("plc1.D_Cyl1_ExtTime", "D100", "int16")
            .add_simple_tag("plc1.D_Cyl1_ErrTime", "D103", "int16")
            .add_simple_tag("plc1.M_St1_ErrJam", "M210", "bool")
            .add_simple_tag("plc1.M_Mach_ErrEstop", "M220", "bool")
            .add_simple_tag("plc1.D_Mach_ManMode", "D106", "int16")
            .add_unit(
                UnitInstanceBuilder::new("cyl1", "cylinder_double", 1)
                    .bind_target("lim_ext", "plc1.X_Cyl1_Ext_LS")
                    .bind_null("lim_ret")
                    .bind_target("valve_ext", "plc1.Y_Cyl1_Ext")
                    .bind_null("valve_ret")
                    .bind_target("man_ext", "plc1.M_Cyl1_ManExt")
                    .bind_target("par_ext_time", "plc1.D_Cyl1_ExtTime")
                    .bind_target("par_err_time", "plc1.D_Cyl1_ErrTime")
                    .bind_target("error_ext", "plc1.M_Cyl1_ErrExt")
                    .bind_target("error_ret", "plc1.M_Cyl1_ErrRet")
                    .build(),
            )
            .add_unit(
                UnitInstanceBuilder::new("st1", "station_load", 1)
                    .bind_target("cyl", "cyl1")
                    .bind_target("error_jam", "plc1.M_St1_ErrJam")
                    .build(),
            )
            .add_unit(
                UnitInstanceBuilder::new("machine", "machine_ctrl", 1)
                    .bind_target("stn", "st1")
                    .bind_target("man_mode", "plc1.D_Mach_ManMode")
                    .bind_target("error_estop", "plc1.M_Mach_ErrEstop")
                    .build(),
            )
            .build();
        let catalog = TypeCatalog::with_embedded_standards();
        resolve_machine_model(&mf, &catalog).unwrap()
    }

    #[test]
    fn test_gap_tolerant_coalescing_merges_nearby_registers() {
        let resolved = sample_resolved_model();
        let (mut svc, _rx) = PlcDataService::from_resolved_model(
            &resolved,
            &[],
            CoalescingPolicy { max_gap_words: 4, max_block_words: 64 },
        );

        // 訂閱 D100, D103, D106 (間隙各為 2 Words <= MaxGap 4)
        svc.subscriptions.enter_viewport(
            "client_1",
            "view_params",
            vec![
                "plc1.D_Cyl1_ExtTime".into(),
                "plc1.D_Cyl1_ErrTime".into(),
                "plc1.D_Mach_ManMode".into(),
            ],
            1000,
        );

        let blocks = svc.compute_active_coalesced_blocks();
        let d_block = blocks.iter().find(|b| b.area_prefix == "D").expect("應有 D 區合併區塊");
        assert_eq!(d_block.start_address, "D100");
        assert_eq!(d_block.total_words, 7); // D100..=D106 共 7 Words
        assert_eq!(d_block.member_tags.len(), 3);
        assert_eq!(d_block.gap_words_bridged, 4); // D101, D102 與 D104, D105
    }

    #[test]
    fn test_two_tier_subscription_and_3s_cooldown() {
        let resolved = sample_resolved_model();
        let (mut svc, _rx) = PlcDataService::from_resolved_model(
            &resolved,
            &[],
            CoalescingPolicy::rs485_default(),
        );

        // 進入視角 t = 1000ms
        svc.subscriptions.enter_viewport(
            "hmi_1",
            "manual_page",
            vec!["plc1.M_Cyl1_ManExt".into()],
            1000,
        );
        assert!(svc.subscriptions.active_polled_tags().contains("plc1.M_Cyl1_ManExt"));

        // 離開視角 t = 2000ms -> 啟動 3000ms 冷卻 (到期時間 5000ms)
        svc.subscriptions.leave_viewport("hmi_1", "manual_page", 2000);
        svc.subscriptions.tick_maintenance(3500);
        // t = 3500ms 仍在 3 秒冷卻期內，點位必須維持在輪詢集合中
        assert!(svc.subscriptions.active_polled_tags().contains("plc1.M_Cyl1_ManExt"));

        // t = 5100ms 超過 3 秒冷卻期，點位正式退訂釋放頻寬；但永久軌警報點仍存在
        svc.subscriptions.tick_maintenance(5100);
        assert!(!svc.subscriptions.active_polled_tags().contains("plc1.M_Cyl1_ManExt"));
        assert!(svc.subscriptions.active_polled_tags().contains("plc1.M_Mach_ErrEstop"));
    }

    #[test]
    fn test_write_guardrails_and_fail_fast_offline_protection() {
        let resolved = sample_resolved_model();
        let (mut svc, _rx) = PlcDataService::from_resolved_model(
            &resolved,
            &[],
            CoalescingPolicy::rs485_default(),
        );

        // 1. 寫入唯讀輸入 X010 -> 觸發護欄 1 ReadOnlyGuardrail
        let res_ro = svc.write_tag("plc1.X_Cyl1_Ext_LS", ScalarValue::Bool(true), "op1", "127.0.0.1");
        assert!(matches!(res_ro, Err(WriteRejectReason::ReadOnlyGuardrail { .. })));

        // 2. 寫入超出範圍的參數 D100 (允許 0..30000，寫入 45000) -> 觸發護欄 3 OutOfBoundsError
        let res_oob = svc.write_tag("plc1.D_Cyl1_ExtTime", ScalarValue::Number(45000.0), "op1", "127.0.0.1");
        assert!(matches!(res_oob, Err(WriteRejectReason::OutOfBoundsError { .. })));

        // 3. 寫入合法範圍參數 D100 = 2500 -> 成功
        let res_ok = svc.write_tag("plc1.D_Cyl1_ExtTime", ScalarValue::Number(2500.0), "op1", "127.0.0.1");
        assert!(res_ok.is_ok());

        // 4. 模擬連續 3 次通訊逾時觸發單站熔斷 -> 再次寫入立即觸發 §6.3 DeviceOfflineError (Fail-Fast)
        svc.report_station_comm_result("plc1", false, 1000);
        svc.report_station_comm_result("plc1", false, 1050);
        let state = svc.report_station_comm_result("plc1", false, 1100);
        assert_eq!(state, StationHealthState::TrippedDegraded);

        let res_offline = svc.write_tag("plc1.D_Cyl1_ExtTime", ScalarValue::Number(2000.0), "op1", "127.0.0.1");
        assert!(matches!(res_offline, Err(WriteRejectReason::DeviceOfflineError { .. })));
    }

    // ------------------------------------------------------------------
    // GAP 1-3：多速率排程、Urgent 寫入佇列與批次回讀校驗
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct FakeState {
        /// 以位址為鍵的記憶體點位表，例如 "D100"
        words: BTreeMap<String, ScalarValue>,
        fail_reads: bool,
        /// 回讀時回傳偏差值 (+1)，模擬硬體與寫入值不一致
        drift: bool,
        /// 驅動最後被設定的通訊逾時 (ms)
        timeout_ms: u64,
    }

    /// 測試用驅動：記憶體點位表，可注入讀取失敗與回讀偏差
    struct FakeDriver {
        cfg: DriverConfig,
        state: Arc<StdMutex<FakeState>>,
    }

    fn fake_trace() -> DriverTrace {
        DriverTrace {
            elapsed_us: 0,
            tx_hex: String::new(),
            rx_hex: String::new(),
        }
    }

    fn injected_failure() -> DriverError {
        DriverError::Disconnected("測試注入的通訊失敗".into())
    }

    fn stored_value(state: &FakeState, address: &str) -> ScalarValue {
        state.words.get(address).cloned().unwrap_or(ScalarValue::Number(0.0))
    }

    /// 16 位元字的無號表示（有號值以二補數表示）
    fn as_word(value: &ScalarValue) -> f64 {
        match value {
            ScalarValue::Number(n) => f64::from(*n as i64 as i16 as u16),
            ScalarValue::Bool(b) => f64::from(u8::from(*b)),
            ScalarValue::Text(_) => 0.0,
        }
    }

    fn as_bit(value: &ScalarValue) -> bool {
        match value {
            ScalarValue::Bool(b) => *b,
            ScalarValue::Number(n) => *n != 0.0,
            ScalarValue::Text(_) => false,
        }
    }

    fn drifted(value: ScalarValue) -> ScalarValue {
        match value {
            ScalarValue::Number(n) => ScalarValue::Number(n + 1.0),
            ScalarValue::Bool(b) => ScalarValue::Bool(!b),
            other => other,
        }
    }

    impl PlcDriver for FakeDriver {
        fn config(&self) -> &DriverConfig {
            &self.cfg
        }

        fn is_connected(&self) -> bool {
            true
        }

        fn connect(&mut self, cfg: DriverConfig) -> Result<DriverTrace, DriverError> {
            self.cfg = cfg;
            Ok(fake_trace())
        }

        fn disconnect(&mut self) -> Result<(), DriverError> {
            Ok(())
        }

        fn read_single(&mut self, address: &str, data_type: &str) -> Result<SingleReadResult, DriverError> {
            let items = [RandomReadItem {
                address: address.into(),
                data_type: data_type.into(),
            }];
            let res = self.read_random(&items)?;
            res.items.into_iter().next().ok_or_else(injected_failure)
        }

        fn write_single(&mut self, address: &str, _data_type: &str, value: ScalarValue) -> Result<WriteAck, DriverError> {
            self.state.lock().unwrap().words.insert(address.to_string(), value.clone());
            Ok(WriteAck {
                address: address.to_string(),
                written_value: value.clone(),
                readback_value: value,
                readback_matched: true,
                timestamp_ms: 0,
                trace: fake_trace(),
            })
        }

        fn read_continuous(
            &mut self,
            start_address: &str,
            length: u16,
            data_type: &str,
        ) -> Result<ContinuousReadResult, DriverError> {
            let guard = self.state.lock().unwrap();
            if guard.fail_reads {
                return Err(injected_failure());
            }
            let (prefix, start, _, _) = parse_universal_plc_address(start_address)?;
            let values = (0..u32::from(length))
                .map(|i| {
                    let stored = stored_value(&guard, &format!("{prefix}{}", start + i));
                    if data_type == "bool" {
                        ScalarValue::Bool(as_bit(&stored))
                    } else {
                        ScalarValue::Number(as_word(&stored))
                    }
                })
                .collect();
            Ok(ContinuousReadResult {
                start_address: start_address.to_string(),
                length,
                data_type: data_type.to_string(),
                values,
                raw_bytes_hex: String::new(),
                trace: fake_trace(),
            })
        }

        fn write_continuous(
            &mut self,
            _start_address: &str,
            _data_type: &str,
            _values: &[ScalarValue],
        ) -> Result<WriteAck, DriverError> {
            Err(DriverError::ProtocolError("測試驅動不支援連續寫入".into()))
        }

        fn read_random(&mut self, items: &[RandomReadItem]) -> Result<RandomReadResult, DriverError> {
            let guard = self.state.lock().unwrap();
            if guard.fail_reads {
                return Err(injected_failure());
            }
            let out = items
                .iter()
                .map(|item| {
                    let stored = stored_value(&guard, &item.address);
                    SingleReadResult {
                        address: item.address.clone(),
                        data_type: item.data_type.clone(),
                        value: if guard.drift { drifted(stored) } else { stored },
                        quality: TagQuality::Good,
                        trace: fake_trace(),
                    }
                })
                .collect();
            Ok(RandomReadResult {
                items: out,
                total_elapsed_us: 0,
                tx_hex: String::new(),
                rx_hex: String::new(),
            })
        }

        fn write_random(&mut self, _items: &[RandomWriteItem]) -> Result<Vec<WriteAck>, DriverError> {
            Err(DriverError::ProtocolError("測試驅動不支援隨機寫入".into()))
        }

        fn ping_latency(&mut self, _address: &str, _rounds: u32) -> Result<LatencyReport, DriverError> {
            Err(DriverError::ProtocolError("測試驅動不支援延遲量測".into()))
        }

        fn run_stress_test(&mut self, _stress_cfg: &StressTestConfig) -> Result<StressTestReport, DriverError> {
            Err(DriverError::ProtocolError("測試驅動不支援壓力測試".into()))
        }

        fn poll(&mut self, _out: &tokio::sync::mpsc::Sender<TagUpdate>) -> Result<(), DriverError> {
            Ok(())
        }

        fn set_simulation(&mut self, _mode: SimMode) -> Result<(), DriverError> {
            Ok(())
        }

        fn set_timeout_ms(&mut self, timeout_ms: u64) {
            self.state.lock().unwrap().timeout_ms = timeout_ms;
        }
    }

    const EXT_TIME: &str = "plc1.D_Cyl1_ExtTime";

    fn write_cmd(tag_id: &str, value: f64, verify_batch: bool) -> PreemptiveWriteCommand {
        PreemptiveWriteCommand {
            tag_id: tag_id.to_string(),
            value: ScalarValue::Number(value),
            operator_id: "op1".to_string(),
            client_ip: "127.0.0.1".to_string(),
            verify_batch,
        }
    }

    fn fake_service() -> (PlcDataService, Arc<StdMutex<FakeState>>) {
        let resolved = sample_resolved_model();
        let (mut svc, _rx) = PlcDataService::from_resolved_model(&resolved, &[], CoalescingPolicy::rs485_default());
        let state = Arc::new(StdMutex::new(FakeState::default()));
        svc.attach_driver(
            "plc1",
            Box::new(FakeDriver {
                cfg: DriverConfig::default(),
                state: Arc::clone(&state),
            }),
        );
        (svc, state)
    }

    #[test]
    fn test_attach_driver_applies_tight_timeout() {
        let (svc, state) = fake_service();
        assert_eq!(svc.breakers["plc1"].tight_timeout_ms, 50);
        assert_eq!(state.lock().unwrap().timeout_ms, 50);
    }

    #[test]
    fn test_multi_rate_scheduler_intervals() {
        let mut sched = MultiRateScheduler::default();
        assert_eq!(sched.due_tiers(0), vec![RateTier::High, RateTier::Normal, RateTier::Low]);
        assert!(sched.due_tiers(49).is_empty());
        assert_eq!(sched.due_tiers(50), vec![RateTier::High]);
        assert_eq!(sched.due_tiers(250), vec![RateTier::High, RateTier::Normal]);
        assert_eq!(sched.due_tiers(1000), vec![RateTier::High, RateTier::Normal, RateTier::Low]);
    }

    #[test]
    fn test_poll_reads_subscribed_tag_with_sign_decoding() {
        let (mut svc, state) = fake_service();
        state.lock().unwrap().words.insert("D100".into(), ScalarValue::Number(-5.0));
        svc.subscriptions.enter_viewport("c", "v", vec![EXT_TIME.into()], 0);

        let report = svc.poll_cycle(0);
        assert_eq!(report.blocks_failed, 0);
        assert!(report.blocks_read >= 1);
        let snap = svc.store.get_snapshots(&[EXT_TIME.to_string()]).remove(EXT_TIME).unwrap();
        assert_eq!(snap.value, ScalarValue::Number(-5.0));
    }

    #[test]
    fn test_urgent_queue_drains_fifo_before_polling() {
        let (mut svc, state) = fake_service();
        let first = svc.submit_write(write_cmd(EXT_TIME, 1000.0, false));
        let second = svc.submit_write(write_cmd(EXT_TIME, 2000.0, false));
        assert_eq!(svc.urgent_write_queue.len(), 2);

        svc.subscriptions.enter_viewport("c", "v", vec![EXT_TIME.into()], 0);
        let report = svc.poll_cycle(0);
        assert_eq!(report.writes_executed, 2);
        assert!(svc.urgent_write_queue.is_empty());
        assert!(matches!(svc.take_write_result(first), Some(Ok(_))));
        assert!(matches!(svc.take_write_result(second), Some(Ok(_))));

        // 寫入先於讀取：本輪讀到的是最後寫入的 2000
        assert_eq!(state.lock().unwrap().words.get("D100"), Some(&ScalarValue::Number(2000.0)));
        let snap = svc.store.get_snapshots(&[EXT_TIME.to_string()]).remove(EXT_TIME).unwrap();
        assert_eq!(snap.value, ScalarValue::Number(2000.0));
    }

    #[test]
    fn test_poll_failures_trip_breaker_and_block_writes() {
        let (mut svc, state) = fake_service();
        state.lock().unwrap().fail_reads = true;
        svc.subscriptions.enter_viewport("c", "v", vec![EXT_TIME.into()], 0);
        for now in [0, 1000, 2000] {
            svc.poll_cycle(now);
        }
        assert_eq!(svc.breakers["plc1"].state, StationHealthState::TrippedDegraded);
        assert!(matches!(
            svc.write_tag(EXT_TIME, ScalarValue::Number(1.0), "op1", "127.0.0.1"),
            Err(WriteRejectReason::DeviceOfflineError { .. })
        ));
    }

    #[test]
    fn test_batch_verify_detects_readback_mismatch() {
        let (mut svc, state) = fake_service();
        state.lock().unwrap().drift = true;
        let report = svc.write_batch(vec![write_cmd(EXT_TIME, 1500.0, true)]);
        let verification = report.verification.expect("標記 verify_batch 應進行校驗");
        assert_eq!(verification.checked, 1);
        assert_eq!(verification.mismatches.len(), 1);
        assert_eq!(verification.mismatches[0].expected, ScalarValue::Number(1500.0));
        assert_eq!(verification.mismatches[0].actual, ScalarValue::Number(1501.0));
        assert!(verification.read_errors.is_empty());
    }

    #[test]
    fn test_batch_verify_passes_and_is_skipped_without_flag() {
        let (mut svc, _state) = fake_service();
        let verified = svc.write_batch(vec![write_cmd(EXT_TIME, 1500.0, true)]);
        let verification = verified.verification.expect("標記 verify_batch 應進行校驗");
        assert_eq!(verification.checked, 1);
        assert!(verification.mismatches.is_empty());

        let plain = svc.write_batch(vec![write_cmd(EXT_TIME, 1600.0, false)]);
        assert!(plain.verification.is_none());
        assert!(plain.results.iter().all(|(_, result)| result.is_ok()));
    }
}
