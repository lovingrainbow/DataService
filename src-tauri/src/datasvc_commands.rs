// src-tauri/src/datasvc_commands.rs
use std::collections::BTreeMap;
use parking_lot::Mutex;
use machine_model::model::ScalarValue;
use plc_data_service::{
    PlcDataService,
    arbiter::WriteResult,
    cache::TagSnapshot,
    poller::CoalescedReadBlock,
    subscription::SubscriptionStats,
};

pub struct DataSvcState {
    pub inner: Mutex<PlcDataService>,
}

/// 1. 讀取單點或多點快照 (Direct Memory Read, < 1 µs, §8.1)
#[tauri::command]
pub async fn datasvc_get_snapshot(
    state: tauri::State<'_, DataSvcState>,
    tags: Vec<String>,
) -> Result<BTreeMap<String, TagSnapshot>, String> {
    let svc = state.inner.lock();
    Ok(svc.store.get_snapshots(&tags))
}

/// 2. 視角進入：動態訂閱並排入 Burst Priority 首幀佇列 (§4.2 & §8.1)
#[tauri::command]
pub async fn datasvc_enter_view(
    state: tauri::State<'_, DataSvcState>,
    client_id: String,
    view_id: String,
    tags: Vec<String>,
) -> Result<bool, String> {
    let mut svc = state.inner.lock();
    Ok(svc.subscriptions.enter_viewport(client_id, view_id, tags, plc_data_service::now_ms()))
}

/// 3. 視角離開：啟動 3 秒退訂冷卻計時器 (§4.2 & §8.1)
#[tauri::command]
pub async fn datasvc_leave_view(
    state: tauri::State<'_, DataSvcState>,
    client_id: String,
    view_id: String,
) -> Result<(), String> {
    let mut svc = state.inner.lock();
    svc.subscriptions.leave_viewport(&client_id, &view_id, plc_data_service::now_ms());
    Ok(())
}

/// 4. 具備 §9.3 四大護欄與 Urgent 插隊預佔之語意寫入 (§6 & §8.1)
#[tauri::command]
pub async fn datasvc_write_tag(
    state: tauri::State<'_, DataSvcState>,
    tag: String,
    value: ScalarValue,
    operator: String,
) -> Result<WriteResult, String> {
    let mut svc = state.inner.lock();
    svc.write_tag(&tag, value, &operator, "ipc://local")
        .map_err(|r| r.to_string())
}

/// 5. 取得目前拼包排程區塊與雙軌訂閱狀態 (供開發者與診斷面板檢視)
#[tauri::command]
pub async fn datasvc_get_diagnostics(
    state: tauri::State<'_, DataSvcState>,
) -> Result<(SubscriptionStats, Vec<CoalescedReadBlock>), String> {
    let svc = state.inner.lock();
    let stats = svc.subscriptions.stats(plc_data_service::now_ms());
    let blocks = svc.compute_active_coalesced_blocks();
    Ok((stats, blocks))
}
