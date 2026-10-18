// crates/plc-data-service/src/cache/mod.rs
// 1. 快取與核心型別模組 (cache: §2 TagSnapshot, DataQuality, Deadband, COS)

pub mod snapshot;

pub use snapshot::{DataQuality, DeadbandConfig, TagChangeEvent, TagRouteMeta, TagSnapshot};

use std::collections::BTreeMap;

use machine_model::model::ScalarValue;
use tokio::sync::broadcast;

use crate::now_ns;

/// 即時記憶體快取 (In-Memory TagStore, 讀取延遲 < 1 µs)
pub struct TagStore {
    snapshots: BTreeMap<String, TagSnapshot>,
    routes: BTreeMap<String, TagRouteMeta>,
    event_tx: broadcast::Sender<TagChangeEvent>,
}

impl TagStore {
    pub fn new(capacity: usize) -> (Self, broadcast::Receiver<TagChangeEvent>) {
        let (tx, rx) = broadcast::channel(capacity.max(256));
        (
            Self {
                snapshots: BTreeMap::new(),
                routes: BTreeMap::new(),
                event_tx: tx,
            },
            rx,
        )
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<TagChangeEvent> {
        self.event_tx.subscribe()
    }

    pub fn register_tag(&mut self, route: TagRouteMeta, initial_val: ScalarValue, initial_quality: DataQuality) {
        let snap = TagSnapshot {
            tag_id: route.tag_id.clone(),
            value: initial_val,
            quality: initial_quality,
            timestamp_ns: now_ns(),
            version: 1,
        };
        self.snapshots.insert(route.tag_id.clone(), snap);
        self.routes.insert(route.tag_id.clone(), route);
    }

    pub fn get_route(&self, tag_id: &str) -> Option<&TagRouteMeta> {
        self.routes.get(tag_id)
    }

    pub fn all_routes(&self) -> &BTreeMap<String, TagRouteMeta> {
        &self.routes
    }

    /// 直接記憶體讀取單一或多點快照 (< 1 µs)
    pub fn get_snapshots(&self, tags: &[String]) -> BTreeMap<String, TagSnapshot> {
        if tags.is_empty() {
            return self.snapshots.clone();
        }
        let mut out = BTreeMap::new();
        for t in tags {
            if let Some(s) = self.snapshots.get(t) {
                out.insert(t.clone(), s.clone());
            }
        }
        out
    }

    /// 更新點位數值與品質；僅當超越死區 (Deadband) 或品質改變時才發送 COS 事件 (§8.2 Zero-Waste)
    pub fn update_tag(
        &mut self,
        tag_id: &str,
        new_value: ScalarValue,
        new_quality: DataQuality,
    ) -> Option<TagChangeEvent> {
        let deadband = self
            .routes
            .get(tag_id)
            .map(|r| r.deadband)
            .unwrap_or(DeadbandConfig::None);

        let ts = now_ns();
        if let Some(existing) = self.snapshots.get_mut(tag_id) {
            let value_changed = deadband.exceeds(&existing.value, &new_value);
            let quality_changed = existing.quality != new_quality;

            existing.timestamp_ns = ts;
            if value_changed || quality_changed {
                let old_val = Some(existing.value.clone());
                existing.value = new_value.clone();
                existing.quality = new_quality;
                existing.version = existing.version.wrapping_add(1);

                let ev = TagChangeEvent {
                    tag_id: tag_id.to_string(),
                    old_value: old_val,
                    new_value,
                    quality: new_quality,
                    timestamp_ns: ts,
                    version: existing.version,
                };
                let _ = self.event_tx.send(ev.clone());
                return Some(ev);
            }
            None
        } else {
            let snap = TagSnapshot {
                tag_id: tag_id.to_string(),
                value: new_value.clone(),
                quality: new_quality,
                timestamp_ns: ts,
                version: 1,
            };
            self.snapshots.insert(tag_id.to_string(), snap);
            let ev = TagChangeEvent {
                tag_id: tag_id.to_string(),
                old_value: None,
                new_value,
                quality: new_quality,
                timestamp_ns: ts,
                version: 1,
            };
            let _ = self.event_tx.send(ev.clone());
            Some(ev)
        }
    }

    /// 當某站點發生斷線或熔斷時，將該站所有點位品質標記為 Bad (保留最後已知值，§7.3)
    pub fn mark_station_quality(&mut self, station_id: &str, quality: DataQuality) -> Vec<TagChangeEvent> {
        let target_tags: Vec<String> = self
            .routes
            .values()
            .filter(|r| r.station_id == station_id)
            .map(|r| r.tag_id.clone())
            .collect();

        let mut events = Vec::new();
        for tid in target_tags {
            if let Some(cur_val) = self.snapshots.get(&tid).map(|s| s.value.clone()) {
                if let Some(ev) = self.update_tag(&tid, cur_val, quality) {
                    events.push(ev);
                }
            }
        }
        events
    }
}
