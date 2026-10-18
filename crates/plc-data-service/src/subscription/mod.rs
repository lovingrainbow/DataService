// crates/plc-data-service/src/subscription/mod.rs
// 2. 雙軌分層動態訂閱管理器 (subscription: §4 Permanent + Viewport + 3s Cooldown)

pub mod session;

pub use session::{SubscriptionStats, ViewportKey, ViewportSession};

use std::collections::{BTreeMap, BTreeSet};

use tracing::debug;

pub struct SubscriptionManager {
    /// 軌道 1：永久常駐軌 (永不退訂，§4.1)
    permanent_tags: BTreeSet<String>,
    /// 軌道 2：動態視角軌會話表
    sessions: BTreeMap<ViewportKey, ViewportSession>,
    /// 待執行首幀 Burst 優先採樣的點位集合 (§4.2)
    burst_queue: BTreeSet<String>,
    /// 退訂冷卻秒數 (預設 3000 ms)
    pub cooldown_duration_ms: u64,
    /// 殭屍連線逾時門檻 (預設 30000 ms)
    pub zombie_timeout_ms: u64,
}

impl SubscriptionManager {
    pub fn new(permanent_tags: impl IntoIterator<Item = String>) -> Self {
        Self {
            permanent_tags: permanent_tags.into_iter().collect(),
            sessions: BTreeMap::new(),
            burst_queue: BTreeSet::new(),
            cooldown_duration_ms: 3_000,
            zombie_timeout_ms: 30_000,
        }
    }

    pub fn register_permanent(&mut self, tag_id: impl Into<String>) {
        self.permanent_tags.insert(tag_id.into());
    }

    /// §4.2 客戶端進入視角 (`datasvc_enter_view`)：
    /// 若該視角正在 3 秒退訂冷卻中，直接重置取消冷卻；否則將新點位排入 Burst Priority 首幀佇列
    pub fn enter_viewport(
        &mut self,
        client_id: impl Into<String>,
        view_id: impl Into<String>,
        tags: Vec<String>,
        now_ms: u64,
    ) -> bool {
        let key = ViewportKey {
            client_id: client_id.into(),
            view_id: view_id.into(),
        };
        let tag_set: BTreeSet<String> = tags.into_iter().collect();
        debug!(client_id = %key.client_id, view_id = %key.view_id, tag_count = tag_set.len(), "viewport entered");

        if let Some(existing) = self.sessions.get_mut(&key) {
            let was_cooling = existing.cooldown_until_ms.is_some();
            existing.cooldown_until_ms = None;
            existing.last_heartbeat_ms = now_ms;
            for t in &tag_set {
                if !existing.tags.contains(t) {
                    self.burst_queue.insert(t.clone());
                }
            }
            existing.tags = tag_set;
            return was_cooling;
        }

        for t in &tag_set {
            self.burst_queue.insert(t.clone());
        }

        self.sessions.insert(
            key.clone(),
            ViewportSession {
                key,
                tags: tag_set,
                first_frame_ready: false,
                cooldown_until_ms: None,
                last_heartbeat_ms: now_ms,
            },
        );
        false
    }

    /// §4.2 客戶端離開視角 (`datasvc_leave_view`)：啟動 3 秒退訂冷卻計時器，不立即刪除點位
    pub fn leave_viewport(&mut self, client_id: &str, view_id: &str, now_ms: u64) {
        let key = ViewportKey {
            client_id: client_id.to_string(),
            view_id: view_id.to_string(),
        };
        if let Some(session) = self.sessions.get_mut(&key) {
            session.cooldown_until_ms = Some(now_ms + self.cooldown_duration_ms);
            debug!(client_id, view_id, cooldown_ms = self.cooldown_duration_ms, "viewport left; entering cooldown");
        }
    }

    /// 客戶端心跳更新
    pub fn touch_client(&mut self, client_id: &str, now_ms: u64) {
        for session in self.sessions.values_mut() {
            if session.key.client_id == client_id {
                session.last_heartbeat_ms = now_ms;
            }
        }
    }

    /// §4.2 殭屍連線自動回收 (Zombie Auto-GC) 與 3 秒退訂冷卻到期清理
    pub fn tick_maintenance(&mut self, now_ms: u64) -> Vec<ViewportKey> {
        let mut expired = Vec::new();
        for (k, s) in &self.sessions {
            let cooldown_expired = s.cooldown_until_ms.map(|deadline| now_ms >= deadline).unwrap_or(false);
            let zombie_expired = now_ms.saturating_sub(s.last_heartbeat_ms) > self.zombie_timeout_ms;
            if cooldown_expired || zombie_expired {
                expired.push(k.clone());
            }
        }
        for k in &expired {
            self.sessions.remove(k);
        }
        if !expired.is_empty() {
            debug!(expired_count = expired.len(), "reclaimed expired viewport sessions");
        }
        expired
    }

    /// 取出待執行首幀 Burst 優先掃描的點位，並將對應視角標記為 `ViewportReady`
    pub fn drain_burst_tags(&mut self) -> (Vec<String>, Vec<ViewportKey>) {
        let tags: Vec<String> = self.burst_queue.iter().cloned().collect();
        self.burst_queue.clear();

        let mut ready_views = Vec::new();
        for (k, s) in &mut self.sessions {
            if !s.first_frame_ready {
                s.first_frame_ready = true;
                ready_views.push(k.clone());
            }
        }
        (tags, ready_views)
    }

    /// 計算當前所有需要被排程器輪詢的聯集點位 (Permanent ∪ Active/Cooling Viewports)
    pub fn active_polled_tags(&self) -> BTreeSet<String> {
        let mut set = self.permanent_tags.clone();
        for s in self.sessions.values() {
            for t in &s.tags {
                set.insert(t.clone());
            }
        }
        set
    }

    /// 取得雙軌訂閱統計與每個 Tag 的引用計數 (`ref_count`)
    pub fn stats(&self, now_ms: u64) -> SubscriptionStats {
        let mut ref_counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut cooling = Vec::new();

        for s in self.sessions.values() {
            for t in &s.tags {
                *ref_counts.entry(t.clone()).or_insert(0) += 1;
            }
            if let Some(until) = s.cooldown_until_ms {
                let remain = until.saturating_sub(now_ms);
                cooling.push((s.key.client_id.clone(), s.key.view_id.clone(), remain));
            }
        }

        SubscriptionStats {
            permanent_tags: self.permanent_tags.iter().cloned().collect(),
            active_viewport_tags: ref_counts,
            cooling_down_views: cooling,
            pending_burst_tags: self.burst_queue.iter().cloned().collect(),
        }
    }
}
