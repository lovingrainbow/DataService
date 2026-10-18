// crates/plc-data-service/src/config.rs
//! `service.toml` 的執行期設定：速率層間隔 (`[poll]`) 與各站點緊湊逾時 (`[station.<id>]`)
//!
//! `[[tag_override]]` 由 `machine_model::datasvc` 解析；本模組忽略該區段與其他未知的頂層區段。
//! 省略的鍵使用預設值：速率為 RS-485 預設，緊湊逾時為 50 ms。
//!
//! ```toml
//! [poll]
//! high_ms = 100
//! normal_ms = 500
//! low_ms = 2000
//!
//! [station.plc1]            # 站點 ID 須與機台模型中的站點一致
//! tight_timeout_ms = 50     # 緊湊逾時 (ms)，不得低於 25
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::poller::PollRatePolicy;

/// 緊湊逾時的下限 (ms)，與熔斷器的下限一致
pub const MIN_TIGHT_TIMEOUT_MS: u64 = 25;

/// 未設定站點逾時時使用的緊湊逾時 (ms) (§7.1)
pub const DEFAULT_TIGHT_TIMEOUT_MS: u64 = 50;

/// `service.toml` 解析後的執行期設定
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    poll: PollSection,
    station: BTreeMap<String, StationSection>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PollSection {
    high_ms: Option<u64>,
    normal_ms: Option<u64>,
    low_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StationSection {
    tight_timeout_ms: Option<u64>,
}

impl ServiceConfig {
    /// 解析 `service.toml` 內容並驗證數值範圍
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| format!("service.toml 解析失敗: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// 驗證速率層與各站點逾時；`from_toml_str` 與建構服務時都會呼叫
    pub fn validate(&self) -> Result<(), String> {
        self.poll_rates().validate()?;
        for (station, section) in &self.station {
            if let Some(ms) = section.tight_timeout_ms {
                if ms < MIN_TIGHT_TIMEOUT_MS {
                    return Err(format!(
                        "[station.{station}] tight_timeout_ms 不得小於 {MIN_TIGHT_TIMEOUT_MS} ms，實際 {ms}"
                    ));
                }
            }
        }
        Ok(())
    }

    /// 速率層間隔；省略的鍵使用 RS-485 預設值
    pub fn poll_rates(&self) -> PollRatePolicy {
        let base = PollRatePolicy::rs485_default();
        PollRatePolicy {
            high_ms: self.poll.high_ms.unwrap_or(base.high_ms),
            normal_ms: self.poll.normal_ms.unwrap_or(base.normal_ms),
            low_ms: self.poll.low_ms.unwrap_or(base.low_ms),
        }
    }

    /// 站點的緊湊逾時 (ms)；未設定時為 50 ms
    pub fn tight_timeout_ms(&self, station_id: &str) -> u64 {
        self.station
            .get(station_id)
            .and_then(|section| section.tight_timeout_ms)
            .unwrap_or(DEFAULT_TIGHT_TIMEOUT_MS)
    }

    /// `[station.<id>]` 區段列出的站點 ID
    pub fn station_ids(&self) -> impl Iterator<Item = &str> {
        self.station.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
[[tag_override]]
id = "plc1.M_Cyl1_ManExt"
writable = true

[poll]
high_ms = 100
normal_ms = 500
low_ms = 2000

[station.plc1]
tight_timeout_ms = 60
"#;

    #[test]
    fn test_full_config_is_parsed_and_tag_override_ignored() {
        let config = ServiceConfig::from_toml_str(FULL).unwrap();
        assert_eq!(config.poll_rates(), PollRatePolicy { high_ms: 100, normal_ms: 500, low_ms: 2_000 });
        assert_eq!(config.tight_timeout_ms("plc1"), 60);
        assert_eq!(config.station_ids().collect::<Vec<_>>(), vec!["plc1"]);
    }

    #[test]
    fn test_empty_config_uses_defaults() {
        let config = ServiceConfig::from_toml_str("").unwrap();
        assert_eq!(config.poll_rates(), PollRatePolicy::rs485_default());
        assert_eq!(config.tight_timeout_ms("plc1"), DEFAULT_TIGHT_TIMEOUT_MS);
    }

    #[test]
    fn test_missing_poll_keys_fall_back_to_rs485_defaults() {
        let config = ServiceConfig::from_toml_str("[poll]\nhigh_ms = 50\n").unwrap();
        assert_eq!(config.poll_rates(), PollRatePolicy { high_ms: 50, normal_ms: 500, low_ms: 2_000 });
    }

    #[test]
    fn test_unknown_poll_key_is_rejected() {
        assert!(ServiceConfig::from_toml_str("[poll]\nhigh = 50\n").is_err());
    }

    #[test]
    fn test_inverted_rates_are_rejected() {
        assert!(ServiceConfig::from_toml_str("[poll]\nhigh_ms = 600\nnormal_ms = 500\n").is_err());
    }

    #[test]
    fn test_tight_timeout_below_floor_is_rejected() {
        assert!(ServiceConfig::from_toml_str("[station.plc1]\ntight_timeout_ms = 10\n").is_err());
    }

    #[test]
    fn test_syntax_error_is_rejected() {
        assert!(ServiceConfig::from_toml_str("[poll\nhigh_ms = 50").is_err());
    }
}
