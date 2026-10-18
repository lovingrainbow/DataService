// crates/datasvc-demo/src/verify.rs
//! DATA_SERVICE_SPEC.md §1 ~ §9 逐項驗證報告
//!
//! 每個檢查都會實際呼叫 plc-data-service 並比對結果。規格中尚未實作的條目以 GAP 標示，
//! 不計入通過，讓報告反映真實的實作落差。

use machine_model::model::{ScalarValue, ScanRate};
use machine_model::resolver::ResolvedMachineModel;
use mm_drivers::{ByteOrder, SimMode};
use plc_data_service::PlcDataService;
use plc_data_service::arbiter::WriteRejectReason;
use plc_data_service::cache::{DataQuality, DeadbandConfig, TagRouteMeta, TagSnapshot, TagStore};
use plc_data_service::gateway::{StationCircuitBreaker, StationHealthState};
use plc_data_service::poller::{CoalescingPolicy, coalesce_register_blocks};
use plc_data_service::subscription::SubscriptionManager;

use crate::model::{STATION_ID, build_service_from, demo_resolved_model};

const OPERATOR_ID: &str = "SPEC_VERIFIER";
const CLIENT_IP: &str = "127.0.0.1";
const EXT_TIME_TAG: &str = "plc1.D_Cyl1_ExtTime";

enum Outcome {
    Pass,
    Fail(String),
    Gap(&'static str),
    NotApplicable(&'static str),
}

#[derive(Default)]
struct Report {
    rows: Vec<(&'static str, &'static str, Outcome)>,
}

impl Report {
    fn check(&mut self, section: &'static str, name: &'static str, check: impl FnOnce() -> Result<(), String>) {
        let outcome = match check() {
            Ok(()) => Outcome::Pass,
            Err(reason) => Outcome::Fail(reason),
        };
        self.rows.push((section, name, outcome));
    }

    fn gap(&mut self, section: &'static str, name: &'static str, reason: &'static str) {
        self.rows.push((section, name, Outcome::Gap(reason)));
    }

    fn not_applicable(&mut self, section: &'static str, name: &'static str, reason: &'static str) {
        self.rows.push((section, name, Outcome::NotApplicable(reason)));
    }

    /// 印出報告，回傳失敗筆數
    fn print(&self) -> usize {
        let (mut pass, mut fail, mut gap, mut na) = (0, 0, 0, 0);
        println!("==================================================================");
        println!("  PlcDataService 規格驗證報告 (DATA_SERVICE_SPEC.md §1 ~ §9)");
        println!("==================================================================");
        for (section, name, outcome) in &self.rows {
            match outcome {
                Outcome::Pass => {
                    pass += 1;
                    println!("[PASS] {section:<6} {name}");
                }
                Outcome::Fail(reason) => {
                    fail += 1;
                    println!("[FAIL] {section:<6} {name}\n         -> {reason}");
                }
                Outcome::Gap(reason) => {
                    gap += 1;
                    println!("[GAP ] {section:<6} {name}\n         -> {reason}");
                }
                Outcome::NotApplicable(reason) => {
                    na += 1;
                    println!("[N/A ] {section:<6} {name}\n         -> {reason}");
                }
            }
        }
        println!("------------------------------------------------------------------");
        println!("通過 {pass}  失敗 {fail}  未實作 {gap}  不適用 {na}");
        fail
    }
}

/// 執行全部規格檢查並印出報告，回傳失敗筆數
pub fn run_report() -> usize {
    let resolved = demo_resolved_model();
    let mut report = Report::default();

    report.not_applicable("§1", "架構定位與拓撲", "設計說明，由 crate 拆分與 cargo test 體現");
    report.not_applicable("§2", "核心型別", "型別定義由編譯器與 serde 保證，行為面由 §8.2 驗證");

    report.check("§3.1", "模型載入後每個點位都建立路由並綁定位址", || check_model_loaded(&resolved));

    report.check("§3.2", "護欄 1：唯讀點位攔截", || check_guardrail_read_only(&resolved));
    match find_safety_monitor_tag(&resolved) {
        Some(tag) => report.check("§3.2", "護欄 2：安全監控點位阻擋", || {
            check_guardrail_safety_monitor(&resolved, &tag)
        }),
        None => report.not_applicable("§3.2", "護欄 2：安全監控點位阻擋", "示範模型中沒有 safety_monitor 點位"),
    }
    report.check("§3.2", "護欄 3：上下限攔截，範圍內寫入成功", || check_guardrail_range(&resolved));
    report.check("§3.2", "護欄 4：審計日誌記錄每次嘗試並標示是否允許", || check_audit_trail(&resolved));

    report.check("§4.1", "永久軌點位不受視角退訂影響", || check_permanent_track(&resolved));
    report.check("§4.2", "引用計數：仍有視角引用時不退訂", check_ref_count);
    report.check("§4.2", "3 秒退訂冷卻：到期前保留、到期後移除", check_cooldown_boundary);
    report.check("§4.2", "冷卻期間重新進入會取消退訂", check_reenter_cancels_cooldown);
    report.check("§4.2", "首幀就緒：Burst 掃描一次後視角標記為 ready", check_readiness_guard);
    report.check("§4.2", "殭屍連線回收：心跳逾時 30 秒後回收", check_zombie_gc);

    report.check("§5.1", "相鄰位址在間隙容忍內合併為單一區塊", check_coalescing_merge);
    report.check("§5.1", "間隙超過 MaxGap 時拆分區塊", check_coalescing_gap_split);
    report.check("§5.1", "合併長度超過 MaxBlock 時拆分區塊", check_coalescing_block_limit);
    report.gap(
        "§5.2",
        "多速率分群佇列 (High 50~100ms / Normal 200~500ms / Low 1000~3000ms)",
        "尚未實作：只保留 ScanRate 欄位，沒有依速率分群的時間輪或排程器",
    );

    report.check("§6.1", "寫入驅動失敗時，快取保持不變", || check_driver_failure_keeps_cache(&resolved));
    report.gap(
        "§6.1",
        "最高優先權 Urgent 寫入佇列",
        "尚未實作：urgent_write_queue 欄位從未推入或取出，寫入為同步直寫",
    );
    report.check("§6.2", "寫入 ACK 後直接更新快取，回傳值與快取一致", || check_write_commit_cache(&resolved));
    report.gap(
        "§6.2",
        "批次寫入後的 verify_batch 一次性回讀校驗",
        "尚未實作：PreemptiveWriteCommand.verify_batch 欄位存在但未使用",
    );
    report.check("§6.3", "斷線或熔斷期間寫入立即拒絕且不排隊", || check_fail_fast(&resolved));

    report.check("§7", "熔斷器保存緊湊逾時設定 50ms (僅驗證設定值)", || check_tight_timeout(&resolved));
    report.gap(
        "§7",
        "緊湊逾時實際套用於通訊：RS-485 理論耗時 + 25ms",
        "尚未實作：tight_timeout_ms 只儲存，未被驅動使用；驅動仍採用 DriverConfig.timeout_ms (預設 1000ms)",
    );
    report.check("§7", "連續 3 次失敗才觸發熔斷", || check_trip_after_three(&resolved));
    report.check("§7", "熔斷時保留最後已知值並標記為 Bad", || check_stale_marking(&resolved));
    report.check("§7", "熔斷期間每 5000ms 放行一次探測", check_probe_interval);
    report.check("§7", "站點恢復後品質重新標記為 Good", || check_recovery_restores_quality(&resolved));

    report.not_applicable("§8.1", "Tauri IPC 指令", "指令定義於 src-tauri，本示範不驗證");
    report.check("§8.2", "死區過濾：死區內不發 COS，超出死區或品質改變才發出", check_deadband_cos);

    report.not_applicable("§9", "專案模組架構", "工程結構由 cargo build / test 驗證");

    report.print()
}

fn ensure(cond: bool, reason: impl Into<String>) -> Result<(), String> {
    if cond { Ok(()) } else { Err(reason.into()) }
}

fn sample_route(tag: &str, word_index: u32, word_len: u16) -> TagRouteMeta {
    TagRouteMeta {
        tag_id: tag.to_string(),
        station_id: STATION_ID.to_string(),
        register_address: format!("D{word_index}"),
        area_prefix: "D".to_string(),
        word_index,
        bit_offset: None,
        word_len,
        data_type: "int16".to_string(),
        byte_order: ByteOrder::CDAB,
        scan_rate: ScanRate::Fast,
        deadband: DeadbandConfig::None,
        system_critical: false,
        final_writable: true,
        safety_monitor: false,
        min_val: None,
        max_val: None,
    }
}

fn snapshot(svc: &PlcDataService, tag: &str) -> Option<TagSnapshot> {
    svc.store.get_snapshots(&[tag.to_string()]).remove(tag)
}

fn snapshot_value(svc: &PlcDataService, tag: &str) -> Option<ScalarValue> {
    snapshot(svc, tag).map(|s| s.value)
}

fn ext_time_bounds(svc: &PlcDataService) -> Result<(f64, f64), String> {
    let route = svc.store.get_route(EXT_TIME_TAG).ok_or("缺少 plc1.D_Cyl1_ExtTime 路由")?;
    let min = route.min_val.ok_or("點位缺少下限")?;
    let max = route.max_val.ok_or("點位缺少上限")?;
    Ok((min, max))
}

fn find_safety_monitor_tag(resolved: &ResolvedMachineModel) -> Option<String> {
    build_service_from(resolved)
        .store
        .all_routes()
        .values()
        .find(|r| r.safety_monitor)
        .map(|r| r.tag_id.clone())
}

/// 讓站點連續 3 次通訊失敗而熔斷
fn trip_station(svc: &mut PlcDataService) -> Result<(), String> {
    let mut state = StationHealthState::Connected;
    for now in [1000, 1050, 1100] {
        state = svc.report_station_comm_result(STATION_ID, false, now);
    }
    ensure(state == StationHealthState::TrippedDegraded, format!("預期熔斷，實際狀態 {state:?}"))
}

// ---------------------------------------------------------------- §3

fn check_model_loaded(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let svc = build_service_from(resolved);
    ensure(
        svc.store.all_routes().len() == resolved.effective_tags.len(),
        format!("路由數 {} 與有效點位數 {} 不符", svc.store.all_routes().len(), resolved.effective_tags.len()),
    )?;
    ensure(svc.breakers.contains_key(STATION_ID), "站點熔斷器未建立")?;
    let route = svc.store.get_route(EXT_TIME_TAG).ok_or("缺少 plc1.D_Cyl1_ExtTime 路由")?;
    ensure(
        route.register_address == "D100",
        format!("位址應為 D100，實際 {}", route.register_address),
    )
}

fn check_guardrail_read_only(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    match svc.write_tag("plc1.X_Cyl1_Ext_LS", ScalarValue::Bool(true), OPERATOR_ID, CLIENT_IP) {
        Err(WriteRejectReason::ReadOnlyGuardrail { .. }) => Ok(()),
        other => Err(format!("預期唯讀攔截，實際結果: {other:?}")),
    }
}

fn check_guardrail_safety_monitor(resolved: &ResolvedMachineModel, tag: &str) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let is_bool = svc.store.get_route(tag).is_some_and(|r| r.data_type == "bool");
    let value = if is_bool { ScalarValue::Bool(true) } else { ScalarValue::Number(0.0) };
    match svc.write_tag(tag, value, OPERATOR_ID, CLIENT_IP) {
        Err(WriteRejectReason::SafetyMonitorGuardrail { .. }) => Ok(()),
        other => Err(format!("安全監控點位 {tag} 應被阻擋，實際結果: {other:?}")),
    }
}

fn check_guardrail_range(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (min, max) = ext_time_bounds(&svc)?;
    for out_of_range in [max + 1.0, min - 1.0] {
        match svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(out_of_range), OPERATOR_ID, CLIENT_IP) {
            Err(WriteRejectReason::OutOfBoundsError { .. }) => {}
            other => return Err(format!("超界值 {out_of_range} 應被攔截，實際結果: {other:?}")),
        }
    }
    let accepted = svc
        .write_tag(EXT_TIME_TAG, ScalarValue::Number(max), OPERATOR_ID, CLIENT_IP)
        .map_err(|e| format!("範圍內寫入不應被拒絕: {e}"))?;
    ensure(accepted.written_value == ScalarValue::Number(max), "寫入結果應為寫入值")?;
    ensure(
        snapshot_value(&svc, EXT_TIME_TAG) == Some(ScalarValue::Number(max)),
        "寫入成功後快取應更新為寫入值",
    )
}

fn check_audit_trail(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (_, max) = ext_time_bounds(&svc)?;
    let before = svc.audit_trail.len();
    let _ = svc.write_tag("plc1.X_Cyl1_Ext_LS", ScalarValue::Bool(true), OPERATOR_ID, CLIENT_IP);
    let _ = svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(max + 1.0), OPERATOR_ID, CLIENT_IP);
    let _ = svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(max), OPERATOR_ID, CLIENT_IP);

    ensure(
        svc.audit_trail.len() == before + 3,
        format!("3 次寫入嘗試應產生 3 筆審計紀錄，實際新增 {}", svc.audit_trail.len() - before),
    )?;
    let newest = svc.audit_trail.front().ok_or("審計日誌為空")?;
    ensure(
        newest.permitted && newest.operator_id == OPERATOR_ID,
        "最新一筆應為允許的寫入，且記錄操作者",
    )?;
    ensure(
        svc.audit_trail.iter().skip(1).take(2).all(|e| !e.permitted),
        "較舊的兩筆應為被拒絕的寫入",
    )
}

// ---------------------------------------------------------------- §4

fn check_permanent_track(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let critical: Vec<String> = svc
        .store
        .all_routes()
        .values()
        .filter(|r| r.system_critical)
        .map(|r| r.tag_id.clone())
        .collect();
    ensure(!critical.is_empty(), "模型中沒有永久軌點位")?;
    let plain = svc
        .store
        .all_routes()
        .values()
        .find(|r| !r.system_critical)
        .map(|r| r.tag_id.clone())
        .ok_or("模型中沒有非永久軌點位，無法驗證退訂")?;

    svc.subscriptions.enter_viewport("chk", "perm_view", vec![plain.clone()], 0);
    svc.subscriptions.leave_viewport("chk", "perm_view", 0);
    svc.subscriptions.tick_maintenance(10_000);

    let active = svc.subscriptions.active_polled_tags();
    ensure(!active.contains(&plain), "非永久軌點位在冷卻到期後應退訂")?;
    for tag in &critical {
        ensure(active.contains(tag), format!("永久軌點位 {tag} 不應被退訂"))?;
    }
    Ok(())
}

fn check_ref_count() -> Result<(), String> {
    let mut mgr = SubscriptionManager::new(Vec::<String>::new());
    mgr.enter_viewport("c1", "v1", vec!["T".into()], 0);
    mgr.enter_viewport("c2", "v2", vec!["T".into()], 0);
    mgr.leave_viewport("c1", "v1", 1000);
    mgr.tick_maintenance(4000);
    ensure(mgr.active_polled_tags().contains("T"), "仍有其他視角引用時不應退訂")?;

    mgr.leave_viewport("c2", "v2", 5000);
    mgr.tick_maintenance(8000);
    ensure(!mgr.active_polled_tags().contains("T"), "引用計數歸零後應退訂")
}

fn check_cooldown_boundary() -> Result<(), String> {
    let mut mgr = SubscriptionManager::new(Vec::<String>::new());
    mgr.enter_viewport("c", "v", vec!["T".into()], 0);
    mgr.leave_viewport("c", "v", 1000);
    mgr.tick_maintenance(3999);
    ensure(mgr.active_polled_tags().contains("T"), "冷卻未到期 (3999ms) 時應保留輪詢")?;
    mgr.tick_maintenance(4000);
    ensure(!mgr.active_polled_tags().contains("T"), "冷卻到期 (4000ms) 後應退訂")
}

fn check_reenter_cancels_cooldown() -> Result<(), String> {
    let mut mgr = SubscriptionManager::new(Vec::<String>::new());
    mgr.enter_viewport("c", "v", vec!["T".into()], 0);
    mgr.leave_viewport("c", "v", 1000);
    let was_cooling = mgr.enter_viewport("c", "v", vec!["T".into()], 2000);
    ensure(was_cooling, "重新進入時應回報仍在冷卻中")?;
    mgr.tick_maintenance(10_000);
    ensure(mgr.active_polled_tags().contains("T"), "重新進入後不應因原冷卻時間而退訂")
}

fn check_readiness_guard() -> Result<(), String> {
    let mut mgr = SubscriptionManager::new(Vec::<String>::new());
    mgr.enter_viewport("c", "v", vec!["T1".into(), "T2".into()], 0);
    let (tags, ready) = mgr.drain_burst_tags();
    ensure(
        tags == vec!["T1".to_string(), "T2".to_string()],
        format!("Burst 佇列應含 T1、T2，實際 {tags:?}"),
    )?;
    ensure(ready.len() == 1, "首次掃描後視角應標記為 ready")?;
    let (tags_again, ready_again) = mgr.drain_burst_tags();
    ensure(
        tags_again.is_empty() && ready_again.is_empty(),
        "第二次掃描不應再有 Burst 點位或 ready 視角",
    )
}

fn check_zombie_gc() -> Result<(), String> {
    let mut mgr = SubscriptionManager::new(Vec::<String>::new());
    mgr.enter_viewport("c", "v", vec!["T".into()], 0);
    ensure(mgr.tick_maintenance(30_000).is_empty(), "心跳逾時恰為 30000ms 時不應回收")?;
    let expired = mgr.tick_maintenance(30_001);
    ensure(expired.len() == 1, "心跳逾時超過 30000ms 後應回收該連線")?;
    ensure(!mgr.active_polled_tags().contains("T"), "回收後點位不應再被輪詢")
}

// ---------------------------------------------------------------- §5

fn check_coalescing_merge() -> Result<(), String> {
    let routes = vec![sample_route("A", 100, 1), sample_route("B", 103, 1), sample_route("C", 106, 1)];
    let policy = CoalescingPolicy { max_gap_words: 4, max_block_words: 64 };
    let blocks = coalesce_register_blocks(&routes, policy);
    ensure(blocks.len() == 1, format!("應合併為 1 個區塊，實際 {} 個", blocks.len()))?;
    let block = &blocks[0];
    ensure(
        block.start_address == "D100" && block.total_words == 7 && block.member_tags.len() == 3,
        format!("區塊應為 D100 起算 7 words 含 3 點位，實際 {} / {} / {}", block.start_address, block.total_words, block.member_tags.len()),
    )
}

fn check_coalescing_gap_split() -> Result<(), String> {
    let routes = vec![sample_route("A", 100, 1), sample_route("B", 200, 1)];
    let policy = CoalescingPolicy { max_gap_words: 4, max_block_words: 64 };
    let blocks = coalesce_register_blocks(&routes, policy);
    ensure(blocks.len() == 2, format!("間隙 99 words 應拆為 2 個區塊，實際 {} 個", blocks.len()))
}

fn check_coalescing_block_limit() -> Result<(), String> {
    let routes: Vec<TagRouteMeta> = (0..10).map(|i| sample_route(&format!("W{i}"), i, 1)).collect();
    let policy = CoalescingPolicy { max_gap_words: 4, max_block_words: 8 };
    let blocks = coalesce_register_blocks(&routes, policy);
    ensure(
        blocks.len() == 2 && blocks[0].total_words == 8 && blocks[1].total_words == 2,
        format!(
            "10 words 在上限 8 下應拆為 8 + 2，實際 {} 個區塊",
            blocks.len()
        ),
    )
}

// ---------------------------------------------------------------- §6

fn check_driver_failure_keeps_cache(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (_, max) = ext_time_bounds(&svc)?;
    let before = snapshot_value(&svc, EXT_TIME_TAG);

    let driver = svc.drivers.get_mut(STATION_ID).ok_or("找不到站點驅動")?;
    driver.set_simulation(SimMode::ReadbackFail).map_err(|e| e.to_string())?;

    match svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(max), OPERATOR_ID, CLIENT_IP) {
        Err(WriteRejectReason::DriverError(_)) => {}
        other => return Err(format!("驅動失敗應回傳 DriverError，實際結果: {other:?}")),
    }
    ensure(snapshot_value(&svc, EXT_TIME_TAG) == before, "驅動寫入失敗時快取不應更新")
}

fn check_write_commit_cache(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (_, max) = ext_time_bounds(&svc)?;
    let value = ScalarValue::Number(max);
    let result = svc
        .write_tag(EXT_TIME_TAG, value.clone(), OPERATOR_ID, CLIENT_IP)
        .map_err(|e| e.to_string())?;
    ensure(result.written_value == value, "回傳值應為寫入值")?;
    let snap = snapshot(&svc, EXT_TIME_TAG).ok_or("快取缺少點位")?;
    ensure(
        snap.value == value && snap.quality == DataQuality::Good,
        "寫入 ACK 後快取應立即更新為寫入值與 Good",
    )
}

fn check_fail_fast(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (_, max) = ext_time_bounds(&svc)?;
    let before = snapshot_value(&svc, EXT_TIME_TAG);
    trip_station(&mut svc)?;

    match svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(max), OPERATOR_ID, CLIENT_IP) {
        Err(WriteRejectReason::DeviceOfflineError { .. }) => {}
        other => return Err(format!("熔斷期間應回傳 DeviceOfflineError，實際結果: {other:?}")),
    }
    ensure(svc.urgent_write_queue.is_empty(), "離線期間不得排入寫入佇列")?;
    ensure(snapshot_value(&svc, EXT_TIME_TAG) == before, "被拒絕的寫入不應更新快取")
}

// ---------------------------------------------------------------- §7

fn check_tight_timeout(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let svc = build_service_from(resolved);
    let breaker = svc.breakers.get(STATION_ID).ok_or("缺少站點熔斷器")?;
    ensure(
        breaker.tight_timeout_ms == 50,
        format!("緊湊逾時應為 50ms，實際 {}", breaker.tight_timeout_ms),
    )
}

fn check_trip_after_three(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let s1 = svc.report_station_comm_result(STATION_ID, false, 1000);
    let s2 = svc.report_station_comm_result(STATION_ID, false, 1050);
    ensure(
        s1 == StationHealthState::Connected && s2 == StationHealthState::Connected,
        "前兩次失敗不應觸發熔斷",
    )?;
    let s3 = svc.report_station_comm_result(STATION_ID, false, 1100);
    ensure(s3 == StationHealthState::TrippedDegraded, format!("第 3 次失敗應熔斷，實際 {s3:?}"))
}

fn check_stale_marking(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    let (_, max) = ext_time_bounds(&svc)?;
    svc.write_tag(EXT_TIME_TAG, ScalarValue::Number(max), OPERATOR_ID, CLIENT_IP)
        .map_err(|e| e.to_string())?;
    trip_station(&mut svc)?;
    let snap = snapshot(&svc, EXT_TIME_TAG).ok_or("快取缺少點位")?;
    ensure(snap.value == ScalarValue::Number(max), "熔斷後應保留最後已知值")?;
    ensure(snap.quality == DataQuality::Bad, "熔斷後品質應為 Bad")
}

fn check_probe_interval() -> Result<(), String> {
    let mut cb = StationCircuitBreaker::new("S", 50);
    for _ in 0..3 {
        cb.record_failure(1000);
    }
    ensure(cb.state == StationHealthState::TrippedDegraded, "3 次失敗後應熔斷")?;
    ensure(!cb.should_allow_poll(5999), "熔斷後未滿 5000ms 不應探測")?;
    ensure(cb.should_allow_poll(6000), "熔斷後滿 5000ms 應放行一次探測")?;
    ensure(
        cb.state == StationHealthState::BackoffReconnecting,
        format!("探測中狀態應為 BackoffReconnecting，實際 {:?}", cb.state),
    )?;
    cb.record_success();
    ensure(cb.state == StationHealthState::Connected, "探測成功後應回到 Connected")
}

fn check_recovery_restores_quality(resolved: &ResolvedMachineModel) -> Result<(), String> {
    let mut svc = build_service_from(resolved);
    trip_station(&mut svc)?;
    let state = svc.report_station_comm_result(STATION_ID, true, 2000);
    ensure(state == StationHealthState::Connected, format!("恢復後應為 Connected，實際 {state:?}"))?;
    let snap = snapshot(&svc, EXT_TIME_TAG).ok_or("快取缺少點位")?;
    ensure(snap.quality == DataQuality::Good, "站點恢復後品質應回到 Good")
}

// ---------------------------------------------------------------- §8.2

fn check_deadband_cos() -> Result<(), String> {
    let (mut store, _rx) = TagStore::new(16);
    let mut route = sample_route("T.DB", 200, 1);
    route.deadband = DeadbandConfig::Absolute(5.0);
    store.register_tag(route, ScalarValue::Number(100.0), DataQuality::Good);

    ensure(
        store.update_tag("T.DB", ScalarValue::Number(103.0), DataQuality::Good).is_none(),
        "死區內變化 (|103-100| <= 5) 不應發出 COS",
    )?;
    let ev = store
        .update_tag("T.DB", ScalarValue::Number(106.0), DataQuality::Good)
        .ok_or("超出死區 (|106-100| > 5) 應發出 COS")?;
    ensure(ev.old_value == Some(ScalarValue::Number(100.0)), "COS 事件應帶有舊值 100")?;
    ensure(ev.new_value == ScalarValue::Number(106.0), "COS 事件應帶有新值 106")?;
    ensure(
        store.update_tag("T.DB", ScalarValue::Number(106.0), DataQuality::Bad).is_some(),
        "品質改變時應發出 COS",
    )
}
