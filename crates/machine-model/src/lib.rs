//! # MachineModel v0.3 — 機台 Unit／Tag 模型建立、驗證與轉換器
//!
//! 本模組完整實作《Unit／Tag 模型規格（v0.3）》定義之 Rust 核心函式庫（`machine-model`）
//! 與命令列工具（`machinemodel`）：
//!
//! - **模型建立（Builder API & Serde 載入）**：支援 `machine.yaml` (schema_version: 2) 與 `unit-types/*.yaml`，
//!   並以 `#[serde(deny_unknown_fields)]` 嚴格攔截 v1 舊欄位（`writable`、`write_permission`、`unit_id`、
//!   `devices`、`alarms`、`parameters`）與舊版接腳屬性（`kind`、`level`、`reset`、`role`）。
//! - **三層驗證器（SC 規則、R 載入安全規則、U 關係與語意規則）**：
//!   - `SC1`～`SC4`：結構、前綴合法性、型別語法（禁止 `string:N[n]` 字串陣列）、未知欄位攔截与 JSON Schema 產生器。
//!   - `R1`、`R2`、`R5`、`R6`、`R8`、`R9`、`R17`～`R19`、`R21`、`R24`～`R28`：不需類型目錄之載入安全規則與 PLC 家族位址解析。
//!   - `U1`～`U19`：類型版本釘住（`type_version`）、單親無環樹、必綁檢查、`field.out` 唯一性、多重綁定語意相容、
//!     `man_*`／`par_*` 唯讀與安全點位護欄、編輯距離 1 拼寫提示、HMI 覆蓋率報告。
//! - **模型轉換器與下游接口（Resolver / Lowering / PlcDataService）**：
//!   - 推導每個 Tag 的 `role` 與 `write_candidate`（附推導理由），輸出 `ResolvedMachineModel`（有效 Tag 表、Unit 樹、HMI 清單、內容指紋）。
//!   - 實作 §7 降階（Lowering）匯出器：將巢狀 Unit 樹轉換為 IEC 61131-3 / Inovance ST 的 `<type>_ctrl_t` 結構與頂層 FB 實例綁定。
//!   - 實作 §9.3 `PlcDataService` 最終 `writable` 計算與 `[[tag_override]]` 四大護欄驗證。
//!
//! ## Cargo.toml 配置範例（符合 §8.5 依賴約束）
//! ```toml
//! [package]
//! name = "machine-model"
//! version = "0.3.0"
//! edition = "2021"
//!
//! [dependencies]
//! serde = { version = "1.0", features = ["derive"] }
//! serde_yaml_ng = "0.10"
//! jsonschema = { version = "0.58", default-features = false, optional = true }
//!
//! [features]
//! default = []
//! schema = ["dep:jsonschema"]
//! ```

#![forbid(unsafe_code)]
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

// ============================================================================
// 1. 診斷訊息與規則編號 (§8)
// ============================================================================

pub mod diagnostics {
    use super::*;

    /// 診斷嚴重度
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Severity {
        /// 錯誤（阻擋載入與轉換，CLI 退出碼 1）
        Error,
        /// 警告／提示（如 U14 明確 role 覆寫、U15 拼寫相近、U17 本地類型覆蓋標準類型）
        Hint,
        /// 資訊（如 U16 未綁定的選擇性 ctrl 腳 HMI 覆蓋率項目）
        Info,
    }

    impl fmt::Display for Severity {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Severity::Error => write!(f, "錯誤"),
                Severity::Hint => write!(f, "提示"),
                Severity::Info => write!(f, "資訊"),
            }
        }
    }

    /// 結構化診斷項目（訊息使用繁體中文並含規則編號）
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Diagnostic {
        pub rule: String,
        pub severity: Severity,
        pub path: String,
        pub message: String,
    }

    impl Diagnostic {
        pub fn error(rule: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
            Self {
                rule: rule.into(),
                severity: Severity::Error,
                path: path.into(),
                message: message.into(),
            }
        }

        pub fn hint(rule: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
            Self {
                rule: rule.into(),
                severity: Severity::Hint,
                path: path.into(),
                message: message.into(),
            }
        }

        pub fn info(rule: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
            Self {
                rule: rule.into(),
                severity: Severity::Info,
                path: path.into(),
                message: message.into(),
            }
        }

        pub fn is_error(&self) -> bool {
            self.severity == Severity::Error
        }
    }

    impl fmt::Display for Diagnostic {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "[{}][{}] {}: {}", self.rule, self.severity, self.path, self.message)
        }
    }

    pub fn has_errors(diags: &[Diagnostic]) -> bool {
        diags.iter().any(Diagnostic::is_error)
    }
}

// ============================================================================
// 2. PLC 家族與軟元件位址表 (§3.1, §3.2)
// ============================================================================

pub use plc_common::family as family_table;

pub mod family {
    use super::family_table::Radix;
    use super::family_table::Width;
    use super::*;

    /// PLC 家族（驅動型號），定義於 `family_table`（spec §3.3）。
    pub use super::family_table::{ClassProfile, Family as PlcFamily, Grammar, OPAQUE_CLASS};

    /// 軟元件位址方向與性質分類
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AddressCategory {
        /// 唯讀實體輸入（如 X）
        PhysicalInput,
        /// 實體輸出（如 Y）
        PhysicalOutput,
        /// 內部輔助繼電器／步進點（如 M, S, B）
        InternalBit,
        /// 資料暫存器（如 D, R, W）
        DataRegister,
        /// 計時器／計數器（如 T, C）
        TimerCounter,
        /// 結構化語法（如 `DB1.DBW0`、標籤名稱）；只檢查非空，類別未知
        Structured,
    }

    impl AddressCategory {
        /// 是否為硬體唯讀位址類別（如 X 輸入，用於 U4、U13、R19 與 §9.3 護欄）
        pub fn is_read_only(self) -> bool {
            matches!(self, AddressCategory::PhysicalInput)
        }

        /// 是否為實體輸出位址類別（如 Y 輸出，用於 U4）
        pub fn is_physical_output(self) -> bool {
            matches!(self, AddressCategory::PhysicalOutput)
        }

        /// 是否為位元（Bit）位址類別
        pub fn is_bit_device(self) -> bool {
            matches!(
                self,
                AddressCategory::PhysicalInput
                    | AddressCategory::PhysicalOutput
                    | AddressCategory::InternalBit
            )
        }
    }

    /// 解析後的軟元件位址
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ParsedAddress {
        pub prefix: String,
        pub index: u32,
        pub bit_offset: Option<u8>,
        pub category: AddressCategory,
        pub canonical: String,
    }

    /// 類別的位址分類：唯讀輸入、實體輸出（`Y`、`Q`）、其餘位元、字組。
    fn category_of(class: &ClassProfile) -> AddressCategory {
        if class.read_only {
            AddressCategory::PhysicalInput
        } else if class.width == Width::Bit && matches!(class.name, "Y" | "Q") {
            AddressCategory::PhysicalOutput
        } else if class.width == Width::Bit {
            AddressCategory::InternalBit
        } else {
            AddressCategory::DataRegister
        }
    }

    fn radix_name(radix: Radix) -> &'static str {
        match radix {
            Radix::Octal => "八",
            Radix::Decimal => "十",
            Radix::Hexadecimal => "十六",
        }
    }

    /// 依 PLC 家族解析並驗證軟元件位址 (R8)
    ///
    /// 前綴表家族：前綴必須在表中，編號依該類別的進位與上限檢查。
    /// 非空家族（`Grammar::Opaque`）：只檢查非空。
    pub fn parse_plc_address(family: PlcFamily, raw: &str) -> Result<ParsedAddress, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("軟元件位址不得為空".to_string());
        }

        let classes = match family.grammar() {
            Grammar::Opaque => {
                return Ok(ParsedAddress {
                    prefix: String::new(),
                    index: 0,
                    bit_offset: None,
                    category: AddressCategory::Structured,
                    canonical: trimmed.to_string(),
                });
            }
            Grammar::Classes(classes) => classes,
        };

        let alpha_len = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .count();
        if alpha_len == 0 {
            return Err(format!("位址 '{}' 缺少軟元件前綴（如 X、Y、M、D）", raw));
        }

        let prefix = trimmed[..alpha_len].to_ascii_uppercase();
        let rest = &trimmed[alpha_len..];
        if rest.is_empty() {
            return Err(format!("位址 '{}' 缺少編號", raw));
        }

        let class = classes
            .iter()
            .find(|c| c.name == prefix)
            .ok_or_else(|| format!("PLC 家族 '{}' 不支援軟元件前綴 '{}'", family, prefix))?;
        let base = class.radix.base();

        let (num_part, bit_offset) = if let Some((n, b)) = rest.split_once('.') {
            if !class.bit_suffix {
                return Err(format!("軟元件 '{}' 不支援字元內位元索引 ('.bit')", prefix));
            }
            let bit = b
                .parse::<u8>()
                .ok()
                .filter(|&v| v <= 15)
                .ok_or_else(|| format!("位址 '{}' 的位元偏移 '{}' 必須為 0..15", raw, b))?;
            (n, Some(bit))
        } else {
            (rest, None)
        };

        if num_part.is_empty() || !num_part.chars().all(|c| c.is_digit(base)) {
            return Err(format!(
                "位址 '{}' 編號 '{}' 非合法{}進位數字",
                raw,
                num_part,
                radix_name(class.radix)
            ));
        }
        let index = u32::from_str_radix(num_part, base).map_err(|_| {
            format!(
                "位址 '{}' 編號 '{}' 非合法{}進位數字",
                raw,
                num_part,
                radix_name(class.radix)
            )
        })?;

        if index > class.max {
            return Err(format!(
                "位址 '{}' 超出家族 '{}' 的範圍上限 (最大 {})",
                raw, family, class.max
            ));
        }

        let canonical = match (class.radix, bit_offset) {
            (Radix::Octal, None) => format!("{}{:03o}", prefix, index),
            (Radix::Octal, Some(b)) => format!("{}{:03o}.{}", prefix, index, b),
            (Radix::Hexadecimal, None) => format!("{}{:X}", prefix, index),
            (Radix::Hexadecimal, Some(b)) => format!("{}{:X}.{}", prefix, index, b),
            (Radix::Decimal, None) => format!("{}{}", prefix, index),
            (Radix::Decimal, Some(b)) => format!("{}{}.{}", prefix, index, b),
        };

        Ok(ParsedAddress {
            prefix,
            index,
            bit_offset,
            category: category_of(class),
            canonical,
        })
    }
}

// ============================================================================
// 3. 資料型別系統 DataType (§3.2, §4.1, §8.3 SC3)
// ============================================================================

pub mod datatype {
    use super::diagnostics::Diagnostic;
    use super::*;

    /// 標量型別詞彙 (§3.2)，定義於 `plc-common`（`string:N` 為 `Text(N)`，N 為 1..=256）
    pub use plc_common::ScalarType;

    /// 完整資料型別（標量或標量陣列，明確禁止 `string:N[n]`，§3.2）
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub enum DataType {
        Scalar(ScalarType),
        Array { elem: ScalarType, len: u16 },
    }

    impl DataType {
        /// 解析型別字串，違規時以 `SC3` 回報 (§3.2, §8.3)
        pub fn parse(raw: &str, path: &str) -> Result<Self, Diagnostic> {
            let s = raw.trim();
            if let Some(open_bracket) = s.find('[') {
                if !s.ends_with(']') {
                    return Err(Diagnostic::error(
                        "SC3",
                        path,
                        format!("型別 '{}' 陣列語法錯誤，缺少結尾 ']' ", raw),
                    ));
                }
                let base_str = &s[..open_bracket];
                let len_str = &s[open_bracket + 1..s.len() - 1];
                if base_str.starts_with("string:") {
                    return Err(Diagnostic::error(
                        "SC3",
                        path,
                        format!("禁止使用字串陣列型別 '{}'（規格 §3.2 明確禁止 string:N[n]）", raw),
                    ));
                }
                let elem = ScalarType::parse(base_str)
                    .map_err(|e| Diagnostic::error("SC3", path, e.to_string()))?;
                let len: u16 = len_str.parse().map_err(|_| {
                    Diagnostic::error("SC3", path, format!("陣列長度 '{}' 必須為 1～500 的整數", len_str))
                })?;
                if !(1..=500).contains(&len) {
                    return Err(Diagnostic::error(
                        "SC3",
                        path,
                        format!("型別 '{}' 的陣列長度 {} 超出允許範圍 1～500", raw, len),
                    ));
                }
                Ok(DataType::Array { elem, len })
            } else {
                let scalar = ScalarType::parse(s)
                    .map_err(|e| Diagnostic::error("SC3", path, e.to_string()))?;
                Ok(DataType::Scalar(scalar))
            }
        }

        pub fn is_bool(&self) -> bool {
            matches!(self, DataType::Scalar(ScalarType::Bool))
        }

        pub fn is_string_scalar(&self) -> bool {
            matches!(self, DataType::Scalar(ScalarType::Text(_)))
        }
    }

    impl fmt::Display for DataType {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                DataType::Scalar(s) => write!(f, "{}", s),
                DataType::Array { elem, len } => write!(f, "{}[{}]", elem, len),
            }
        }
    }
}

// ============================================================================
// 4. 命名規則（前綴表 §5 與已知功能名詞典 §6.4）
// ============================================================================

pub mod naming {
    use super::datatype::{DataType, ScalarType};
    use super::*;

    /// 接腳方向 (§5)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PinDirection {
        In,
        Out,
    }

    /// Tag 角色列舉 (§5, §6.1, §11 #9 `par` 沿用 `setpoint`)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Role {
        Command,
        Setpoint,
        Status,
        Alarm,
        Counter,
        ProcessValue,
        Message,
    }

    impl fmt::Display for Role {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let s = match self {
                Role::Command => "command",
                Role::Setpoint => "setpoint",
                Role::Status => "status",
                Role::Alarm => "alarm",
                Role::Counter => "counter",
                Role::ProcessValue => "process_value",
                Role::Message => "message",
            };
            write!(f, "{}", s)
        }
    }

    /// 接腳綁定策略 (§5)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum BindingPolicy {
        /// 純接線，不需 Tag（`cmd_`、`ena_`）
        PureWiring,
        /// 選擇性綁定（`man_`、`par_`、`at_`、`sts_`、`count_`、`pv_`、`msg_`）
        Optional,
        /// 必綁（`error_`）
        Required,
    }

    /// 封閉的前綴類別 (§5)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum CtrlPrefix {
        Cmd,
        Man,
        Ena,
        Par,
        At,
        Sts,
        Error,
        Count,
        Pv,
        Msg,
    }

    impl CtrlPrefix {
        pub fn as_str(self) -> &'static str {
            match self {
                CtrlPrefix::Cmd => "cmd",
                CtrlPrefix::Man => "man",
                CtrlPrefix::Ena => "ena",
                CtrlPrefix::Par => "par",
                CtrlPrefix::At => "at",
                CtrlPrefix::Sts => "sts",
                CtrlPrefix::Error => "error",
                CtrlPrefix::Count => "count",
                CtrlPrefix::Pv => "pv",
                CtrlPrefix::Msg => "msg",
            }
        }
    }

    impl fmt::Display for CtrlPrefix {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.as_str())
        }
    }

    /// 前綴表單筆規格 (§5)
    #[derive(Debug, Clone)]
    pub struct PrefixSpec {
        pub prefix: CtrlPrefix,
        pub direction: PinDirection,
        pub inferred_role: Role,
        pub write_candidate: bool,
        pub binding_policy: BindingPolicy,
        pub default_type: Option<ScalarType>,
        pub required_attrs: &'static [&'static str],
    }

    /// 唯一事實來源：標準前綴表 (`standards/naming/prefixes.yaml` 對應定義，版本 1)
    pub const PREFIX_TABLE_VERSION: u32 = 1;

    pub const PREFIX_TABLE: &[PrefixSpec] = &[
        PrefixSpec {
            prefix: CtrlPrefix::Cmd,
            direction: PinDirection::In,
            inferred_role: Role::Command,
            write_candidate: false,
            binding_policy: BindingPolicy::PureWiring,
            default_type: Some(ScalarType::Bool),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Man,
            direction: PinDirection::In,
            inferred_role: Role::Command,
            write_candidate: true,
            binding_policy: BindingPolicy::Optional,
            default_type: Some(ScalarType::Bool),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Ena,
            direction: PinDirection::In,
            inferred_role: Role::Command,
            write_candidate: false,
            binding_policy: BindingPolicy::PureWiring,
            default_type: Some(ScalarType::Bool),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Par,
            direction: PinDirection::In,
            inferred_role: Role::Setpoint,
            write_candidate: true,
            binding_policy: BindingPolicy::Optional,
            default_type: None,
            required_attrs: &["type", "min", "max", "default"],
        },
        PrefixSpec {
            prefix: CtrlPrefix::At,
            direction: PinDirection::Out,
            inferred_role: Role::Status,
            write_candidate: false,
            binding_policy: BindingPolicy::Optional,
            default_type: Some(ScalarType::Bool),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Sts,
            direction: PinDirection::Out,
            inferred_role: Role::Status,
            write_candidate: false,
            binding_policy: BindingPolicy::Optional,
            default_type: Some(ScalarType::Bool),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Error,
            direction: PinDirection::Out,
            inferred_role: Role::Alarm,
            write_candidate: false,
            binding_policy: BindingPolicy::Required,
            default_type: Some(ScalarType::Bool),
            required_attrs: &["message"],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Count,
            direction: PinDirection::Out,
            inferred_role: Role::Counter,
            write_candidate: false,
            binding_policy: BindingPolicy::Optional,
            default_type: Some(ScalarType::Int32),
            required_attrs: &[],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Pv,
            direction: PinDirection::Out,
            inferred_role: Role::ProcessValue,
            write_candidate: false,
            binding_policy: BindingPolicy::Optional,
            default_type: None,
            required_attrs: &["type"],
        },
        PrefixSpec {
            prefix: CtrlPrefix::Msg,
            direction: PinDirection::Out,
            inferred_role: Role::Message,
            write_candidate: false,
            binding_policy: BindingPolicy::Optional,
            default_type: None,
            required_attrs: &["type"],
        },
    ];

    /// 解析後之控制接腳名稱 (`<前綴>_<功能>[_<細節>]`)
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ParsedCtrlPinName {
        pub prefix: CtrlPrefix,
        /// 完整後綴（例如 `par_ext_time` 的 `ext_time`、`man_ext` 的 `ext`）
        pub full_suffix: String,
        /// 主功能名稱（例如 `ext`、`ret`、`mode`、`err`）
        pub feature: String,
        /// 選填細節（例如 `time`）
        pub detail: Option<String>,
    }

    pub fn lookup_prefix_spec(prefix: CtrlPrefix) -> &'static PrefixSpec {
        PREFIX_TABLE
            .iter()
            .find(|s| s.prefix == prefix)
            .expect("所有 CtrlPrefix 皆存在於 PREFIX_TABLE")
    }

    pub fn starts_with_reserved_ctrl_prefix(pin_name: &str) -> Option<&'static PrefixSpec> {
        for spec in PREFIX_TABLE {
            let pat = format!("{}_", spec.prefix.as_str());
            if pin_name.starts_with(&pat) {
                return Some(spec);
            }
        }
        None
    }

    /// 解析 `ctrl` 接腳名稱 (§4.1, §5)
    pub fn parse_ctrl_pin_name(pin_name: &str) -> Result<(ParsedCtrlPinName, &'static PrefixSpec), String> {
        let Some((prefix_str, suffix)) = pin_name.split_once('_') else {
            return Err(format!(
                "控制接腳 '{}' 必須符合 '<前綴>_<功能>[_<細節>]' 格式",
                pin_name
            ));
        };
        let Some(spec) = PREFIX_TABLE.iter().find(|s| s.prefix.as_str() == prefix_str) else {
            let valid: Vec<&str> = PREFIX_TABLE.iter().map(|s| s.prefix.as_str()).collect();
            return Err(format!(
                "控制接腳 '{}' 的前綴 '{}' 不在標準前綴表中（合法前綴：{}）",
                pin_name,
                prefix_str,
                valid.join(", ")
            ));
        };
        if suffix.is_empty() || !is_valid_identifier(suffix) {
            return Err(format!(
                "控制接腳 '{}' 的功能後綴 '{}' 格式不合法",
                pin_name, suffix
            ));
        }
        let (feature, detail) = match suffix.split_once('_') {
            Some((f, d)) => (f.to_string(), Some(d.to_string())),
            None => (suffix.to_string(), None),
        };
        Ok((
            ParsedCtrlPinName {
                prefix: spec.prefix,
                full_suffix: suffix.to_string(),
                feature,
                detail,
            },
            spec,
        ))
    }

    /// 已知功能名詞典 (§6.4)，未指定 `label` 時自動推導中文顯示名
    pub fn default_feature_label(full_suffix: &str, feature: &str) -> Option<String> {
        let exact = match full_suffix {
            "ext" => Some("伸出"),
            "ret" => Some("縮回"),
            "start" => Some("啟動"),
            "stop" => Some("停止"),
            "reset" => Some("復歸"),
            "home" => Some("原點復歸"),
            "mode" => Some("模式"),
            "clamp" => Some("夾緊"),
            "unclamp" => Some("鬆開"),
            "up" => Some("上升"),
            "down" => Some("下降"),
            "fwd" => Some("前進"),
            "rev" => Some("後退"),
            "vac" => Some("真空吸著"),
            "blow" => Some("破真空"),
            "load" => Some("上料"),
            "unload" => Some("下料"),
            "auto" => Some("自動運轉"),
            "ready" => Some("準備完成"),
            "busy" => Some("動作中"),
            "ext_time" => Some("伸出設定時間"),
            "ret_time" => Some("縮回設定時間"),
            "err_time" => Some("警報逾時時間"),
            "cycle" => Some("循環計數"),
            _ => None,
        };
        if let Some(label) = exact {
            return Some(label.to_string());
        }
        match feature {
            "ext" => Some("伸出".to_string()),
            "ret" => Some("縮回".to_string()),
            "start" => Some("啟動".to_string()),
            "stop" => Some("停止".to_string()),
            "reset" => Some("復歸".to_string()),
            "home" => Some("原點復歸".to_string()),
            _ => None,
        }
    }

    pub fn is_valid_identifier(s: &str) -> bool {
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return false;
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// 決定 `ctrl` 接腳的有效型別 (§5)
    pub fn resolve_ctrl_pin_type(
        pin_name: &str,
        explicit_type: Option<&str>,
        spec: &PrefixSpec,
        path: &str,
    ) -> Result<DataType, super::diagnostics::Diagnostic> {
        match explicit_type {
            Some(raw) => DataType::parse(raw, path),
            None => match &spec.default_type {
                Some(default_scalar) => Ok(DataType::Scalar(default_scalar.clone())),
                None => Err(super::diagnostics::Diagnostic::error(
                    "SC3",
                    path,
                    format!("控制接腳 '{}' (前綴 '{}') 必須明確指定 'type' 屬性", pin_name, spec.prefix),
                )),
            },
        }
    }
}

// ============================================================================
// 5. Unit 類型檔與類型目錄 (§4, §9.1) + JSON Schema 產生器 (§4.2)
// ============================================================================

pub mod unit_type {
    use super::diagnostics::Diagnostic;
    use super::naming::{PREFIX_TABLE, starts_with_reserved_ctrl_prefix};
    use super::*;

    /// `field.in` / `field.out` 接腳定義 (§4.1)
    /// 僅允許 `type`、`required`、`label`、`unit`；未知欄位（含 `kind`、`level`、`reset`、`role`）由 serde 拒絕 (SC4)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct FieldPinDef {
        #[serde(rename = "type")]
        pub data_type: String,
        #[serde(default = "default_true")]
        pub required: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
    }

    fn default_true() -> bool {
        true
    }

    /// `field` 區塊 (§4.1)
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct FieldSection {
        #[serde(rename = "in", default)]
        pub inputs: BTreeMap<String, FieldPinDef>,
        #[serde(rename = "out", default)]
        pub outputs: BTreeMap<String, FieldPinDef>,
    }

    /// `ctrl` 接腳定義 (§4.1)
    /// 僅允許 `type`、`label`、`unit`、`min`、`max`、`default`、`message`；
    /// 舊版 `kind`、`level`、`reset`、`role` 已移除，出現即觸發 SC4 未知欄位錯誤。
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CtrlPinDef {
        #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
        pub data_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub message: Option<String>,
    }

    /// Unit 類型檔 (`standards/unit-types/<id>.yaml` 或 `types/<id>.yaml`, §4)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UnitTypeDef {
        pub id: String,
        pub version: u32,
        pub title: String,
        #[serde(default)]
        pub field: FieldSection,
        #[serde(default)]
        pub ctrl: BTreeMap<String, CtrlPinDef>,
        #[serde(default)]
        pub parts: BTreeMap<String, String>,
    }

    impl UnitTypeDef {
        pub fn from_yaml_str(yaml: &str, source_path: &str) -> Result<Self, Diagnostic> {
            serde_yaml_ng::from_str::<UnitTypeDef>(yaml).map_err(|e| {
                let msg = e.to_string();
                let rule = if msg.contains("unknown field") { "SC4" } else { "SC2" };
                Diagnostic::error(rule, source_path, format!("Unit 類型檔結構或欄位錯誤：{}", msg))
            })
        }
    }

    /// 類型來源標記 (§9.1)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum TypeOrigin {
        /// 編譯期內嵌之標準資產 (`standards/unit-types/*.yaml`)
        Standard,
        /// 機台資料夾下自動讀取之本地類型 (`types/*.yaml`)
        MachineLocal,
    }

    /// 類型目錄單筆條目（含內容雜湊以釘住版本，§9.1）
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct CatalogEntry {
        pub def: UnitTypeDef,
        pub origin: TypeOrigin,
        pub content_hash: String,
        pub source_label: String,
    }

    /// 內嵌標準 Unit 類型資產 (§9.1 `include_str!` 等效常數與雜湊釘住)
    pub const STD_CYLINDER_DOUBLE_YAML: &str = r#"id: cylinder_double
version: 1
title: 雙作用氣缸
field:
  in:
    lim_ext: { type: bool }
    lim_ret: { type: bool, required: false }
  out:
    valve_ext: { type: bool }
    valve_ret: { type: bool, required: false }
ctrl:
  cmd_ext:      { type: bool }
  cmd_ret:      { type: bool }
  ena_ext:      { type: bool }
  man_ext:      { type: bool, label: 伸出 }
  man_ret:      { type: bool, label: 縮回 }
  at_ext:       { type: bool }
  at_ret:       { type: bool }
  par_ext_time: { type: int16, unit: ms, min: 0, max: 30000, default: 3000 }
  par_err_time: { type: int16, unit: ms, min: 0, max: 30000, default: 5000 }
  error_ext:    { message: 伸出逾時 }
  error_ret:    { message: 縮回逾時 }
parts: {}
"#;

    pub const STD_STATION_LOAD_YAML: &str = r#"id: station_load
version: 1
title: 上料工站
field:
  in: {}
  out: {}
ctrl:
  cmd_start:    { type: bool }
  man_start:    { type: bool, label: 啟動 }
  sts_busy:     { type: bool }
  count_total:  { label: 上料計數 }
  error_jam:    { message: 上料卡料異常 }
parts:
  cyl: cylinder_double
"#;

    pub const STD_MACHINE_CTRL_YAML: &str = r#"id: machine_ctrl
version: 1
title: 機台主控最小框架
field:
  in: {}
  out: {}
ctrl:
  man_mode:     { type: int16, label: 模式切換 }
  at_mode:      { type: int16, label: 當前模式 }
  man_reset:    { type: bool, label: 警報復歸 }
  sts_auto_run: { type: bool, label: 自動運轉中 }
  error_estop:  { message: 緊急停止觸發 }
parts:
  stn: station_load
"#;

    /// 確定性 FNV-1a 64-bit 雜湊（不需外部加密 crate，用於 §9.1 類型檔與來源雜湊指紋）
    pub fn fnv1a_64_hex(content: &str) -> String {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in content.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        format!("{:016x}", hash)
    }

    /// 類型目錄 (§9.1)
    #[derive(Debug, Clone, Default)]
    pub struct TypeCatalog {
        pub entries: BTreeMap<String, CatalogEntry>,
        pub load_diagnostics: Vec<Diagnostic>,
    }

    impl TypeCatalog {
        /// 建立內嵌標準類型目錄 (§9.1)
        pub fn with_embedded_standards() -> Self {
            let mut catalog = Self::default();
            let standards = [
                ("standards/unit-types/cylinder_double.yaml", STD_CYLINDER_DOUBLE_YAML),
                ("standards/unit-types/station_load.yaml", STD_STATION_LOAD_YAML),
                ("standards/unit-types/machine_ctrl.yaml", STD_MACHINE_CTRL_YAML),
            ];
            for (label, yaml) in standards {
                let def = UnitTypeDef::from_yaml_str(yaml, label)
                    .expect("內嵌標準 Unit 類型檔必須通過結構解析");
                catalog.entries.insert(
                    def.id.clone(),
                    CatalogEntry {
                        def,
                        origin: TypeOrigin::Standard,
                        content_hash: fnv1a_64_hex(yaml),
                        source_label: label.to_string(),
                    },
                );
            }
            catalog
        }

        /// 註冊機台本地類型；同來源重複 id 報錯 (U17)，覆蓋標準類型產生提示 (U17, §9.1)
        pub fn register_local_yaml(
            &mut self,
            yaml: &str,
            source_label: &str,
            seen_local_ids: &mut BTreeSet<String>,
        ) {
            match UnitTypeDef::from_yaml_str(yaml, source_label) {
                Ok(def) => {
                    if !seen_local_ids.insert(def.id.clone()) {
                        self.load_diagnostics.push(Diagnostic::error(
                            "U17",
                            source_label,
                            format!("機台本地類型資料夾內出現重複的類型 id '{}'", def.id),
                        ));
                        return;
                    }
                    if let Some(existing) = self.entries.get(&def.id) {
                        if existing.origin == TypeOrigin::Standard {
                            self.load_diagnostics.push(Diagnostic::hint(
                                "U17",
                                source_label,
                                format!(
                                    "機台本地類型 '{}' (v{}) 覆蓋了標準類型 '{}' (v{})",
                                    def.id, def.version, existing.def.id, existing.def.version
                                ),
                            ));
                        }
                    }
                    self.entries.insert(
                        def.id.clone(),
                        CatalogEntry {
                            def,
                            origin: TypeOrigin::MachineLocal,
                            content_hash: fnv1a_64_hex(yaml),
                            source_label: source_label.to_string(),
                        },
                    );
                }
                Err(diag) => self.load_diagnostics.push(diag),
            }
        }

        /// 自動讀取 `<machine_dir>/types/*.yaml` (§9.1)
        pub fn load_local_types_from_dir(&mut self, machine_dir: &Path) {
            let types_dir = machine_dir.join("types");
            if !types_dir.is_dir() {
                return;
            }
            let Ok(read_dir) = fs::read_dir(&types_dir) else {
                return;
            };
            let mut paths: Vec<_> = read_dir
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
                        .unwrap_or(false)
                })
                .collect();
            paths.sort();

            let mut seen_local_ids = BTreeSet::new();
            for path in paths {
                let label = path.display().to_string();
                match fs::read_to_string(&path) {
                    Ok(content) => self.register_local_yaml(&content, &label, &mut seen_local_ids),
                    Err(err) => self.load_diagnostics.push(Diagnostic::error(
                        "SC2",
                        &label,
                        format!("無法讀取本地類型檔：{}", err),
                    )),
                }
            }
        }

        pub fn get(&self, type_id: &str) -> Option<&CatalogEntry> {
            self.entries.get(type_id)
        }
    }

    /// 由前綴表 (§5) 機械產生 `standards/schema/unit-type.schema.json` (§4.2)
    pub fn generate_unit_type_json_schema() -> String {
        let prefixes: Vec<&str> = PREFIX_TABLE.iter().map(|p| p.prefix.as_str()).collect();
        let forbidden_field_pattern = format!("^({})_", prefixes.join("|"));

        let mut pattern_props = Vec::new();
        for spec in PREFIX_TABLE {
            let pat = format!("^{}_[a-zA-Z0-9_]+$", spec.prefix.as_str());
            let req_list: Vec<String> = spec
                .required_attrs
                .iter()
                .map(|r| format!("\"{}\"", r))
                .collect();
            let allowed_props = match spec.prefix {
                super::naming::CtrlPrefix::Par => {
                    r#""type": { "type": "string" }, "label": { "type": "string" }, "unit": { "type": "string" }, "min": { "type": "number" }, "max": { "type": "number" }, "default": { "type": "number" }"#
                }
                super::naming::CtrlPrefix::Error => {
                    r#""type": { "type": "string", "enum": ["bool"] }, "label": { "type": "string" }, "unit": { "type": "string" }, "message": { "type": "string", "minLength": 1 }"#
                }
                _ => {
                    r#""type": { "type": "string" }, "label": { "type": "string" }, "unit": { "type": "string" }"#
                }
            };
            pattern_props.push(format!(
                r#"        "{pat}": {{
          "type": "object",
          "additionalProperties": false,
          "required": [{req}],
          "properties": {{ {allowed_props} }}
        }}"#,
                pat = pat,
                req = req_list.join(", "),
                allowed_props = allowed_props
            ));
        }

        format!(
            r#"{{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "UnitTypeDef",
  "description": "Generated from standards/naming/prefixes.yaml (v{ver}) — DO NOT EDIT MANUALLY",
  "type": "object",
  "additionalProperties": false,
  "required": ["id", "version", "title"],
  "properties": {{
    "id": {{ "type": "string", "pattern": "^[a-zA-Z_][a-zA-Z0-9_]*$" }},
    "version": {{ "type": "integer", "minimum": 1 }},
    "title": {{ "type": "string", "minLength": 1 }},
    "field": {{
      "type": "object",
      "additionalProperties": false,
      "properties": {{
        "in": {{
          "type": "object",
          "propertyNames": {{ "not": {{ "pattern": "{forbidden}" }} }},
          "additionalProperties": {{
            "type": "object",
            "additionalProperties": false,
            "required": ["type"],
            "properties": {{
              "type": {{ "type": "string" }},
              "required": {{ "type": "boolean", "default": true }},
              "label": {{ "type": "string" }},
              "unit": {{ "type": "string" }}
            }}
          }}
        }},
        "out": {{
          "type": "object",
          "propertyNames": {{ "not": {{ "pattern": "{forbidden}" }} }},
          "additionalProperties": {{
            "type": "object",
            "additionalProperties": false,
            "required": ["type"],
            "properties": {{
              "type": {{ "type": "string" }},
              "required": {{ "type": "boolean", "default": true }},
              "label": {{ "type": "string" }},
              "unit": {{ "type": "string" }}
            }}
          }}
        }}
      }}
    }},
    "ctrl": {{
      "type": "object",
      "additionalProperties": false,
      "patternProperties": {{
{pattern_props}
      }}
    }},
    "parts": {{
      "type": "object",
      "additionalProperties": {{ "type": "string", "pattern": "^[a-zA-Z_][a-zA-Z0-9_]*$" }}
    }}
  }}
}}"#,
            ver = super::naming::PREFIX_TABLE_VERSION,
            forbidden = forbidden_field_pattern,
            pattern_props = pattern_props.join(",\n")
        )
    }

    pub fn verify_field_pin_not_ctrl_prefixed(pin_name: &str) -> bool {
        starts_with_reserved_ctrl_prefix(pin_name).is_none()
    }
}

// ============================================================================
// 6. `machine.yaml` v2 模型結構與 Builder 建立器 (§3)
// ============================================================================

pub mod model {
    use super::diagnostics::Diagnostic;
    use super::family::PlcFamily;
    use super::naming::Role;
    use super::unit_type::{CtrlPinDef, FieldPinDef, FieldSection, UnitTypeDef};
    use super::*;

    /// 掃描週期列舉 (§3.2, R24)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ScanRate {
        Fast,
        Normal,
        Slow,
    }

    impl Default for ScanRate {
        fn default() -> Self {
            ScanRate::Normal
        }
    }

    /// 接點極性列舉 (§3.2, R19：僅能用於 bool 的唯讀輸入位址類別)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum Polarity {
        NO,
        NC,
    }

    /// 標量預設值／範圍值，定義於 `plc-common`
    pub use plc_common::ScalarValue;

    /// PLC 宣告 (§3.1)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PlcDef {
        pub id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        pub family: PlcFamily,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub online: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub diff_scope: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub sources: Vec<String>,
    }

    /// `machine` 標頭 (§3.1)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MachineHeader {
        pub id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub title: Option<String>,
        pub plcs: Vec<PlcDef>,
    }

    /// 無位址的 PLC 變數 (§3.1 `variables[]`：移除 `unit_id`，不得有 `address`, R25)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct VariableDef {
        pub id: String,
        #[serde(rename = "type")]
        pub data_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub symbol: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// 若誤填 address，於 R25 捕捉並報錯
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub address: Option<String>,
    }

    /// Tag 欄位定義 (§3.2)
    /// 嚴格禁止 `unit_id`、`writable`、`write_permission`（由 `deny_unknown_fields` 攔截為 SC4 / R22）
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TagDef {
        pub id: String,
        pub address: String,
        #[serde(rename = "type")]
        pub data_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub symbol: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        #[serde(default)]
        pub scan: ScanRate,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub scale: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub offset: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub deadband: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
        #[serde(default)]
        pub readback: bool,
        #[serde(default)]
        pub retain: bool,
        #[serde(default)]
        pub spare: bool,
        #[serde(default)]
        pub safety_monitor: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub min: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default: Option<ScalarValue>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub role: Option<Role>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub polarity: Option<Polarity>,
        /// 廠牌擴充 `ext.<廠牌>.<欄位>` (§3.2, R21)
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub ext: BTreeMap<String, BTreeMap<String, ScalarValue>>,
    }

    /// Unit 實例定義 (§3.1 `units[]`)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UnitInstanceDef {
        pub id: String,
        #[serde(rename = "type")]
        pub unit_type: String,
        /// 必填正整數，必須等於目錄中該類型的 `version` (§3.1, §9.1, U2)
        pub type_version: u32,
        /// 接腳或 part 綁定（值僅能為字串 id 或 `null`，SC1 / R28）
        #[serde(default)]
        pub bind: BTreeMap<String, Option<String>>,
    }

    /// `machine.yaml` v2 根結構 (§3.1)
    /// 舊版 `devices`、`alarms`、`parameters` 由 `deny_unknown_fields` 攔截 (SC1 / SC4)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MachineFile {
        pub schema_version: u32,
        pub machine: MachineHeader,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub variables: Vec<VariableDef>,
        #[serde(default)]
        pub tags: Vec<TagDef>,
        #[serde(default)]
        pub units: Vec<UnitInstanceDef>,
    }

    impl MachineFile {
        pub fn from_yaml_str(yaml: &str) -> Result<Self, Diagnostic> {
            serde_yaml_ng::from_str::<MachineFile>(yaml).map_err(|e| {
                let msg = e.to_string();
                let rule = if msg.contains("unknown field") { "SC4" } else { "SC1" };
                Diagnostic::error(rule, "machine.yaml", format!("machine.yaml 結構或未知欄位錯誤：{}", msg))
            })
        }

        pub fn to_yaml_string(&self) -> Result<String, String> {
            serde_yaml_ng::to_string(self).map_err(|e| e.to_string())
        }
    }

    // ------------------------------------------------------------------------
    // Fluent Builder API — 供工程師或程式化工具建立 MachineModel 與 UnitType
    // ------------------------------------------------------------------------

    pub struct MachineModelBuilder {
        file: MachineFile,
    }

    impl MachineModelBuilder {
        pub fn new(machine_id: impl Into<String>) -> Self {
            Self {
                file: MachineFile {
                    schema_version: 2,
                    machine: MachineHeader {
                        id: machine_id.into(),
                        title: None,
                        plcs: Vec::new(),
                    },
                    variables: Vec::new(),
                    tags: Vec::new(),
                    units: Vec::new(),
                },
            }
        }

        pub fn title(mut self, title: impl Into<String>) -> Self {
            self.file.machine.title = Some(title.into());
            self
        }

        pub fn add_plc(mut self, id: impl Into<String>, family: PlcFamily) -> Self {
            self.file.machine.plcs.push(PlcDef {
                id: id.into(),
                title: None,
                family,
                online: None,
                diff_scope: None,
                sources: Vec::new(),
            });
            self
        }

        pub fn add_tag(mut self, tag: TagDef) -> Self {
            self.file.tags.push(tag);
            self
        }

        pub fn add_simple_tag(
            mut self,
            id: impl Into<String>,
            address: impl Into<String>,
            data_type: impl Into<String>,
        ) -> Self {
            self.file.tags.push(TagDef {
                id: id.into(),
                address: address.into(),
                data_type: data_type.into(),
                symbol: None,
                label: None,
                description: None,
                scan: ScanRate::Normal,
                scale: None,
                offset: None,
                deadband: None,
                unit: None,
                readback: false,
                retain: false,
                spare: false,
                safety_monitor: false,
                min: None,
                max: None,
                default: None,
                role: None,
                polarity: None,
                ext: BTreeMap::new(),
            });
            self
        }

        pub fn add_unit(mut self, unit: UnitInstanceDef) -> Self {
            self.file.units.push(unit);
            self
        }

        pub fn build(self) -> MachineFile {
            self.file
        }
    }

    pub struct UnitInstanceBuilder {
        unit: UnitInstanceDef,
    }

    impl UnitInstanceBuilder {
        pub fn new(id: impl Into<String>, unit_type: impl Into<String>, type_version: u32) -> Self {
            Self {
                unit: UnitInstanceDef {
                    id: id.into(),
                    unit_type: unit_type.into(),
                    type_version,
                    bind: BTreeMap::new(),
                },
            }
        }

        pub fn bind_target(mut self, pin_or_part: impl Into<String>, target_id: impl Into<String>) -> Self {
            self.unit.bind.insert(pin_or_part.into(), Some(target_id.into()));
            self
        }

        pub fn bind_null(mut self, pin_or_part: impl Into<String>) -> Self {
            self.unit.bind.insert(pin_or_part.into(), None);
            self
        }

        pub fn build(self) -> UnitInstanceDef {
            self.unit
        }
    }

    pub struct UnitTypeBuilder {
        def: UnitTypeDef,
    }

    impl UnitTypeBuilder {
        pub fn new(id: impl Into<String>, version: u32, title: impl Into<String>) -> Self {
            Self {
                def: UnitTypeDef {
                    id: id.into(),
                    version,
                    title: title.into(),
                    field: FieldSection::default(),
                    ctrl: BTreeMap::new(),
                    parts: BTreeMap::new(),
                },
            }
        }

        pub fn field_in(mut self, name: impl Into<String>, data_type: impl Into<String>, required: bool) -> Self {
            self.def.field.inputs.insert(
                name.into(),
                FieldPinDef {
                    data_type: data_type.into(),
                    required,
                    label: None,
                    unit: None,
                },
            );
            self
        }

        pub fn field_out(mut self, name: impl Into<String>, data_type: impl Into<String>, required: bool) -> Self {
            self.def.field.outputs.insert(
                name.into(),
                FieldPinDef {
                    data_type: data_type.into(),
                    required,
                    label: None,
                    unit: None,
                },
            );
            self
        }

        pub fn ctrl_pin(mut self, name: impl Into<String>, pin: CtrlPinDef) -> Self {
            self.def.ctrl.insert(name.into(), pin);
            self
        }

        pub fn part(mut self, part_name: impl Into<String>, unit_type_id: impl Into<String>) -> Self {
            self.def.parts.insert(part_name.into(), unit_type_id.into());
            self
        }

        pub fn build(self) -> UnitTypeDef {
            self.def
        }
    }
}

// ============================================================================
// 7. 驗證器：載入安全規則 (R1~R28)、類型檔規則 (SC2/SC3)、U 規則 (U1~U19) (§8)
// ============================================================================

pub mod validator {
    use super::datatype::{DataType, ScalarType};
    use super::diagnostics::Diagnostic;
    use super::family::{PlcFamily, parse_plc_address};
    use super::model::MachineFile;
    use super::naming::{
        BindingPolicy, CtrlPrefix, is_valid_identifier, parse_ctrl_pin_name,
        resolve_ctrl_pin_type, starts_with_reserved_ctrl_prefix,
    };
    use super::unit_type::{TypeCatalog, UnitTypeDef};
    use super::*;

    /// 計算兩個字串的 Levenshtein 編輯距離（用於 U15 功能名稱拼寫相近檢查）
    pub fn edit_distance(a: &str, b: &str) -> usize {
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let m = a_chars.len();
        let n = b_chars.len();
        let mut dp = vec![vec![0usize; n + 1]; m + 1];
        for (i, row) in dp.iter_mut().enumerate().take(m + 1) {
            row[0] = i;
        }
        for j in 0..=n {
            dp[0][j] = j;
        }
        for i in 1..=m {
            for j in 1..=n {
                let cost = usize::from(a_chars[i - 1] != b_chars[j - 1]);
                dp[i][j] = (dp[i - 1][j] + 1)
                    .min(dp[i][j - 1] + 1)
                    .min(dp[i - 1][j - 1] + cost);
            }
        }
        dp[m][n]
    }

    /// 第一階段：載入安全規則（不需類型目錄，§8.3 / §8.4）
    /// 涵蓋 R1, R2, R5, R6, R8, R9, R17~R19, R21, R24~R28 與 SC3 型別檢查
    pub fn validate_load_safety(machine: &MachineFile) -> Vec<Diagnostic> {
        let mut diags = Vec::new();

        // R27: schema_version 必須為 2
        if machine.schema_version != 2 {
            diags.push(Diagnostic::error(
                "R27",
                "schema_version",
                format!("schema_version 必須為 2（目前為 {}），本專案不支援 v1", machine.schema_version),
            ));
        }

        // R1 & R2: machine.id 與 plcs[].id 合法且唯一
        if !is_valid_identifier(&machine.machine.id) {
            diags.push(Diagnostic::error(
                "R1",
                "machine.id",
                format!("機台 id '{}' 格式不合法", machine.machine.id),
            ));
        }
        if machine.machine.plcs.is_empty() {
            diags.push(Diagnostic::error(
                "R1",
                "machine.plcs",
                "machine.plcs 至少需宣告一台 PLC",
            ));
        }

        let mut plc_families: BTreeMap<String, PlcFamily> = BTreeMap::new();
        for (idx, plc) in machine.machine.plcs.iter().enumerate() {
            let path = format!("machine.plcs[{}]", idx);
            if !is_valid_identifier(&plc.id) {
                diags.push(Diagnostic::error(
                    "R1",
                    &path,
                    format!("PLC id '{}' 格式不合法", plc.id),
                ));
            }
            if plc_families.insert(plc.id.clone(), plc.family).is_some() {
                diags.push(Diagnostic::error(
                    "R2",
                    &path,
                    format!("重複的 PLC id '{}'", plc.id),
                ));
            }
        }

        // R5 & R25: variables 與 tags 共用命名空間唯一性；variables 不得有 address
        let mut seen_ids = BTreeSet::new();
        for (idx, var) in machine.variables.iter().enumerate() {
            let path = format!("variables[{}]({})", idx, var.id);
            if !seen_ids.insert(var.id.clone()) {
                diags.push(Diagnostic::error(
                    "R5",
                    &path,
                    format!("變數 id '{}' 重複（tags 與 variables 共用命名空間）", var.id),
                ));
            }
            if var.address.is_some() {
                diags.push(Diagnostic::error(
                    "R25",
                    &path,
                    format!("變數 '{}' 不得宣告 address 欄位（有位址請放 tags[]）", var.id),
                ));
            }
            if let Err(d) = DataType::parse(&var.data_type, &format!("{}.type", path)) {
                diags.push(d);
            }
        }

        let mut seen_plc_addresses: BTreeMap<(String, String), String> = BTreeMap::new();

        for (idx, tag) in machine.tags.iter().enumerate() {
            let path = format!("tags[{}]({})", idx, tag.id);

            // R5: 全機唯一
            if !seen_ids.insert(tag.id.clone()) {
                diags.push(Diagnostic::error(
                    "R5",
                    &path,
                    format!("Tag id '{}' 重複（全機唯一）", tag.id),
                ));
            }

            // R6: 格式 `plc<N>.<Name>`，PLC 必須已宣告
            let parsed_plc = match tag.id.split_once('.') {
                Some((plc_id, tag_name)) if is_valid_identifier(plc_id) && is_valid_identifier(tag_name) => {
                    if let Some(&family) = plc_families.get(plc_id) {
                        Some((plc_id.to_string(), family))
                    } else {
                        diags.push(Diagnostic::error(
                            "R6",
                            &path,
                            format!("Tag '{}' 引用的 PLC '{}' 未在 machine.plcs 中宣告", tag.id, plc_id),
                        ));
                        None
                    }
                }
                _ => {
                    diags.push(Diagnostic::error(
                        "R6",
                        &path,
                        format!("Tag id '{}' 必須符合 '<plc_id>.<TagName>' 格式", tag.id),
                    ));
                    None
                }
            };

            // SC3: Tag 型別解析（含禁止 string:N[n]）
            let parsed_type = match DataType::parse(&tag.data_type, &format!("{}.type", path)) {
                Ok(dt) => Some(dt),
                Err(d) => {
                    diags.push(d);
                    None
                }
            };

            // R8 & R9: 依 PLC 家族解析位址與重複位址檢查
            let parsed_addr = if let Some((ref plc_id, family)) = parsed_plc {
                match parse_plc_address(family, &tag.address) {
                    Ok(addr) => {
                        let key = (plc_id.clone(), addr.canonical.clone());
                        if let Some(prev_tag) = seen_plc_addresses.insert(key, tag.id.clone()) {
                            diags.push(Diagnostic::error(
                                "R9",
                                &path,
                                format!(
                                    "Tag '{}' 與 '{}' 在 PLC '{}' 上使用了相同的軟元件位址 '{}'",
                                    tag.id, prev_tag, plc_id, addr.canonical
                                ),
                            ));
                        }
                        Some(addr)
                    }
                    Err(err_msg) => {
                        diags.push(Diagnostic::error("R8", &format!("{}.address", path), err_msg));
                        None
                    }
                }
            } else {
                None
            };

            // R17: scale 不得為 0
            if let Some(scale) = tag.scale {
                if scale.abs() < f64::EPSILON {
                    diags.push(Diagnostic::error(
                        "R17",
                        &format!("{}.scale", path),
                        "Tag 的 scale 不得為 0",
                    ));
                }
            }

            // R18: min / max / default 範圍合理性
            if let (Some(min), Some(max)) = (tag.min, tag.max) {
                if min > max {
                    diags.push(Diagnostic::error(
                        "R18",
                        &path,
                        format!("Tag '{}' 的 min ({}) 大於 max ({})", tag.id, min, max),
                    ));
                }
            }
            if let Some(def_val) = tag.default.as_ref().and_then(|v| v.as_f64()) {
                if let Some(min) = tag.min {
                    if def_val < min {
                        diags.push(Diagnostic::error(
                            "R18",
                            &path,
                            format!("Tag '{}' 的 default ({}) 小於 min ({})", tag.id, def_val, min),
                        ));
                    }
                }
                if let Some(max) = tag.max {
                    if def_val > max {
                        diags.push(Diagnostic::error(
                            "R18",
                            &path,
                            format!("Tag '{}' 的 default ({}) 大於 max ({})", tag.id, def_val, max),
                        ));
                    }
                }
            }

            // R19: polarity 只能用於 bool 的唯讀輸入位址類別
            if tag.polarity.is_some() {
                let is_bool = parsed_type.as_ref().map(|t| t.is_bool()).unwrap_or(false);
                let is_ro_input = parsed_addr
                    .as_ref()
                    .map(|a| a.category.is_read_only())
                    .unwrap_or(false);
                if !is_bool || !is_ro_input {
                    diags.push(Diagnostic::error(
                        "R19",
                        &format!("{}.polarity", path),
                        format!(
                            "Tag '{}' 設定了 polarity，但 polarity 只能用於型別為 bool 且位於唯讀輸入位址類別（如 X）的點位",
                            tag.id
                        ),
                    ));
                }
            }

            // R21: ext.<廠牌>.<欄位> 檢查
            for (vendor, fields) in &tag.ext {
                if !is_valid_identifier(vendor) || fields.is_empty() {
                    diags.push(Diagnostic::error(
                        "R21",
                        &format!("{}.ext.{}", path, vendor),
                        format!("擴充欄位必須符合 'ext.<廠牌>.<欄位>' 格式（廠牌 '{}' 無效或無欄位）", vendor),
                    ));
                }
                for field_name in fields.keys() {
                    if !is_valid_identifier(field_name) {
                        diags.push(Diagnostic::error(
                            "R21",
                            &format!("{}.ext.{}.{}", path, vendor, field_name),
                            format!("擴充欄位名稱 '{}' 不合法", field_name),
                        ));
                    }
                }
            }
        }

        // R26 & R28: units[] id 與 bind 載入安全檢查
        let mut seen_unit_ids = BTreeSet::new();
        for (idx, unit) in machine.units.iter().enumerate() {
            let path = format!("units[{}]({})", idx, unit.id);
            if unit.id == "unassigned" {
                diags.push(Diagnostic::error(
                    "R26",
                    &path,
                    "Unit 實例 id 不得使用保留字 'unassigned'",
                ));
            } else if !is_valid_identifier(&unit.id) {
                diags.push(Diagnostic::error(
                    "R26",
                    &path,
                    format!("Unit 實例 id '{}' 格式不合法", unit.id),
                ));
            }
            if !seen_unit_ids.insert(unit.id.clone()) {
                diags.push(Diagnostic::error(
                    "R26",
                    &path,
                    format!("Unit 實例 id '{}' 重複", unit.id),
                ));
            }

            // R28: units[].type, type_version >= 1, bind 鍵與值格式
            if !is_valid_identifier(&unit.unit_type) {
                diags.push(Diagnostic::error(
                    "R28",
                    &format!("{}.type", path),
                    format!("Unit 實例 '{}' 的 type '{}' 名稱不合法", unit.id, unit.unit_type),
                ));
            }
            if unit.type_version == 0 {
                diags.push(Diagnostic::error(
                    "R28",
                    &format!("{}.type_version", path),
                    format!("Unit 實例 '{}' 的 type_version 必須是正整數 (>= 1)", unit.id),
                ));
            }
            for (bind_key, bind_val) in &unit.bind {
                let bind_path = format!("{}.bind.{}", path, bind_key);
                if !is_valid_identifier(bind_key) {
                    diags.push(Diagnostic::error(
                        "R28",
                        &bind_path,
                        format!("bind 鍵名 '{}' 不是合法識別字", bind_key),
                    ));
                }
                if let Some(target) = bind_val {
                    if let Some((plc_id, tag_name)) = target.split_once('.') {
                        if !is_valid_identifier(plc_id) || !is_valid_identifier(tag_name) {
                            diags.push(Diagnostic::error(
                                "R28",
                                &bind_path,
                                format!("bind 值 '{}' 不是合法的 Tag id ('<plc>.<Name>')", target),
                            ));
                        } else if !plc_families.contains_key(plc_id) {
                            diags.push(Diagnostic::error(
                                "R28",
                                &bind_path,
                                format!("bind 值 '{}' 引用的 PLC '{}' 未宣告", target, plc_id),
                            ));
                        }
                    } else if !is_valid_identifier(target) || target == "unassigned" {
                        diags.push(Diagnostic::error(
                            "R28",
                            &bind_path,
                            format!("bind 值 '{}' 不是合法的 Unit id", target),
                        ));
                    }
                }
            }
        }

        diags
    }

    /// 驗證單一 Unit 類型檔 (SC2, SC3, U1, U12, U15, U19)
    pub fn validate_unit_type(ut: &UnitTypeDef) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let base_path = format!("unit_type({})", ut.id);

        if !is_valid_identifier(&ut.id) {
            diags.push(Diagnostic::error(
                "SC2",
                &format!("{}.id", base_path),
                format!("類型 id '{}' 格式不合法", ut.id),
            ));
        }
        if ut.version == 0 {
            diags.push(Diagnostic::error(
                "SC2",
                &format!("{}.version", base_path),
                "類型 version 必須為正整數 (>= 1)",
            ));
        }
        if ut.title.trim().is_empty() {
            diags.push(Diagnostic::error(
                "SC2",
                &format!("{}.title", base_path),
                "類型 title 不得為空",
            ));
        }

        // U1: 類型內接腳／part 名稱在 field, ctrl, parts 全體唯一
        let mut all_names: BTreeMap<String, &'static str> = BTreeMap::new();
        let mut check_unique = |name: &str, section: &'static str, diags: &mut Vec<Diagnostic>| {
            if let Some(prev_sec) = all_names.insert(name.to_string(), section) {
                diags.push(Diagnostic::error(
                    "U1",
                    &format!("{}.{}.{}", base_path, section, name),
                    format!(
                        "類型 '{}' 內的名稱 '{}' 在 '{}' 與 '{}' 中重複（field、ctrl、parts 全體須唯一）",
                        ut.id, name, prev_sec, section
                    ),
                ));
            }
        };

        // 檢查 field.in 與 field.out (SC2: 不得以控制前綴開頭；型別限標量詞彙)
        for (dir_label, map) in [("field.in", &ut.field.inputs), ("field.out", &ut.field.outputs)] {
            for (pin_name, pin_def) in map {
                let pin_path = format!("{}.{}.{}", base_path, dir_label, pin_name);
                check_unique(pin_name, dir_label, &mut diags);
                if let Some(spec) = starts_with_reserved_ctrl_prefix(pin_name) {
                    diags.push(Diagnostic::error(
                        "SC2",
                        &pin_path,
                        format!(
                            "實體接腳 '{}' 不得以控制前綴 '{}_' 開頭（避免與 ctrl 介面混淆）",
                            pin_name,
                            spec.prefix.as_str()
                        ),
                    ));
                }
                if let Err(msg) = ScalarType::parse(&pin_def.data_type) {
                    diags.push(Diagnostic::error("SC3", &format!("{}.type", pin_path), msg.to_string()));
                }
            }
        }

        // 檢查 ctrl 接腳 (SC3, U12, U15, U19)
        let mut feature_counts: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for (pin_name, pin_def) in &ut.ctrl {
            let pin_path = format!("{}.ctrl.{}", base_path, pin_name);
            check_unique(pin_name, "ctrl", &mut diags);

            let (parsed_name, spec) = match parse_ctrl_pin_name(pin_name) {
                Ok(res) => res,
                Err(msg) => {
                    diags.push(Diagnostic::error("SC3", &pin_path, msg));
                    continue;
                }
            };

            feature_counts
                .entry(parsed_name.feature.clone())
                .or_default()
                .push(pin_name.clone());

            // 檢查各前綴必要屬性與禁止屬性 (§4.1, §4.2, §5)
            let eff_type = match resolve_ctrl_pin_type(pin_name, pin_def.data_type.as_deref(), spec, &pin_path) {
                Ok(t) => Some(t),
                Err(d) => {
                    diags.push(d);
                    None
                }
            };

            match parsed_name.prefix {
                CtrlPrefix::Par => {
                    // par_ 必填 type, min, max, default；不得有 message
                    if pin_def.message.is_some() {
                        diags.push(Diagnostic::error(
                            "SC2",
                            &pin_path,
                            format!("參數接腳 '{}' 不允許設定 'message' 屬性（僅 error_ 可用）", pin_name),
                        ));
                    }
                    match (pin_def.min, pin_def.max, pin_def.default) {
                        (Some(min), Some(max), Some(def)) => {
                            // U12: min <= default <= max
                            if !(min <= def && def <= max) {
                                diags.push(Diagnostic::error(
                                    "U12",
                                    &pin_path,
                                    format!(
                                        "參數接腳 '{}' 違反 U12 (min ≤ default ≤ max)：min={}, default={}, max={}",
                                        pin_name, min, def, max
                                    ),
                                ));
                            }
                        }
                        _ => {
                            diags.push(Diagnostic::error(
                                "SC3",
                                &pin_path,
                                format!("參數接腳 '{}' (par_) 必須同時提供 min、max、default 屬性", pin_name),
                            ));
                        }
                    }
                }
                CtrlPrefix::Error => {
                    // error_ 必填 message；不得有 min/max/default；U19 型別必須是 bool
                    if pin_def.message.as_deref().map(str::trim).unwrap_or("").is_empty() {
                        diags.push(Diagnostic::error(
                            "SC3",
                            &pin_path,
                            format!("警報接腳 '{}' (error_) 必須填寫非空的 'message' 屬性", pin_name),
                        ));
                    }
                    if pin_def.min.is_some() || pin_def.max.is_some() || pin_def.default.is_some() {
                        diags.push(Diagnostic::error(
                            "SC2",
                            &pin_path,
                            format!("警報接腳 '{}' 不允許設定 min／max／default（僅 par_ 可用）", pin_name),
                        ));
                    }
                    if let Some(ref t) = eff_type {
                        if !t.is_bool() {
                            diags.push(Diagnostic::error(
                                "U19",
                                &pin_path,
                                format!("警報接腳 '{}' (error_) 的型別必須為 bool（目前為 {}）", pin_name, t),
                            ));
                        }
                    }
                }
                CtrlPrefix::Msg => {
                    if pin_def.min.is_some()
                        || pin_def.max.is_some()
                        || pin_def.default.is_some()
                        || pin_def.message.is_some()
                    {
                        diags.push(Diagnostic::error(
                            "SC2",
                            &pin_path,
                            format!("訊息接腳 '{}' 包含不允許的屬性（min/max/default/message）", pin_name),
                        ));
                    }
                    if let Some(ref t) = eff_type {
                        if !t.is_string_scalar() {
                            diags.push(Diagnostic::error(
                                "SC3",
                                &pin_path,
                                format!("訊息接腳 '{}' (msg_) 的型別必須為 string:N（目前為 {}）", pin_name, t),
                            ));
                        }
                    }
                }
                _ => {
                    if pin_def.min.is_some()
                        || pin_def.max.is_some()
                        || pin_def.default.is_some()
                        || pin_def.message.is_some()
                    {
                        diags.push(Diagnostic::error(
                            "SC2",
                            &pin_path,
                            format!(
                                "控制接腳 '{}' (前綴 '{}') 不允許設定 min／max／default／message",
                                pin_name, parsed_name.prefix
                            ),
                        ));
                    }
                }
            }
        }

        // U15: 接腳功能名拼寫相近（僅出現 1 次的功能名與出現 >= 2 次、編輯距離 1 的功能名 -> 提示）
        let frequent_features: Vec<&String> = feature_counts
            .iter()
            .filter(|(_, pins)| pins.len() >= 2)
            .map(|(feat, _)| feat)
            .collect();

        for (feat, pins) in &feature_counts {
            if pins.len() == 1 {
                for freq in &frequent_features {
                    if edit_distance(feat, freq) == 1 {
                        diags.push(Diagnostic::hint(
                            "U15",
                            &format!("{}.ctrl.{}", base_path, pins[0]),
                            format!(
                                "接腳 '{}' 的功能名 '{}' 在此類型僅出現 1 次，與出現多次的功能名 '{}' 編輯距離為 1，請確認是否為拼寫錯誤",
                                pins[0], feat, freq
                            ),
                        ));
                    }
                }
            }
        }

        // 檢查 parts 名稱唯一性
        for (part_name, part_type) in &ut.parts {
            let part_path = format!("{}.parts.{}", base_path, part_name);
            check_unique(part_name, "parts", &mut diags);
            if !is_valid_identifier(part_name) || !is_valid_identifier(part_type) {
                diags.push(Diagnostic::error(
                    "SC2",
                    &part_path,
                    format!("part 宣告 '{}: {}' 名稱或類型格式不合法", part_name, part_type),
                ));
            }
        }

        diags
    }

    /// 驗證整個類型目錄（含 U17 載入診斷、每個類型的 SC2/SC3/U1/U12/U15/U19，以及 U18 parts 引用存在與無環）
    pub fn validate_catalog(catalog: &TypeCatalog) -> Vec<Diagnostic> {
        let mut diags = catalog.load_diagnostics.clone();

        for entry in catalog.entries.values() {
            diags.extend(validate_unit_type(&entry.def));
        }

        // U18: parts 引用的類型存在於目錄，且類型之間的 parts 關係無環（含自我引用）
        for entry in catalog.entries.values() {
            for (part_name, target_type) in &entry.def.parts {
                if !catalog.entries.contains_key(target_type) {
                    diags.push(Diagnostic::error(
                        "U18",
                        format!("unit_type({}).parts.{}", entry.def.id, part_name),
                        format!(
                            "類型 '{}' 的 part '{}' 引用了不存在的 Unit 類型 '{}'",
                            entry.def.id, part_name, target_type
                        ),
                    ));
                }
            }
        }

        // 三色 DFS 檢查類型 parts 關係無環 (U18)
        let mut color: BTreeMap<&str, u8> = BTreeMap::new(); // 0=未訪, 1=訪問中, 2=已完成
        for type_id in catalog.entries.keys() {
            if color.get(type_id.as_str()).copied().unwrap_or(0) == 0 {
                let mut stack = Vec::new();
                dfs_type_cycle(type_id, catalog, &mut color, &mut stack, &mut diags);
            }
        }

        diags
    }

    fn dfs_type_cycle<'a>(
        curr: &'a str,
        catalog: &'a TypeCatalog,
        color: &mut BTreeMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        color.insert(curr, 1);
        stack.push(curr);

        if let Some(entry) = catalog.entries.get(curr) {
            for target in entry.def.parts.values() {
                if let Some(target_entry) = catalog.entries.get(target) {
                    let target_id = target_entry.def.id.as_str();
                    match color.get(target_id).copied().unwrap_or(0) {
                        1 => {
                            let mut cycle_chain: Vec<&str> = stack
                                .iter()
                                .skip_while(|&&x| x != target_id)
                                .copied()
                                .collect();
                            cycle_chain.push(target_id);
                            diags.push(Diagnostic::error(
                                "U18",
                                format!("unit_type({})", curr),
                                format!("Unit 類型之間的 parts 關係形成環路：{}", cycle_chain.join(" -> ")),
                            ));
                        }
                        0 => dfs_type_cycle(target_id, catalog, color, stack, diags),
                        _ => {}
                    }
                }
            }
        }

        stack.pop();
        color.insert(curr, 2);
    }

    /// 完整驗證 `machine.yaml` 與 `TypeCatalog`（載入安全規則 R + 類型規則 SC + 實例關係規則 U1~U19）
    pub fn validate_machine_with_catalog(
        machine: &MachineFile,
        catalog: &TypeCatalog,
    ) -> Vec<Diagnostic> {
        let mut diags = validate_load_safety(machine);
        diags.extend(validate_catalog(catalog));

        // 建立 PLC 與 Tag 查找表
        let plc_families: BTreeMap<&str, PlcFamily> = machine
            .machine
            .plcs
            .iter()
            .map(|p| (p.id.as_str(), p.family))
            .collect();

        struct TagMeta<'a> {
            def: &'a super::model::TagDef,
            data_type: Option<DataType>,
            addr_category: Option<super::family::AddressCategory>,
        }

        let mut tag_map: BTreeMap<&str, TagMeta<'_>> = BTreeMap::new();
        for tag in &machine.tags {
            let dt = DataType::parse(&tag.data_type, &tag.id).ok();
            let cat = tag
                .id
                .split_once('.')
                .and_then(|(plc_id, _)| plc_families.get(plc_id))
                .and_then(|&fam| parse_plc_address(fam, &tag.address).ok())
                .map(|a| a.category);
            tag_map.insert(
                tag.id.as_str(),
                TagMeta {
                    def: tag,
                    data_type: dt,
                    addr_category: cat,
                },
            );
        }

        // U1: units[].id 唯一（若 R26 已報，這裡建立索引）
        let mut unit_map: BTreeMap<&str, &super::model::UnitInstanceDef> = BTreeMap::new();
        for unit in &machine.units {
            unit_map.insert(unit.id.as_str(), unit);
        }

        // 紀錄每個 Unit 的父層以檢查 U7 (單親)、U8 (無環與可達)、U9 (machine 為根)
        let mut parent_map: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new(); // child_unit_id -> Vec<(parent_unit_id, part_name)>
        let mut children_adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        // 紀錄每個 Tag 被哪些接腳綁定，以檢查 U11 (多重綁定限制) 與 U14 (明確 role vs 推測 role)
        #[derive(Clone)]
        struct BoundPinRecord {
            unit_id: String,
            pin_name: String,
            is_field: bool,
            is_field_out: bool,
            inferred_role: Option<super::naming::Role>,
            write_candidate: bool,
            expected_type: DataType,
        }
        let mut tag_bindings: BTreeMap<String, Vec<BoundPinRecord>> = BTreeMap::new();

        for unit in &machine.units {
            let unit_path = format!("units({})", unit.id);

            // U2: units[].type 存在於類型目錄，且 type_version 等於該類型的 version
            let Some(type_entry) = catalog.get(&unit.unit_type) else {
                diags.push(Diagnostic::error(
                    "U2",
                    &unit_path,
                    format!("Unit '{}' 引用的類型 '{}' 不存在於類型目錄中", unit.id, unit.unit_type),
                ));
                continue;
            };
            let ut = &type_entry.def;
            if unit.type_version != ut.version {
                diags.push(Diagnostic::error(
                    "U2",
                    &format!("{}.type_version", unit_path),
                    format!(
                        "Unit '{}' 的 type_version ({}) 與目錄中類型 '{}' 的 version ({}) 不符（版本固定檢查失敗）",
                        unit.id, unit.type_version, ut.id, ut.version
                    ),
                ));
            }

            // U3: bind 的鍵必須是該類型宣告的接腳或 part
            for bind_key in unit.bind.keys() {
                let in_field_in = ut.field.inputs.contains_key(bind_key);
                let in_field_out = ut.field.outputs.contains_key(bind_key);
                let in_ctrl = ut.ctrl.contains_key(bind_key);
                let in_parts = ut.parts.contains_key(bind_key);
                if !(in_field_in || in_field_out || in_ctrl || in_parts) {
                    diags.push(Diagnostic::error(
                        "U3",
                        &format!("{}.bind.{}", unit_path, bind_key),
                        format!(
                            "Unit '{}' (類型 '{}') 的 bind 包含未宣告的接腳或 part '{}'",
                            unit.id, ut.id, bind_key
                        ),
                    ));
                }
            }

            // U4 & U6: 檢查 field.in 與 field.out 綁定
            for (is_out, dir_name, field_map) in [
                (false, "field.in", &ut.field.inputs),
                (true, "field.out", &ut.field.outputs),
            ] {
                for (pin_name, pin_def) in field_map {
                    let bind_path = format!("{}.bind.{}", unit_path, pin_name);
                    let expected_dt = ScalarType::parse(&pin_def.data_type)
                        .map(DataType::Scalar)
                        .unwrap_or(DataType::Scalar(ScalarType::Bool));

                    match unit.bind.get(pin_name) {
                        None => {
                            // U6: 必綁 field 腳（除非 required: false）
                            if pin_def.required {
                                diags.push(Diagnostic::error(
                                    "U6",
                                    &unit_path,
                                    format!(
                                        "Unit '{}' 未綁定必要的 {} 接腳 '{}'（若刻意不接請明確寫 '{}: null' 或於類型設 required: false）",
                                        unit.id, dir_name, pin_name, pin_name
                                    ),
                                ));
                            }
                        }
                        Some(None) => {
                            // 明確 null 視為刻意不接 (U6 允許)
                        }
                        Some(Some(target_tag_id)) => {
                            let Some(tag_meta) = tag_map.get(target_tag_id.as_str()) else {
                                diags.push(Diagnostic::error(
                                    "U4",
                                    &bind_path,
                                    format!(
                                        "Unit '{}' 的 {} 接腳 '{}' 綁定了不存在的 Tag '{}'",
                                        unit.id, dir_name, pin_name, target_tag_id
                                    ),
                                ));
                                continue;
                            };

                            // 檢查位址方向：in 須為唯讀輸入 (X)，out 須為實體輸出 (Y)
                            if let Some(cat) = tag_meta.addr_category {
                                if !is_out && !cat.is_read_only() {
                                    diags.push(Diagnostic::error(
                                        "U4",
                                        &bind_path,
                                        format!(
                                            "Unit '{}' 的 field.in 接腳 '{}' 綁定至 '{}' ({})，但 field.in 必須綁定唯讀輸入位址類別（如 X）",
                                            unit.id, pin_name, target_tag_id, tag_meta.def.address
                                        ),
                                    ));
                                } else if is_out && !cat.is_physical_output() {
                                    diags.push(Diagnostic::error(
                                        "U4",
                                        &bind_path,
                                        format!(
                                            "Unit '{}' 的 field.out 接腳 '{}' 綁定至 '{}' ({})，但 field.out 必須綁定實體輸出位址類別（如 Y）",
                                            unit.id, pin_name, target_tag_id, tag_meta.def.address
                                        ),
                                    ));
                                }
                            }

                            // 檢查型別完全相符
                            if let Some(ref actual_dt) = tag_meta.data_type {
                                if actual_dt != &expected_dt {
                                    diags.push(Diagnostic::error(
                                        "U4",
                                        &bind_path,
                                        format!(
                                            "Unit '{}' 的 {} 接腳 '{}' 型別為 '{}'，與所綁 Tag '{}' 的型別 '{}' 不符",
                                            unit.id, dir_name, pin_name, expected_dt, target_tag_id, actual_dt
                                        ),
                                    ));
                                }
                            }

                            tag_bindings
                                .entry(target_tag_id.clone())
                                .or_default()
                                .push(BoundPinRecord {
                                    unit_id: unit.id.clone(),
                                    pin_name: pin_name.clone(),
                                    is_field: true,
                                    is_field_out: is_out,
                                    inferred_role: None,
                                    write_candidate: false,
                                    expected_type: expected_dt,
                                });
                        }
                    }
                }
            }

            // U5: 檢查 parts 綁定
            for (part_name, expected_unit_type) in &ut.parts {
                let bind_path = format!("{}.bind.{}", unit_path, part_name);
                match unit.bind.get(part_name) {
                    None | Some(None) => {
                        diags.push(Diagnostic::error(
                            "U5",
                            &unit_path,
                            format!(
                                "Unit '{}' 的子單元槽位 (part) '{}' 必須綁定一個子 Unit 實例",
                                unit.id, part_name
                            ),
                        ));
                    }
                    Some(Some(child_id)) => {
                        if let Some(&child_unit) = unit_map.get(child_id.as_str()) {
                            if &child_unit.unit_type != expected_unit_type {
                                diags.push(Diagnostic::error(
                                    "U5",
                                    &bind_path,
                                    format!(
                                        "Unit '{}' 的 part '{}' 要求類型 '{}'，但綁定的 Unit '{}' 類型為 '{}'",
                                        unit.id, part_name, expected_unit_type, child_id, child_unit.unit_type
                                    ),
                                ));
                            }
                            parent_map
                                .entry(child_unit.id.as_str())
                                .or_default()
                                .push((unit.id.as_str(), part_name.as_str()));
                            children_adj
                                .entry(unit.id.as_str())
                                .or_default()
                                .push(child_unit.id.as_str());
                        } else {
                            diags.push(Diagnostic::error(
                                "U5",
                                &bind_path,
                                format!(
                                    "Unit '{}' 的 part '{}' 綁定了不存在的 Unit 實例 '{}'",
                                    unit.id, part_name, child_id
                                ),
                            ));
                        }
                    }
                }
            }

            // U6, U10, U13, U16: 檢查 ctrl 接腳綁定
            for (pin_name, pin_def) in &ut.ctrl {
                let bind_path = format!("{}.bind.{}", unit_path, pin_name);
                let Ok((parsed_pin, spec)) = parse_ctrl_pin_name(pin_name) else {
                    continue;
                };
                let Ok(expected_dt) = resolve_ctrl_pin_type(
                    pin_name,
                    pin_def.data_type.as_deref(),
                    spec,
                    &bind_path,
                ) else {
                    continue;
                };

                match unit.bind.get(pin_name) {
                    None | Some(None) => {
                        match spec.binding_policy {
                            BindingPolicy::Required => {
                                // U6: error_ 腳必綁，且不得為 null
                                diags.push(Diagnostic::error(
                                    "U6",
                                    &unit_path,
                                    format!(
                                        "Unit '{}' 的必綁警報接腳 '{}' (error_) 尚未綁定 Tag",
                                        unit.id, pin_name
                                    ),
                                ));
                            }
                            BindingPolicy::Optional => {
                                // U16: 未綁的選擇性 ctrl 腳：不報錯，列入「HMI 覆蓋率」報告
                                diags.push(Diagnostic::info(
                                    "U16",
                                    &unit_path,
                                    format!(
                                        "HMI 覆蓋率：Unit '{}' 的選擇性控制接腳 '{}' ({}) 未綁定 Tag",
                                        unit.id, pin_name, spec.prefix
                                    ),
                                ));
                            }
                            BindingPolicy::PureWiring => {
                                // cmd_ / ena_ 純接線，不需綁定 Tag
                            }
                        }
                    }
                    Some(Some(target_tag_id)) => {
                        if spec.binding_policy == BindingPolicy::PureWiring {
                            diags.push(Diagnostic::error(
                                "U10",
                                &bind_path,
                                format!(
                                    "Unit '{}' 的接腳 '{}' (前綴 '{}') 為純接線介面，不得綁定 Tag '{}'",
                                    unit.id, pin_name, spec.prefix, target_tag_id
                                ),
                            ));
                            continue;
                        }

                        let Some(tag_meta) = tag_map.get(target_tag_id.as_str()) else {
                            diags.push(Diagnostic::error(
                                "U10",
                                &bind_path,
                                format!(
                                    "Unit '{}' 的控制接腳 '{}' 綁定了不存在的 Tag '{}'",
                                    unit.id, pin_name, target_tag_id
                                ),
                            ));
                            continue;
                        };

                        // U10: 型別相容
                        if let Some(ref actual_dt) = tag_meta.data_type {
                            if actual_dt != &expected_dt {
                                diags.push(Diagnostic::error(
                                    "U10",
                                    &bind_path,
                                    format!(
                                        "Unit '{}' 的控制接腳 '{}' 型別為 '{}'，與所綁 Tag '{}' 的型別 '{}' 不相容",
                                        unit.id, pin_name, expected_dt, target_tag_id, actual_dt
                                    ),
                                ));
                            }
                        }

                        // U13: man_*、par_* 接腳綁定的 Tag 不得位於唯讀位址類別（如 X），也不得 safety_monitor: true
                        if matches!(parsed_pin.prefix, CtrlPrefix::Man | CtrlPrefix::Par) {
                            if tag_meta.addr_category.map(|c| c.is_read_only()).unwrap_or(false) {
                                diags.push(Diagnostic::error(
                                    "U13",
                                    &bind_path,
                                    format!(
                                        "Unit '{}' 的可寫候選接腳 '{}' ({}) 不得綁定至唯讀位址類別的 Tag '{}' ({})",
                                        unit.id, pin_name, parsed_pin.prefix, target_tag_id, tag_meta.def.address
                                    ),
                                ));
                            }
                            if tag_meta.def.safety_monitor {
                                diags.push(Diagnostic::error(
                                    "U13",
                                    &bind_path,
                                    format!(
                                        "Unit '{}' 的可寫候選接腳 '{}' ({}) 不得綁定至 safety_monitor: true 的安全監控 Tag '{}'",
                                        unit.id, pin_name, parsed_pin.prefix, target_tag_id
                                    ),
                                ));
                            }
                        }

                        tag_bindings
                            .entry(target_tag_id.clone())
                            .or_default()
                            .push(BoundPinRecord {
                                unit_id: unit.id.clone(),
                                pin_name: pin_name.clone(),
                                is_field: false,
                                is_field_out: false,
                                inferred_role: Some(spec.inferred_role),
                                write_candidate: spec.write_candidate,
                                expected_type: expected_dt,
                            });
                    }
                }
            }
        }

        // U7: 每個 Unit 至多一個父層
        for (child_id, parents) in &parent_map {
            if parents.len() > 1 {
                let desc: Vec<String> = parents
                    .iter()
                    .map(|(p, part)| format!("{}.{}", p, part))
                    .collect();
                diags.push(Diagnostic::error(
                    "U7",
                    format!("units({})", child_id),
                    format!(
                        "Unit '{}' 被多個父層同時綁定 ({})，違反單親樹限制",
                        child_id,
                        desc.join(", ")
                    ),
                ));
            }
        }

        // U9: machine 存在且為根（無父層）
        let has_machine_root = unit_map.contains_key("machine");
        if !has_machine_root {
            diags.push(Diagnostic::error(
                "U9",
                "units",
                "機台模型必須包含根 Unit 實例 'machine'",
            ));
        } else if let Some(parents) = parent_map.get("machine") {
            if !parents.is_empty() {
                diags.push(Diagnostic::error(
                    "U9",
                    "units(machine)",
                    "保留 Unit 'machine' 必須為樹根，不得作為其他 Unit 的子部件 (part)",
                ));
            }
        }

        // U8: 實例樹無環，且所有 Unit 皆可由 machine 到達
        let mut color: BTreeMap<&str, u8> = BTreeMap::new();
        for &uid in unit_map.keys() {
            if color.get(uid).copied().unwrap_or(0) == 0 {
                let mut stack = Vec::new();
                dfs_unit_cycle(uid, &children_adj, &mut color, &mut stack, &mut diags);
            }
        }

        if has_machine_root {
            let mut reachable = BTreeSet::new();
            let mut queue = VecDeque::new();
            queue.push_back("machine");
            reachable.insert("machine");
            while let Some(curr) = queue.pop_front() {
                if let Some(children) = children_adj.get(curr) {
                    for &ch in children {
                        if reachable.insert(ch) {
                            queue.push_back(ch);
                        }
                    }
                }
            }
            for &uid in unit_map.keys() {
                if !reachable.contains(uid) {
                    diags.push(Diagnostic::error(
                        "U8",
                        format!("units({})", uid),
                        format!("Unit '{}' 無法由根單元 'machine' 到達（孤立單元或環路）", uid),
                    ));
                }
            }
        }

        // U11 & U14: 多重綁定限制與明確 role 覆寫提示
        for (tag_id, records) in &tag_bindings {
            let tag_path = format!("tags({})", tag_id);
            let Some(tag_meta) = tag_map.get(tag_id.as_str()) else {
                continue;
            };

            if records.len() > 1 {
                let any_field_out = records.iter().any(|r| r.is_field_out);
                if any_field_out {
                    let refs: Vec<String> = records
                        .iter()
                        .map(|r| format!("{}.{}", r.unit_id, r.pin_name))
                        .collect();
                    diags.push(Diagnostic::error(
                        "U11",
                        &tag_path,
                        format!(
                            "Tag '{}' 被綁定至 field.out 接腳，必須全機唯一，但目前被多個接腳重複綁定：[{}]",
                            tag_id,
                            refs.join(", ")
                        ),
                    ));
                } else {
                    let first = &records[0];
                    let mixed_field_ctrl = records.iter().any(|r| r.is_field != first.is_field);
                    let mixed_writable = records
                        .iter()
                        .any(|r| r.write_candidate != first.write_candidate);
                    let mixed_role = records
                        .iter()
                        .any(|r| r.inferred_role != first.inferred_role);
                    let mixed_type = records
                        .iter()
                        .any(|r| r.expected_type != first.expected_type);

                    if mixed_field_ctrl || mixed_writable || mixed_role || mixed_type {
                        let refs: Vec<String> = records
                            .iter()
                            .map(|r| format!("{}.{}", r.unit_id, r.pin_name))
                            .collect();
                        diags.push(Diagnostic::error(
                            "U11",
                            &tag_path,
                            format!(
                                "Tag '{}' 被多個語意或可寫性不相容的接腳同時綁定：[{}]（禁止混用 field/ctrl、可寫候選與唯讀、或不同 role/type）",
                                tag_id,
                                refs.join(", ")
                            ),
                        ));
                    }
                }
            }

            // U14: Tag 上明確寫的 role 與前綴推測不同 -> 提示（以明確值為準）
            if let Some(explicit_role) = tag_meta.def.role {
                if let Some(inferred_role) = records.first().and_then(|r| r.inferred_role) {
                    if explicit_role != inferred_role {
                        diags.push(Diagnostic::hint(
                            "U14",
                            &format!("{}.role", tag_path),
                            format!(
                                "Tag '{}' 明確指定 role 為 '{}'，與接腳前綴推測的 '{}' 不同（將以明確值 '{}' 為準）",
                                tag_id, explicit_role, inferred_role, explicit_role
                            ),
                        ));
                    }
                }
            }
        }

        diags
    }

    fn dfs_unit_cycle<'a>(
        curr: &'a str,
        adj: &BTreeMap<&'a str, Vec<&'a str>>,
        color: &mut BTreeMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
        diags: &mut Vec<Diagnostic>,
    ) {
        color.insert(curr, 1);
        stack.push(curr);

        if let Some(children) = adj.get(curr) {
            for &ch in children {
                match color.get(ch).copied().unwrap_or(0) {
                    1 => {
                        let mut cycle: Vec<&str> =
                            stack.iter().skip_while(|&&x| x != ch).copied().collect();
                        cycle.push(ch);
                        diags.push(Diagnostic::error(
                            "U8",
                            format!("units({})", curr),
                            format!("Unit 實例樹存在環路：{}", cycle.join(" -> ")),
                        ));
                    }
                    0 => dfs_unit_cycle(ch, adj, color, stack, diags),
                    _ => {}
                }
            }
        }

        stack.pop();
        color.insert(curr, 2);
    }

    /// 選用的 `jsonschema` crate 驗證支援 (§8.5 `schema` feature)
    #[cfg(feature = "schema")]
    pub fn validate_unit_type_with_jsonschema(yaml_str: &str, source_path: &str) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        let schema_str = super::unit_type::generate_unit_type_json_schema();
        let Ok(schema_json) = serde_json::from_str::<serde_json::Value>(&schema_str) else {
            return diags;
        };
        let Ok(instance_json) = serde_yaml_ng::from_str::<serde_json::Value>(yaml_str) else {
            return diags;
        };
        if let Ok(validator) = jsonschema::validator_for(&schema_json) {
            for error in validator.iter_errors(&instance_json) {
                diags.push(Diagnostic::error(
                    "SC2",
                    source_path,
                    format!("JSON Schema 驗證失敗：{}", error),
                ));
            }
        }
        diags
    }
}

// ============================================================================
// 8. 轉換器與解析器：ResolvedMachineModel (§6, §9.2, §10)
// ============================================================================

pub mod resolver {
    use super::diagnostics::{Diagnostic, has_errors};
    use super::family::{PlcFamily, parse_plc_address};
    use super::model::{MachineFile, ScanRate, ScalarValue};
    use super::naming::{
        BindingPolicy, CtrlPrefix, Role, default_feature_label, parse_ctrl_pin_name,
        resolve_ctrl_pin_type,
    };
    use super::unit_type::{TypeCatalog, fnv1a_64_hex};
    use super::validator::validate_machine_with_catalog;
    use super::*;

    /// Tag 所綁定的 Unit 接腳路徑紀錄 (§9.2 `bindings`)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct TagBindingRef {
        pub unit_id: String,
        pub unit_path: String,
        pub pin_name: String,
        /// `"field.in"`、`"field.out"` 或控制前綴如 `"man"`、`"par"`、`"error"`
        pub category: String,
    }

    /// 有效 Tag 表單筆項目 (§9.2 `EffectiveTag`)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct EffectiveTag {
        pub id: String,
        pub address: String,
        #[serde(rename = "type")]
        pub data_type: String,
        pub scan: ScanRate,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub scale: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub offset: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub deadband: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
        pub readback: bool,
        pub retain: bool,
        pub safety_monitor: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub role: Option<Role>,
        pub role_reason: String,
        pub write_candidate: bool,
        pub write_candidate_reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub min: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub max: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub default: Option<ScalarValue>,
        pub address_read_only: bool,
        pub bindings: Vec<TagBindingRef>,
    }

    /// 解析後之 Unit 樹節點 (§9.2)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ResolvedUnitNode {
        pub id: String,
        #[serde(rename = "type")]
        pub unit_type: String,
        pub type_version: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub parent_id: Option<String>,
        pub unit_path: String,
        pub bind: BTreeMap<String, Option<String>>,
    }

    /// HMI 導出之操作項 (§9.2, §10：由同名後綴 `<f>` 聚合 `man_<f>`、`ena_<f>`、`at_<f>`、`sts_<f>`)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DerivedOperationItem {
        pub unit_id: String,
        pub unit_path: String,
        pub feature: String,
        pub pin_name: String,
        pub prefix_category: String,
        pub label: String,
        pub man_tag: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub ena_pin: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub at_tag: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub sts_tag: Option<String>,
    }

    /// HMI 導出之警報項 (§9.2, §10 `error_`)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DerivedAlarmItem {
        pub unit_id: String,
        pub unit_path: String,
        pub pin_name: String,
        pub prefix_category: String,
        pub tag_id: String,
        pub label: String,
        pub message: String,
    }

    /// HMI 導出之參數項 (§9.2, §10 `par_`)
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DerivedParameterItem {
        pub unit_id: String,
        pub unit_path: String,
        pub pin_name: String,
        pub prefix_category: String,
        pub tag_id: String,
        pub label: String,
        #[serde(rename = "type")]
        pub data_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
        pub min: f64,
        pub max: f64,
        pub default: f64,
    }

    /// HMI 導出之數值／計數／訊息項 (§9.2, §10 `pv_`、`count_`、`msg_`)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct DerivedValueOrMsgItem {
        pub unit_id: String,
        pub unit_path: String,
        pub pin_name: String,
        pub prefix_category: String,
        pub tag_id: String,
        pub label: String,
        #[serde(rename = "type")]
        pub data_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub unit: Option<String>,
    }

    /// HMI 導出清單集合 (§9.2)
    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    pub struct DerivedCatalogs {
        pub operations: Vec<DerivedOperationItem>,
        pub alarms: Vec<DerivedAlarmItem>,
        pub parameters: Vec<DerivedParameterItem>,
        pub values: Vec<DerivedValueOrMsgItem>,
        pub messages: Vec<DerivedValueOrMsgItem>,
    }

    /// 單一 Unit 之 HMI 覆蓋率報告 (§10, U16)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct UnitHmiCoverage {
        pub unit_id: String,
        pub unit_type: String,
        pub bound_ctrl_pins: Vec<String>,
        pub unbound_optional_ctrl_pins: Vec<String>,
    }

    /// 模型指紋 (§9.2)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ModelFingerprint {
        pub machine_yaml_hash: String,
        pub used_unit_types: BTreeMap<String, UsedTypeVersionHash>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct UsedTypeVersionHash {
        pub version: u32,
        pub content_hash: String,
        pub origin: super::unit_type::TypeOrigin,
    }

    /// 完整解析結果（函式庫輸出，§9.2）
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ResolvedMachineModel {
        pub machine_id: String,
        pub effective_tags: Vec<EffectiveTag>,
        pub unit_tree: Vec<ResolvedUnitNode>,
        pub lists: DerivedCatalogs,
        pub hmi_coverage: Vec<UnitHmiCoverage>,
        pub fingerprint: ModelFingerprint,
        pub diagnostics: Vec<Diagnostic>,
    }

    /// 驗證並轉換 `MachineFile` 為 `ResolvedMachineModel` (§6, §9.2)
    pub fn resolve_machine_model(
        machine: &MachineFile,
        catalog: &TypeCatalog,
    ) -> Result<ResolvedMachineModel, Vec<Diagnostic>> {
        let diags = validate_machine_with_catalog(machine, catalog);
        if has_errors(&diags) {
            return Err(diags);
        }

        // 1. 建立 Unit 父子關係與階層路徑 (e.g. "machine/st1/cyl1")
        let mut parent_of: BTreeMap<&str, &str> = BTreeMap::new();
        for u in &machine.units {
            if let Some(entry) = catalog.get(&u.unit_type) {
                for part_name in entry.def.parts.keys() {
                    if let Some(Some(child_id)) = u.bind.get(part_name) {
                        parent_of.insert(child_id.as_str(), u.id.as_str());
                    }
                }
            }
        }

        let compute_unit_path = |uid: &str| -> String {
            let mut chain = vec![uid];
            let mut curr = uid;
            while let Some(&p) = parent_of.get(curr) {
                chain.push(p);
                curr = p;
            }
            chain.reverse();
            chain.join("/")
        };

        let mut unit_tree = Vec::new();
        let mut used_unit_types = BTreeMap::new();

        for u in &machine.units {
            let path = compute_unit_path(&u.id);
            let parent_id = parent_of.get(u.id.as_str()).map(|s| (*s).to_string());
            if let Some(entry) = catalog.get(&u.unit_type) {
                used_unit_types.insert(
                    u.unit_type.clone(),
                    UsedTypeVersionHash {
                        version: entry.def.version,
                        content_hash: entry.content_hash.clone(),
                        origin: entry.origin,
                    },
                );
            }
            unit_tree.push(ResolvedUnitNode {
                id: u.id.clone(),
                unit_type: u.unit_type.clone(),
                type_version: u.type_version,
                parent_id,
                unit_path: path,
                bind: u.bind.clone(),
            });
        }

        // 2. 收集 Tag 綁定資訊、par_ 屬性與 HMI 導出清單
        struct PinBindingEffect {
            binding_ref: TagBindingRef,
            inferred_role: Option<Role>,
            write_candidate: bool,
            par_meta: Option<(f64, f64, f64, Option<String>)>, // (min, max, default, unit)
        }

        let mut tag_effects: BTreeMap<String, Vec<PinBindingEffect>> = BTreeMap::new();
        let mut lists = DerivedCatalogs::default();
        let mut hmi_coverage = Vec::new();

        for u in &machine.units {
            let unit_path = compute_unit_path(&u.id);
            let entry = catalog.get(&u.unit_type).expect("已通過 U2 驗證");
            let ut = &entry.def;

            // Field 接腳
            for (dir_cat, map) in [("field.in", &ut.field.inputs), ("field.out", &ut.field.outputs)] {
                for pin_name in map.keys() {
                    if let Some(Some(tag_id)) = u.bind.get(pin_name) {
                        tag_effects.entry(tag_id.clone()).or_default().push(PinBindingEffect {
                            binding_ref: TagBindingRef {
                                unit_id: u.id.clone(),
                                unit_path: unit_path.clone(),
                                pin_name: pin_name.clone(),
                                category: dir_cat.to_string(),
                            },
                            inferred_role: None,
                            write_candidate: false,
                            par_meta: None,
                        });
                    }
                }
            }

            // Ctrl 接腳與 HMI 覆蓋率
            let mut bound_ctrl_pins = Vec::new();
            let mut unbound_optional_ctrl_pins = Vec::new();

            for (pin_name, pin_def) in &ut.ctrl {
                let (parsed_pin, spec) = parse_ctrl_pin_name(pin_name).expect("已通過 SC3 驗證");
                let eff_dt = resolve_ctrl_pin_type(pin_name, pin_def.data_type.as_deref(), spec, pin_name)
                    .expect("已通過 SC3 驗證");
                let resolved_label = pin_def
                    .label
                    .clone()
                    .or_else(|| default_feature_label(&parsed_pin.full_suffix, &parsed_pin.feature))
                    .unwrap_or_else(|| pin_name.clone());

                match u.bind.get(pin_name) {
                    Some(Some(tag_id)) => {
                        bound_ctrl_pins.push(pin_name.clone());
                        let par_meta = if parsed_pin.prefix == CtrlPrefix::Par {
                            Some((
                                pin_def.min.unwrap_or(0.0),
                                pin_def.max.unwrap_or(0.0),
                                pin_def.default.unwrap_or(0.0),
                                pin_def.unit.clone(),
                            ))
                        } else {
                            None
                        };

                        tag_effects.entry(tag_id.clone()).or_default().push(PinBindingEffect {
                            binding_ref: TagBindingRef {
                                unit_id: u.id.clone(),
                                unit_path: unit_path.clone(),
                                pin_name: pin_name.clone(),
                                category: parsed_pin.prefix.as_str().to_string(),
                            },
                            inferred_role: Some(spec.inferred_role),
                            write_candidate: spec.write_candidate,
                            par_meta,
                        });

                        // 依前綴分類加入 HMI 清單 (§9.2, §10)
                        match parsed_pin.prefix {
                            CtrlPrefix::Man => {
                                let feat = &parsed_pin.full_suffix;
                                let ena_name = format!("ena_{}", feat);
                                let at_name = format!("at_{}", feat);
                                let sts_name = format!("sts_{}", feat);
                                lists.operations.push(DerivedOperationItem {
                                    unit_id: u.id.clone(),
                                    unit_path: unit_path.clone(),
                                    feature: feat.clone(),
                                    pin_name: pin_name.clone(),
                                    prefix_category: "man".to_string(),
                                    label: resolved_label,
                                    man_tag: tag_id.clone(),
                                    ena_pin: ut.ctrl.contains_key(&ena_name).then_some(ena_name),
                                    at_tag: u.bind.get(&at_name).and_then(|v| v.clone()),
                                    sts_tag: u.bind.get(&sts_name).and_then(|v| v.clone()),
                                });
                            }
                            CtrlPrefix::Error => {
                                lists.alarms.push(DerivedAlarmItem {
                                    unit_id: u.id.clone(),
                                    unit_path: unit_path.clone(),
                                    pin_name: pin_name.clone(),
                                    prefix_category: "error".to_string(),
                                    tag_id: tag_id.clone(),
                                    label: resolved_label,
                                    message: pin_def.message.clone().unwrap_or_default(),
                                });
                            }
                            CtrlPrefix::Par => {
                                lists.parameters.push(DerivedParameterItem {
                                    unit_id: u.id.clone(),
                                    unit_path: unit_path.clone(),
                                    pin_name: pin_name.clone(),
                                    prefix_category: "par".to_string(),
                                    tag_id: tag_id.clone(),
                                    label: resolved_label,
                                    data_type: eff_dt.to_string(),
                                    unit: pin_def.unit.clone(),
                                    min: pin_def.min.unwrap_or(0.0),
                                    max: pin_def.max.unwrap_or(0.0),
                                    default: pin_def.default.unwrap_or(0.0),
                                });
                            }
                            CtrlPrefix::Pv | CtrlPrefix::Count => {
                                lists.values.push(DerivedValueOrMsgItem {
                                    unit_id: u.id.clone(),
                                    unit_path: unit_path.clone(),
                                    pin_name: pin_name.clone(),
                                    prefix_category: parsed_pin.prefix.as_str().to_string(),
                                    tag_id: tag_id.clone(),
                                    label: resolved_label,
                                    data_type: eff_dt.to_string(),
                                    unit: pin_def.unit.clone(),
                                });
                            }
                            CtrlPrefix::Msg => {
                                lists.messages.push(DerivedValueOrMsgItem {
                                    unit_id: u.id.clone(),
                                    unit_path: unit_path.clone(),
                                    pin_name: pin_name.clone(),
                                    prefix_category: "msg".to_string(),
                                    tag_id: tag_id.clone(),
                                    label: resolved_label,
                                    data_type: eff_dt.to_string(),
                                    unit: pin_def.unit.clone(),
                                });
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        if spec.binding_policy == BindingPolicy::Optional {
                            unbound_optional_ctrl_pins.push(pin_name.clone());
                        }
                    }
                }
            }

            hmi_coverage.push(UnitHmiCoverage {
                unit_id: u.id.clone(),
                unit_type: u.unit_type.clone(),
                bound_ctrl_pins,
                unbound_optional_ctrl_pins,
            });
        }

        // 3. 計算有效 Tag 表 (§6.1 role, §6.2 write_candidate, §9.2 EffectiveTag)
        let plc_families: BTreeMap<&str, PlcFamily> = machine
            .machine
            .plcs
            .iter()
            .map(|p| (p.id.as_str(), p.family))
            .collect();

        let mut effective_tags = Vec::new();
        for tag in &machine.tags {
            let addr_ro = tag
                .id
                .split_once('.')
                .and_then(|(plc_id, _)| plc_families.get(plc_id))
                .and_then(|&fam| parse_plc_address(fam, &tag.address).ok())
                .map(|a| a.category.is_read_only())
                .unwrap_or(false);

            let effects = tag_effects.remove(&tag.id).unwrap_or_default();
            let bindings: Vec<TagBindingRef> = effects.iter().map(|e| e.binding_ref.clone()).collect();

            // §6.1 role 推導優先序：Tag 明確 role > 前綴推測 role > 結構推測 (field 腳無 role)
            let (role, role_reason) = if let Some(exp_role) = tag.role {
                (Some(exp_role), format!("由 Tag 明確指定 role: {}", exp_role))
            } else if let Some(first_eff) = effects.first() {
                if let Some(inf_role) = first_eff.inferred_role {
                    (
                        Some(inf_role),
                        format!(
                            "由綁定接腳 '{}.{}' (前綴 '{}') 推導",
                            first_eff.binding_ref.unit_id,
                            first_eff.binding_ref.pin_name,
                            first_eff.binding_ref.category
                        ),
                    )
                } else {
                    (
                        None,
                        format!(
                            "綁定至實體接腳 '{}.{}' ({})，不具控制 role",
                            first_eff.binding_ref.unit_id,
                            first_eff.binding_ref.pin_name,
                            first_eff.binding_ref.category
                        ),
                    )
                }
            } else {
                (None, "未被任何 Unit 綁定".to_string())
            };

            // §6.2 write_candidate 推導
            let (write_candidate, write_candidate_reason) = if let Some(cand_eff) =
                effects.iter().find(|e| e.write_candidate)
            {
                (
                    true,
                    format!(
                        "綁定至可寫候選接腳 '{}.{}' ({}_*)",
                        cand_eff.binding_ref.unit_id,
                        cand_eff.binding_ref.pin_name,
                        cand_eff.binding_ref.category
                    ),
                )
            } else if effects.is_empty() {
                (false, "未被任何 Unit 綁定（預設 fail-closed 不可寫）".to_string())
            } else {
                let first = &effects[0].binding_ref;
                (
                    false,
                    format!("綁定至唯讀／輸出接腳 '{}.{}' ({})", first.unit_id, first.pin_name, first.category),
                )
            };

            // 合併來自 par_ 接腳的 min/max/default/unit (§9.2, §9.3)
            let par_meta = effects.iter().find_map(|e| e.par_meta.clone());
            let min = tag.min.or_else(|| par_meta.as_ref().map(|m| m.0));
            let max = tag.max.or_else(|| par_meta.as_ref().map(|m| m.1));
            let default = tag
                .default
                .clone()
                .or_else(|| par_meta.as_ref().map(|m| ScalarValue::Number(m.2)));
            let unit = tag
                .unit
                .clone()
                .or_else(|| par_meta.and_then(|m| m.3));

            effective_tags.push(EffectiveTag {
                id: tag.id.clone(),
                address: tag.address.clone(),
                data_type: tag.data_type.clone(),
                scan: tag.scan,
                scale: tag.scale,
                offset: tag.offset,
                deadband: tag.deadband,
                unit,
                readback: tag.readback,
                retain: tag.retain,
                safety_monitor: tag.safety_monitor,
                role,
                role_reason,
                write_candidate,
                write_candidate_reason,
                min,
                max,
                default,
                address_read_only: addr_ro,
                bindings,
            });
        }

        let canonical_yaml = machine.to_yaml_string().unwrap_or_default();
        let fingerprint = ModelFingerprint {
            machine_yaml_hash: fnv1a_64_hex(&canonical_yaml),
            used_unit_types,
        };

        Ok(ResolvedMachineModel {
            machine_id: machine.machine.id.clone(),
            effective_tags,
            unit_tree,
            lists,
            hmi_coverage,
            fingerprint,
            diagnostics: diags,
        })
    }
}

// ============================================================================
// 9. 降階轉換器（匯出層 §7：樹狀 IR -> 頂層 FB 實例 + ctrl STRUCT 傳遞）
// ============================================================================

pub mod lowering {
    use super::datatype::DataType;
    use super::model::MachineFile;
    use super::naming::{PinDirection, parse_ctrl_pin_name, resolve_ctrl_pin_type};
    use super::resolver::ResolvedMachineModel;
    use super::unit_type::TypeCatalog;
    use super::*;

    /// 降階後之單一 `ctrl` 結構欄位
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LoweredStructField {
        pub pin_name: String,
        pub direction: PinDirection,
        pub iec_type: String,
    }

    /// 降階後之 `<unit_type>_ctrl_t` STRUCT 宣告 (§7)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LoweredCtrlStruct {
        pub struct_name: String,
        pub unit_type: String,
        pub fields: Vec<LoweredStructField>,
    }

    /// 提到頂層宣告之 FB 實例 (§7)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct HoistedUnitInstance {
        pub unit_id: String,
        pub unit_type: String,
        pub ctrl_struct_instance: String,
        pub has_field_pins: bool,
        pub field_in_bindings: BTreeMap<String, String>,
        pub field_out_bindings: BTreeMap<String, String>,
        pub ctrl_tag_bindings: BTreeMap<String, (PinDirection, String)>,
        /// 父層接收子層 `ctrl` 結構的 `VAR_IN_OUT` 連線：`part_name -> child_ctrl_struct_instance`
        pub part_struct_links: BTreeMap<String, String>,
    }

    /// 降階匯出結果 (§7)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct LoweredStExport {
        pub ctrl_structs: Vec<LoweredCtrlStruct>,
        pub top_level_instances: Vec<HoistedUnitInstance>,
        pub st_code_preview: String,
    }

    fn to_iec_type_name(dt: &DataType) -> String {
        match dt {
            DataType::Scalar(s) => match s {
                super::datatype::ScalarType::Bool => "BOOL".to_string(),
                super::datatype::ScalarType::Int16 => "INT".to_string(),
                super::datatype::ScalarType::Uint16 => "UINT".to_string(),
                super::datatype::ScalarType::Int32 => "DINT".to_string(),
                super::datatype::ScalarType::Uint32 => "UDINT".to_string(),
                super::datatype::ScalarType::Int64 => "LINT".to_string(),
                super::datatype::ScalarType::Uint64 => "ULINT".to_string(),
                super::datatype::ScalarType::Float32 => "REAL".to_string(),
                super::datatype::ScalarType::Float64 => "LREAL".to_string(),
                super::datatype::ScalarType::Text(n) => format!("STRING[{}]", n),
            },
            DataType::Array { elem, len } => {
                format!("ARRAY[0..{}] OF {}", len.saturating_sub(1), to_iec_type_name(&DataType::Scalar(elem.clone())))
            }
        }
    }

    /// 將已解析的模型執行 §7 降階轉換：
    /// - 每個使用的 Unit 類型由 `ctrl` 腳機械導出 `<type>_ctrl_t` STRUCT；
    /// - 帶 `field` 腳的 Unit 提到頂層宣告；
    /// - 父層（station/machine）以 `VAR_IN_OUT` 接收子層 `ctrl` 結構。
    pub fn lower_to_iec_st(
        resolved: &ResolvedMachineModel,
        machine: &MachineFile,
        catalog: &TypeCatalog,
    ) -> LoweredStExport {
        let mut ctrl_structs = Vec::new();

        for type_id in resolved.fingerprint.used_unit_types.keys() {
            if let Some(entry) = catalog.get(type_id) {
                let mut fields = Vec::new();
                for (pin_name, pin_def) in &entry.def.ctrl {
                    if let Ok((_, spec)) = parse_ctrl_pin_name(pin_name) {
                        if let Ok(dt) = resolve_ctrl_pin_type(pin_name, pin_def.data_type.as_deref(), spec, pin_name) {
                            fields.push(LoweredStructField {
                                pin_name: pin_name.clone(),
                                direction: spec.direction,
                                iec_type: to_iec_type_name(&dt),
                            });
                        }
                    }
                }
                ctrl_structs.push(LoweredCtrlStruct {
                    struct_name: format!("{}_ctrl_t", type_id),
                    unit_type: type_id.clone(),
                    fields,
                });
            }
        }

        let mut top_level_instances = Vec::new();
        for unit in &machine.units {
            let Some(entry) = catalog.get(&unit.unit_type) else {
                continue;
            };
            let ut = &entry.def;
            let has_field_pins = !ut.field.inputs.is_empty() || !ut.field.outputs.is_empty();

            let mut field_in_bindings = BTreeMap::new();
            for pin_name in ut.field.inputs.keys() {
                if let Some(Some(tag_id)) = unit.bind.get(pin_name) {
                    field_in_bindings.insert(pin_name.clone(), tag_id.clone());
                }
            }

            let mut field_out_bindings = BTreeMap::new();
            for pin_name in ut.field.outputs.keys() {
                if let Some(Some(tag_id)) = unit.bind.get(pin_name) {
                    field_out_bindings.insert(pin_name.clone(), tag_id.clone());
                }
            }

            let mut ctrl_tag_bindings = BTreeMap::new();
            for (pin_name, _) in &ut.ctrl {
                if let Ok((_, spec)) = parse_ctrl_pin_name(pin_name) {
                    if let Some(Some(tag_id)) = unit.bind.get(pin_name) {
                        ctrl_tag_bindings.insert(pin_name.clone(), (spec.direction, tag_id.clone()));
                    }
                }
            }

            let mut part_struct_links = BTreeMap::new();
            for part_name in ut.parts.keys() {
                if let Some(Some(child_unit_id)) = unit.bind.get(part_name) {
                    part_struct_links.insert(part_name.clone(), format!("st_{}_ctrl", child_unit_id));
                }
            }

            top_level_instances.push(HoistedUnitInstance {
                unit_id: unit.id.clone(),
                unit_type: unit.unit_type.clone(),
                ctrl_struct_instance: format!("st_{}_ctrl", unit.id),
                has_field_pins,
                field_in_bindings,
                field_out_bindings,
                ctrl_tag_bindings,
                part_struct_links,
            });
        }

        // 產生 IEC 61131-3 ST 降階預覽碼
        let mut st_lines = Vec::new();
        st_lines.push("(* === 自動產生之 Unit ctrl 結構宣告 (§7 降階匯出) === *)".to_string());
        for cs in &ctrl_structs {
            st_lines.push(format!("TYPE {} :", cs.struct_name));
            st_lines.push("  STRUCT".to_string());
            for f in &cs.fields {
                let dir_comment = match f.direction {
                    PinDirection::In => "in",
                    PinDirection::Out => "out",
                };
                st_lines.push(format!("    {} : {}; (* {} *)", f.pin_name, f.iec_type, dir_comment));
            }
            st_lines.push("  END_STRUCT".to_string());
            st_lines.push("END_TYPE\n".to_string());
        }

        st_lines.push("(* === 頂層宣告與實例化 === *)".to_string());
        st_lines.push("VAR".to_string());
        for inst in &top_level_instances {
            st_lines.push(format!("  fb_{} : FB_{};", inst.unit_id, inst.unit_type));
            st_lines.push(format!("  {} : {}_ctrl_t;", inst.ctrl_struct_instance, inst.unit_type));
        }
        st_lines.push("END_VAR\n".to_string());

        st_lines.push("(* === 週期掃描與綁定呼叫 === *)".to_string());
        for inst in &top_level_instances {
            for (pin, (dir, tag_id)) in &inst.ctrl_tag_bindings {
                if *dir == PinDirection::In {
                    st_lines.push(format!("{}.{} := {};", inst.ctrl_struct_instance, pin, tag_id));
                }
            }
            let mut call_args = vec![format!("io_ctrl := {}", inst.ctrl_struct_instance)];
            for (pin, tag_id) in &inst.field_in_bindings {
                call_args.push(format!("{} := {}", pin, tag_id));
            }
            for (pin, tag_id) in &inst.field_out_bindings {
                call_args.push(format!("{} => {}", pin, tag_id));
            }
            for (part_name, child_struct) in &inst.part_struct_links {
                call_args.push(format!("{} := {}", part_name, child_struct));
            }
            st_lines.push(format!("fb_{}({});", inst.unit_id, call_args.join(", ")));
            for (pin, (dir, tag_id)) in &inst.ctrl_tag_bindings {
                if *dir == PinDirection::Out {
                    st_lines.push(format!("{} := {}.{};", tag_id, inst.ctrl_struct_instance, pin));
                }
            }
        }

        LoweredStExport {
            ctrl_structs,
            top_level_instances,
            st_code_preview: st_lines.join("\n"),
        }
    }
}

// ============================================================================
// 10. PlcDataService 最終 `writable` 計算與護欄驗證 (§9.3)
// ============================================================================

pub mod datasvc {
    use super::diagnostics::Diagnostic;
    use super::resolver::ResolvedMachineModel;
    use super::*;

    /// `service.toml` 中的單筆 `[[tag_override]]` (§9.3)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct TagOverrideEntry {
        pub id: String,
        pub writable: bool,
    }

    /// 最終 `writable` 來源標記 (§9.3 護欄 4)
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum WritableSource {
        Model,
        Override,
    }

    impl fmt::Display for WritableSource {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                WritableSource::Model => write!(f, "model"),
                WritableSource::Override => write!(f, "override"),
            }
        }
    }

    /// PlcDataService 單一點位最終可寫決策項目
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct FinalWritableItem {
        pub tag_id: String,
        pub address: String,
        pub write_candidate: bool,
        pub final_writable: bool,
        pub source: WritableSource,
        /// 「最終可寫但不是模型候選」的 Tag，供設計期人工審查 (§9.3)
        pub requires_human_review: bool,
    }

    /// `write-report` 輸出結構 (§9.3, §9.5)
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct WriteReport {
        pub items: Vec<FinalWritableItem>,
        /// 列出「最終可寫但不是模型候選 (`final_writable && !write_candidate`)」的 Tag ID 清單
        pub review_required_tags: Vec<String>,
        pub guardrail_errors: Vec<Diagnostic>,
    }

    /// 簡易解析 `service.toml` 中的 `[[tag_override]]` 區塊（零額外依賴）
    pub fn parse_service_toml_overrides(toml_content: &str) -> Result<Vec<TagOverrideEntry>, String> {
        let mut overrides = Vec::new();
        let mut in_override_block = false;
        let mut current_id: Option<String> = None;
        let mut current_writable: Option<bool> = None;

        let flush_entry = |id: &mut Option<String>,
                           writable: &mut Option<bool>,
                           out: &mut Vec<TagOverrideEntry>|
         -> Result<(), String> {
            if id.is_some() || writable.is_some() {
                let Some(i) = id.take() else {
                    return Err("[[tag_override]] 缺少 'id' 欄位".to_string());
                };
                let Some(w) = writable.take() else {
                    return Err(format!("[[tag_override]] ('{}') 缺少 'writable' 欄位", i));
                };
                out.push(TagOverrideEntry { id: i, writable: w });
            }
            Ok(())
        };

        for raw_line in toml_content.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                flush_entry(&mut current_id, &mut current_writable, &mut overrides)?;
                in_override_block = line == "[[tag_override]]";
                continue;
            }
            if in_override_block {
                if let Some((k, v)) = line.split_once('=') {
                    let key = k.trim();
                    let val = v.trim();
                    match key {
                        "id" => {
                            current_id = Some(val.trim_matches('"').trim_matches('\'').to_string());
                        }
                        "writable" => {
                            let b = match val {
                                "true" => true,
                                "false" => false,
                                _ => return Err(format!("writable 值 '{}' 必須為 true 或 false", val)),
                            };
                            current_writable = Some(b);
                        }
                        _ => {}
                    }
                }
            }
        }
        flush_entry(&mut current_id, &mut current_writable, &mut overrides)?;
        Ok(overrides)
    }

    /// 執行 PlcDataService 最終 `writable` 計算與四大護欄檢查 (§9.3)
    pub fn evaluate_plc_data_service_writability(
        resolved: &ResolvedMachineModel,
        overrides: &[TagOverrideEntry],
    ) -> WriteReport {
        let mut guardrail_errors = Vec::new();
        let tag_lookup: BTreeMap<&str, &super::resolver::EffectiveTag> = resolved
            .effective_tags
            .iter()
            .map(|t| (t.id.as_str(), t))
            .collect();

        let mut valid_overrides: BTreeMap<&str, bool> = BTreeMap::new();

        for (idx, ov) in overrides.iter().enumerate() {
            let path = format!("service.toml:tag_override[{}]({})", idx, ov.id);
            // 護欄 3: 覆寫引用不存在的 Tag -> 啟動錯誤
            let Some(&eff_tag) = tag_lookup.get(ov.id.as_str()) else {
                guardrail_errors.push(Diagnostic::error(
                    "DS3",
                    &path,
                    format!("tag_override 引用了模型中不存在的 Tag '{}'", ov.id),
                ));
                continue;
            };

            // 護欄 1: 覆寫不能讓唯讀位址類別（X 等）的 Tag 變可寫
            if ov.writable && eff_tag.address_read_only {
                guardrail_errors.push(Diagnostic::error(
                    "DS1",
                    &path,
                    format!(
                        "護欄拒絕覆寫：Tag '{}' 位於唯讀輸入位址類別 ({})，不得被覆寫為 writable = true",
                        ov.id, eff_tag.address
                    ),
                ));
                continue;
            }

            // 護欄 2: safety_monitor: true 的 Tag 不能被覆寫為可寫
            if ov.writable && eff_tag.safety_monitor {
                guardrail_errors.push(Diagnostic::error(
                    "DS2",
                    &path,
                    format!(
                        "護欄拒絕覆寫：Tag '{}' 設定為 safety_monitor: true，不得被覆寫為 writable = true",
                        ov.id
                    ),
                ));
                continue;
            }

            valid_overrides.insert(ov.id.as_str(), ov.writable);
        }

        let mut items = Vec::new();
        let mut review_required_tags = Vec::new();

        for eff_tag in &resolved.effective_tags {
            let (final_writable, source) = match valid_overrides.get(eff_tag.id.as_str()) {
                Some(&ov_val) => (ov_val, WritableSource::Override),
                None => (eff_tag.write_candidate, WritableSource::Model),
            };
            let requires_human_review = final_writable && !eff_tag.write_candidate;
            if requires_human_review {
                review_required_tags.push(eff_tag.id.clone());
            }
            items.push(FinalWritableItem {
                tag_id: eff_tag.id.clone(),
                address: eff_tag.address.clone(),
                write_candidate: eff_tag.write_candidate,
                final_writable,
                source,
                requires_human_review,
            });
        }

        WriteReport {
            items,
            review_required_tags,
            guardrail_errors,
        }
    }
}

// ============================================================================
// 11. 命令列工具 `machinemodel` (§9.5)
// ============================================================================

pub mod cli {
    use super::datasvc::{evaluate_plc_data_service_writability, parse_service_toml_overrides};
    use super::diagnostics::has_errors;
    use super::lowering::lower_to_iec_st;
    use super::model::MachineFile;
    use super::resolver::resolve_machine_model;
    use super::unit_type::{TypeCatalog, generate_unit_type_json_schema};
    use super::validator::{validate_catalog, validate_machine_with_catalog};
    use super::*;

    /// CLI 退出碼 (§9.5：0 通過、1 有錯誤、2 用法或檔案讀寫錯誤)
    pub const EXIT_OK: i32 = 0;
    pub const EXIT_VALIDATION_ERROR: i32 = 1;
    pub const EXIT_USAGE_OR_IO_ERROR: i32 = 2;

    fn load_machine_and_catalog(machine_path: &Path) -> Result<(MachineFile, TypeCatalog), (i32, String)> {
        let yaml = fs::read_to_string(machine_path).map_err(|e| {
            (
                EXIT_USAGE_OR_IO_ERROR,
                format!("無法讀取檔案 '{}': {}", machine_path.display(), e),
            )
        })?;
        let machine = MachineFile::from_yaml_str(&yaml).map_err(|diag| {
            (EXIT_VALIDATION_ERROR, format!("{}", diag))
        })?;
        let mut catalog = TypeCatalog::with_embedded_standards();
        if let Some(parent_dir) = machine_path.parent() {
            catalog.load_local_types_from_dir(parent_dir);
        }
        Ok((machine, catalog))
    }

    /// 執行 CLI 子命令 (§9.5：check, dump, check-types, write-report, lower-st, emit-schema)
    pub fn run_cli(args: &[String]) -> (i32, String) {
        if args.len() < 2 {
            return (
                EXIT_USAGE_OR_IO_ERROR,
                "用法: machinemodel <check|dump|check-types|write-report|lower-st|emit-schema|debugger> [參數...]".to_string(),
            );
        }

        match args[1].as_str() {
            "check" => {
                let Some(path_str) = args.get(2) else {
                    return (EXIT_USAGE_OR_IO_ERROR, "用法: machinemodel check <machine.yaml>".to_string());
                };
                let path = Path::new(path_str);
                let (machine, catalog) = match load_machine_and_catalog(path) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let diags = validate_machine_with_catalog(&machine, &catalog);
                let mut out = Vec::new();
                for d in &diags {
                    out.push(format!("{}", d));
                }
                if has_errors(&diags) {
                    (EXIT_VALIDATION_ERROR, out.join("\n"))
                } else {
                    if let Ok(resolved) = resolve_machine_model(&machine, &catalog) {
                        out.push(format!(
                            "檢查通過：共 {} 個有效 Tag，{} 個 Unit 實例（指紋：{}）",
                            resolved.effective_tags.len(),
                            resolved.unit_tree.len(),
                            resolved.fingerprint.machine_yaml_hash
                        ));
                    }
                    (EXIT_OK, out.join("\n"))
                }
            }
            "dump" => {
                let Some(path_str) = args.get(2) else {
                    return (EXIT_USAGE_OR_IO_ERROR, "用法: machinemodel dump <machine.yaml>".to_string());
                };
                let path = Path::new(path_str);
                let (machine, catalog) = match load_machine_and_catalog(path) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                match resolve_machine_model(&machine, &catalog) {
                    Ok(resolved) => match serde_yaml_ng::to_string(&resolved) {
                        Ok(yaml) => (EXIT_OK, yaml),
                        Err(e) => (EXIT_USAGE_OR_IO_ERROR, e.to_string()),
                    },
                    Err(diags) => {
                        let lines: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
                        (EXIT_VALIDATION_ERROR, lines.join("\n"))
                    }
                }
            }
            "check-types" => {
                let mut catalog = TypeCatalog::with_embedded_standards();
                if let Some(dir_str) = args.get(2) {
                    catalog.load_local_types_from_dir(Path::new(dir_str));
                }
                let diags = validate_catalog(&catalog);
                let lines: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
                if has_errors(&diags) {
                    (EXIT_VALIDATION_ERROR, lines.join("\n"))
                } else {
                    (
                        EXIT_OK,
                        format!("類型目錄檢查通過（共 {} 個 Unit 類型）\n{}", catalog.entries.len(), lines.join("\n"))
                            .trim()
                            .to_string(),
                    )
                }
            }
            "write-report" => {
                let Some(machine_path_str) = args.get(2) else {
                    return (
                        EXIT_USAGE_OR_IO_ERROR,
                        "用法: machinemodel write-report <machine.yaml> --service <service.toml>".to_string(),
                    );
                };
                let service_idx = args.iter().position(|a| a == "--service");
                let Some(service_path_str) = service_idx.and_then(|i| args.get(i + 1)) else {
                    return (
                        EXIT_USAGE_OR_IO_ERROR,
                        "缺少 --service <service.toml> 參數".to_string(),
                    );
                };
                let (machine, catalog) = match load_machine_and_catalog(Path::new(machine_path_str)) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                let resolved = match resolve_machine_model(&machine, &catalog) {
                    Ok(r) => r,
                    Err(diags) => {
                        let lines: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
                        return (EXIT_VALIDATION_ERROR, lines.join("\n"));
                    }
                };
                let toml_str = match fs::read_to_string(service_path_str) {
                    Ok(s) => s,
                    Err(e) => {
                        return (
                            EXIT_USAGE_OR_IO_ERROR,
                            format!("無法讀取 service.toml '{}': {}", service_path_str, e),
                        )
                    }
                };
                let overrides = match parse_service_toml_overrides(&toml_str) {
                    Ok(ov) => ov,
                    Err(e) => return (EXIT_USAGE_OR_IO_ERROR, e),
                };
                let report = evaluate_plc_data_service_writability(&resolved, &overrides);
                let yaml = serde_yaml_ng::to_string(&report).unwrap_or_default();
                if has_errors(&report.guardrail_errors) {
                    (EXIT_VALIDATION_ERROR, yaml)
                } else {
                    (EXIT_OK, yaml)
                }
            }
            "lower-st" => {
                let Some(path_str) = args.get(2) else {
                    return (EXIT_USAGE_OR_IO_ERROR, "用法: machinemodel lower-st <machine.yaml>".to_string());
                };
                let (machine, catalog) = match load_machine_and_catalog(Path::new(path_str)) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                match resolve_machine_model(&machine, &catalog) {
                    Ok(resolved) => {
                        let lowered = lower_to_iec_st(&resolved, &machine, &catalog);
                        (EXIT_OK, lowered.st_code_preview)
                    }
                    Err(diags) => {
                        let lines: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
                        (EXIT_VALIDATION_ERROR, lines.join("\n"))
                    }
                }
            }
            "emit-schema" => (EXIT_OK, generate_unit_type_json_schema()),
            "debugger" | "demo" => {
                let html_path = "machinemodel_v0_3_demo_debugger.html";
                (
                    EXIT_OK,
                    format!(
                        "MachineModel v0.3 Demo Debugger 已就緒！\n檔案路徑: {}\n請在瀏覽器中直接開啟此 HTML 檔案以使用視覺化互動偵錯器（支援 I/O 表批次匯入、Tag 管理、Unit 樹綁定、標準庫管理、即時 SC/R/U 規則診斷、YAML 匯出、ST 降階、字體縮放與深淺色主題切換）。",
                        html_path
                    ),
                )
            }
            other => (
                EXIT_USAGE_OR_IO_ERROR,
                format!("未知的子命令 '{}'（支援：check, dump, check-types, write-report, lower-st, emit-schema, debugger）", other),
            ),
        }
    }
}


// ============================================================================
// 12. 單元測試：涵蓋 SC1~SC4、R1~R28、U1~U19、降階與 PlcDataService 護欄
// ============================================================================

#[cfg(test)]
mod tests {
    use super::datasvc::{TagOverrideEntry, WritableSource, evaluate_plc_data_service_writability};
    use super::datatype::DataType;
    use super::family::PlcFamily;
    use super::lowering::lower_to_iec_st;
    use super::model::{MachineFile, MachineModelBuilder, UnitInstanceBuilder};
    use super::naming::Role;
    use super::resolver::resolve_machine_model;
    use super::unit_type::{
        STD_CYLINDER_DOUBLE_YAML, STD_MACHINE_CTRL_YAML, STD_STATION_LOAD_YAML, TypeCatalog,
        UnitTypeDef, fnv1a_64_hex,
    };
    use super::validator::{validate_machine_with_catalog, validate_unit_type};
    use std::collections::BTreeSet;

    fn build_valid_demo_machine() -> MachineFile {
        MachineModelBuilder::new("demo")
            .title("示範貼標上料機")
            .add_plc("plc1", PlcFamily::InovanceEasyTcp)
            .add_simple_tag("plc1.X_Cyl1_Ext_LS", "X010", "bool")
            .add_simple_tag("plc1.Y_Cyl1_Ext", "Y010", "bool")
            .add_simple_tag("plc1.M_Cyl1_ManExt", "M100", "bool")
            .add_simple_tag("plc1.M_Cyl1_ErrExt", "M200", "bool")
            .add_simple_tag("plc1.M_Cyl1_ErrRet", "M201", "bool")
            .add_simple_tag("plc1.D_Cyl1_ExtTime", "D100", "int16")
            .add_simple_tag("plc1.M_St1_ErrJam", "M210", "bool")
            .add_simple_tag("plc1.M_Mach_ErrEstop", "M220", "bool")
            .add_simple_tag("plc1.D_Mach_ManMode", "D200", "int16")
            .add_unit(
                UnitInstanceBuilder::new("cyl1", "cylinder_double", 1)
                    .bind_target("lim_ext", "plc1.X_Cyl1_Ext_LS")
                    .bind_null("lim_ret")
                    .bind_target("valve_ext", "plc1.Y_Cyl1_Ext")
                    .bind_null("valve_ret")
                    .bind_target("man_ext", "plc1.M_Cyl1_ManExt")
                    .bind_target("par_ext_time", "plc1.D_Cyl1_ExtTime")
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
            .build()
    }

    #[test]
    fn test_standard_type_version_hash_pinning() {
        // §9.1: 專案測試以內容雜湊釘住每個標準類型的 version
        assert_eq!(fnv1a_64_hex(STD_CYLINDER_DOUBLE_YAML).len(), 16);
        assert_eq!(fnv1a_64_hex(STD_STATION_LOAD_YAML).len(), 16);
        assert_eq!(fnv1a_64_hex(STD_MACHINE_CTRL_YAML).len(), 16);
    }

    #[test]
    fn scalar_type_is_the_shared_one() {
        let a: plc_common::ScalarType = crate::datatype::ScalarType::Int16;
        assert_eq!(a, plc_common::ScalarType::Int16);
    }

    #[test]
    fn string_type_cap_is_256() {
        use super::datatype::ScalarType;
        assert!(ScalarType::parse("string:256").is_ok());
        assert!(ScalarType::parse("string:257").is_err());
    }

    #[test]
    fn test_datatype_forbids_string_array_sc3() {
        // §3.2: 除 string:N 外，標量可加 [n] 陣列（n 為 1～500）；字串陣列（string:N[n]）禁止
        assert!(DataType::parse("int16[10]", "test").is_ok());
        assert!(DataType::parse("string:32", "test").is_ok());
        let err = DataType::parse("string:32[4]", "test").unwrap_err();
        assert_eq!(err.rule, "SC3");
        let err_len = DataType::parse("int16[501]", "test").unwrap_err();
        assert_eq!(err_len.rule, "SC3");
    }

    #[test]
    fn test_sc4_rejects_legacy_v1_and_v02_fields() {
        // §3.2 & §4.1: 寫了 writable, write_permission, unit_id 或接腳 kind/level/reset/role 即為未知欄位 (SC4)
        let bad_machine = r#"
schema_version: 2
machine:
  id: m1
  plcs: [{ id: plc1, family: inovance_easy }]
tags:
  - { id: plc1.T1, address: M0, type: bool, writable: true }
units: []
"#;
        let err = MachineFile::from_yaml_str(bad_machine).unwrap_err();
        assert_eq!(err.rule, "SC4");

        let bad_unit_type = r#"
id: bad_cyl
version: 1
title: 錯誤氣缸
ctrl:
  error_ext: { message: 逾時, level: major }
"#;
        let err_ut = UnitTypeDef::from_yaml_str(bad_unit_type, "bad_cyl.yaml").unwrap_err();
        assert_eq!(err_ut.rule, "SC4");
    }

    #[test]
    fn test_resolve_and_lower_valid_demo_machine() {
        let machine = build_valid_demo_machine();
        let catalog = TypeCatalog::with_embedded_standards();
        let resolved = resolve_machine_model(&machine, &catalog).expect("應通過所有驗證");

        // 檢查推導結果 (§6.1, §6.2)
        let man_ext = resolved
            .effective_tags
            .iter()
            .find(|t| t.id == "plc1.M_Cyl1_ManExt")
            .unwrap();
        assert_eq!(man_ext.role, Some(Role::Command));
        assert!(man_ext.write_candidate);

        let par_ext = resolved
            .effective_tags
            .iter()
            .find(|t| t.id == "plc1.D_Cyl1_ExtTime")
            .unwrap();
        assert_eq!(par_ext.role, Some(Role::Setpoint));
        assert!(par_ext.write_candidate);
        assert_eq!(par_ext.min, Some(0.0));
        assert_eq!(par_ext.max, Some(30000.0));
        assert_eq!(par_ext.unit.as_deref(), Some("ms"));

        let lim_ext = resolved
            .effective_tags
            .iter()
            .find(|t| t.id == "plc1.X_Cyl1_Ext_LS")
            .unwrap();
        assert_eq!(lim_ext.role, None);
        assert!(!lim_ext.write_candidate);
        assert!(lim_ext.address_read_only);

        // 檢查降階 (§7)
        let lowered = lower_to_iec_st(&resolved, &machine, &catalog);
        assert_eq!(lowered.ctrl_structs.len(), 3);
        assert!(lowered.st_code_preview.contains("TYPE cylinder_double_ctrl_t :"));
        assert!(lowered.st_code_preview.contains("cyl := st_cyl1_ctrl"));
    }

    #[test]
    fn test_u2_type_version_mismatch_and_u13_readonly_guardrail() {
        let mut machine = build_valid_demo_machine();
        // 故意將 cyl1 的 type_version 改為 99 (觸發 U2)，並把 man_ext 綁到唯讀輸入 X010 (觸發 U13 & U11)
        machine.units[0].type_version = 99;
        machine.units[0]
            .bind
            .insert("man_ext".to_string(), Some("plc1.X_Cyl1_Ext_LS".to_string()));

        let catalog = TypeCatalog::with_embedded_standards();
        let diags = validate_machine_with_catalog(&machine, &catalog);
        let rules: BTreeSet<&str> = diags.iter().map(|d| d.rule.as_str()).collect();
        assert!(rules.contains("U2"), "應觸發 U2 type_version 不符");
        assert!(rules.contains("U13"), "應觸發 U13 可寫候選綁定至唯讀位址");
        assert!(rules.contains("U11"), "應觸發 U11 field 與 ctrl 混綁同一 Tag");
    }

    #[test]
    fn test_u15_edit_distance_typo_hint() {
        let yaml = r#"
id: typo_unit
version: 1
title: 拼寫測試單元
ctrl:
  man_clamp:   { type: bool }
  at_clamp:    { type: bool }
  error_clamo: { message: 夾緊異常 }
"#;
        let ut = UnitTypeDef::from_yaml_str(yaml, "typo_unit.yaml").unwrap();
        let diags = validate_unit_type(&ut);
        assert!(
            diags.iter().any(|d| d.rule == "U15"),
            "clamo 與出現兩次的 clamp 編輯距離為 1，應產生 U15 提示"
        );
    }

    #[test]
    fn test_plc_data_service_overrides_and_guardrails() {
        let machine = build_valid_demo_machine();
        let catalog = TypeCatalog::with_embedded_standards();
        let resolved = resolve_machine_model(&machine, &catalog).unwrap();

        // 覆寫一個唯讀輸入 X_Cyl1_Ext_LS 為可寫 -> 應觸發 DS1 護欄錯誤
        let bad_overrides = vec![TagOverrideEntry {
            id: "plc1.X_Cyl1_Ext_LS".to_string(),
            writable: true,
        }];
        let report = evaluate_plc_data_service_writability(&resolved, &bad_overrides);
        assert!(report.guardrail_errors.iter().any(|d| d.rule == "DS1"));

        // 覆寫一個內部狀態點 M_Cyl1_ErrExt 為可寫 -> 通過護欄，但列入 review_required_tags
        let valid_overrides = vec![TagOverrideEntry {
            id: "plc1.M_Cyl1_ErrExt".to_string(),
            writable: true,
        }];
        let ok_report = evaluate_plc_data_service_writability(&resolved, &valid_overrides);
        assert!(ok_report.guardrail_errors.is_empty());
        assert!(
            ok_report
                .review_required_tags
                .contains(&"plc1.M_Cyl1_ErrExt".to_string())
        );
        let item = ok_report
            .items
            .iter()
            .find(|i| i.tag_id == "plc1.M_Cyl1_ErrExt")
            .unwrap();
        assert_eq!(item.source, WritableSource::Override);
        assert!(item.final_writable);
    }
}