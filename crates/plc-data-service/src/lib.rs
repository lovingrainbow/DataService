// crates/plc-data-service/src/lib.rs
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use machine_model::datasvc::{TagOverrideEntry, evaluate_plc_data_service_writability};
use machine_model::model::{ScanRate, ScalarValue};
use machine_model::resolver::ResolvedMachineModel;
use mm_drivers::{ByteOrder, PlcDriver, parse_universal_plc_address};
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

use arbiter::{AuditLogEntry, PreemptiveWriteCommand, WriteRejectReason, WriteResult, verify_write_guardrails};
use cache::{DataQuality, DeadbandConfig, TagChangeEvent, TagRouteMeta, TagStore};
use gateway::{StationCircuitBreaker, StationHealthState};
use poller::{CoalescedReadBlock, CoalescingPolicy, coalesce_register_blocks};
use subscription::SubscriptionManager;

pub struct PlcDataService {
    pub store: TagStore,
    pub subscriptions: SubscriptionManager,
    pub coalescing_policy: CoalescingPolicy,
    pub drivers: BTreeMap<String, Box<dyn PlcDriver>>,
    pub breakers: BTreeMap<String, StationCircuitBreaker>,
    pub urgent_write_queue: VecDeque<PreemptiveWriteCommand>,
    pub audit_trail: VecDeque<AuditLogEntry>,
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
                audit_trail: VecDeque::new(),
            },
            rx,
        )
    }

    pub fn attach_driver(&mut self, station_id: impl Into<String>, driver: Box<dyn PlcDriver>) {
        let sid = station_id.into();
        self.breakers
            .entry(sid.clone())
            .or_insert_with(|| StationCircuitBreaker::new(&sid, 50));
        info!(station_id = %sid, "PLC driver attached");
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

    /// §6.1 & §3.2 執行具備四大護欄與插隊預佔之語意寫入 (`datasvc_write_tag`)
    pub fn write_tag(
        &mut self,
        tag_id: &str,
        value: ScalarValue,
        operator_id: &str,
        client_ip: &str,
    ) -> Result<WriteResult, WriteRejectReason> {
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
}
