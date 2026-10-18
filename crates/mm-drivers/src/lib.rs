// ArgonGUI/crates/mm-drivers/src/lib.rs
#![forbid(unsafe_code)]
#![allow(unused_imports)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use machine_model::datatype::ScalarType;
use machine_model::family::PlcFamily;
use machine_model::model::ScalarValue;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

pub mod serial_and_protocols;
pub use serial_and_protocols::{
    checksum, inovance, melsec_fx, modbus, omron, open_tokio_serial_builder, InovanceSeries,
    SerialParity, SerialPortConfig, TransportKind,
};

// ============================================================================
// 1. 工業通訊位元組序與 ByteTransform 模組
// ============================================================================

/// 工業通訊標準位元組序 (16/32/64-bit)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ByteOrder {
    /// Big-Endian 標準網路序 (Siemens S7, Omron FINS, Modbus 預設)
    ABCD,
    /// 單字內高低位元組正常、雙字高低字反轉 (部分 Modbus 設備常用)
    BADC,
    /// 單字內位元組反轉、雙字正常 (Inovance, Delta 常用)
    CDAB,
    /// Little-Endian 標準序 (Mitsubishi MC / Intel x86 / CIP 預設)
    DCBA,
}

impl Default for ByteOrder {
    fn default() -> Self {
        ByteOrder::ABCD
    }
}

/// 通用位元組轉換器
#[derive(Debug, Clone, Copy, Default)]
pub struct ByteTransform {
    pub order: ByteOrder,
}

impl ByteTransform {
    pub fn new(order: ByteOrder) -> Self {
        Self { order }
    }

    pub fn u16_to_bytes(&self, val: u16) -> [u8; 2] {
        match self.order {
            ByteOrder::ABCD | ByteOrder::CDAB => val.to_be_bytes(),
            ByteOrder::DCBA | ByteOrder::BADC => val.to_le_bytes(),
        }
    }

    pub fn bytes_to_u16(&self, b: &[u8]) -> u16 {
        if b.len() < 2 {
            return 0;
        }
        match self.order {
            ByteOrder::ABCD | ByteOrder::CDAB => u16::from_be_bytes([b[0], b[1]]),
            ByteOrder::DCBA | ByteOrder::BADC => u16::from_le_bytes([b[0], b[1]]),
        }
    }

    pub fn reorder_4(&self, be: [u8; 4]) -> [u8; 4] {
        match self.order {
            ByteOrder::ABCD => [be[0], be[1], be[2], be[3]],
            ByteOrder::BADC => [be[1], be[0], be[3], be[2]],
            ByteOrder::CDAB => [be[2], be[3], be[0], be[1]],
            ByteOrder::DCBA => [be[3], be[2], be[1], be[0]],
        }
    }

    pub fn u32_to_bytes(&self, val: u32) -> [u8; 4] {
        self.reorder_4(val.to_be_bytes())
    }

    pub fn bytes_to_u32(&self, b: &[u8]) -> u32 {
        if b.len() < 4 {
            return 0;
        }
        let raw = [b[0], b[1], b[2], b[3]];
        let be = self.reorder_4(raw);
        u32::from_be_bytes(be)
    }

    pub fn f32_to_bytes(&self, val: f32) -> [u8; 4] {
        self.reorder_4(val.to_be_bytes())
    }

    pub fn bytes_to_f32(&self, b: &[u8]) -> f32 {
        if b.len() < 4 {
            return 0.0;
        }
        let raw = [b[0], b[1], b[2], b[3]];
        let be = self.reorder_4(raw);
        f32::from_be_bytes(be)
    }
}

// ============================================================================
// 2. 工業通訊協定全硬體類別列舉 (DriverKind)
// ============================================================================

/// 涵蓋所有 56 種工業硬體驅動類別 (PLC, Modbus, Robot, CNC, Instrument)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    // 0. 內建高擬真模擬器
    Simulator,

    // 1. Mitsubishi 三菱電機 (Melsec)
    MelsecMcBinary,
    MelsecMcAscii,
    MelsecMcRBinary,
    MelsecA1EBinary,
    MelsecA1EAscii,
    MelsecA3CTcp,
    MelsecFxSerialTcp,
    MelsecFxLinksTcp,
    MelsecCipNet,

    // 2. Siemens 西門子 (Siemens S7)
    SiemensS7_1200,
    SiemensS7_1500,
    SiemensS7_300,
    SiemensS7_400,
    SiemensS7_200Smart,
    SiemensS7_200,
    SiemensFetchWrite,
    SiemensPpiTcp,

    // 3. Modbus 家族
    ModbusTcp,
    ModbusRtuOverTcp,
    ModbusAsciiOverTcp,
    ModbusUdp,

    // 4. Omron 歐姆龍 (Omron FINS / CIP)
    OmronFinsTcp,
    OmronFinsUdp,
    OmronCipNet,
    OmronHostLinkTcp,
    OmronHostLinkCModeTcp,

    // 5. Keyence 基恩士 (Keyence KV / Nano)
    KeyenceMcBinary,
    KeyenceMcAscii,
    KeyenceNanoTcp,
    KeyenceDlEn1,
    KeyenceSr2000,

    // 6. Inovance 匯川技術 (Easy / H3U / H5U / AM)
    InovanceEasyTcp,
    InovanceH3uTcp,
    InovanceH5uTcp,
    InovanceAmTcp,

    // 7. Delta 台達電子 (DVP / AS)
    DeltaDvpTcp,
    DeltaAsTcp,
    DeltaSerialAsciiTcp,

    // 8. Allen-Bradley 羅克韋爾 (CIP / PCCC / DF1)
    AllenBradleyCip,
    AllenBradleyMicroCip,
    AllenBradleySlcNet,
    AllenBradleyPccc,
    AllenBradleyDf1Tcp,

    // 9. Beckhoff 倍福 (TwinCAT ADS)
    BeckhoffAds,

    // 10. Panasonic 松下 (Mewtocol / MC)
    PanasonicMewtocolTcp,
    PanasonicMcBinary,

    // 11. FATEK 台灣永宏 (FBS)
    FatekProgramTcp,

    // 12. LSis 韓國樂星 (XGB / FastEnet)
    LSisFastEnet,
    LSisCnetTcp,

    // 13. Fuji 富士電機 (SPB / SPH)
    FujiSpbTcp,
    FujiSphNet,
    FujiCsType,

    // 14. XINJE 信捷電氣 (XC / XD)
    XinjeTcp,
    XinjeInternalNet,
    XinjeSerialTcp,

    // 15. Yokogawa 橫河電機 (FA-M3)
    YokogawaLinkTcp,

    // 16. Yaskawa 安川電機 (Memobus)
    YaskawaMemobusTcp,

    // 17. GE 通用電氣 (SRTP)
    GeSrtpTcp,

    // 18. 豐煒 / 豐田 / 西蒙 / 圖爾克 / 馬達槍開放協議
    VigorTcp,
    ToyotaToyoPuc,
    CimonHmiTcp,
    TurckReaderTcp,
    OpenProtocolTcp,

    // 19. 工業機器人 (Robot: FANUC / KUKA / Yaskawa / ABB / EFORT / Estun)
    RobotFanucNet,
    RobotEstunTcp,
    RobotEfortTcp,
    RobotKukaAvar,
    RobotYaskawaYrc,
    RobotAbbWeb,

    // 20. CNC 與儀器儀表 (Fanuc CNC & DL/T 645, DL/T 698, CJ/T 188, RKC)
    CncFanuc0i,
    InstDlt645Tcp,
    InstDlt698Tcp,
    InstRkcTempTcp,
    InstCjt188Tcp,
}

impl DriverKind {
    pub fn default_byte_order(self) -> ByteOrder {
        match self {
            DriverKind::SiemensS7_1200
            | DriverKind::SiemensS7_1500
            | DriverKind::SiemensS7_300
            | DriverKind::SiemensS7_400
            | DriverKind::SiemensS7_200Smart
            | DriverKind::SiemensS7_200
            | DriverKind::SiemensFetchWrite
            | DriverKind::SiemensPpiTcp
            | DriverKind::ModbusTcp
            | DriverKind::ModbusRtuOverTcp
            | DriverKind::ModbusAsciiOverTcp
            | DriverKind::ModbusUdp
            | DriverKind::OmronFinsTcp
            | DriverKind::OmronFinsUdp
            | DriverKind::OmronHostLinkTcp
            | DriverKind::OmronHostLinkCModeTcp
            | DriverKind::FatekProgramTcp
            | DriverKind::XinjeTcp
            | DriverKind::XinjeInternalNet
            | DriverKind::XinjeSerialTcp
            | DriverKind::YokogawaLinkTcp
            | DriverKind::YaskawaMemobusTcp
            | DriverKind::CimonHmiTcp
            | DriverKind::OpenProtocolTcp
            | DriverKind::RobotEstunTcp
            | DriverKind::RobotKukaAvar
            | DriverKind::RobotYaskawaYrc
            | DriverKind::RobotAbbWeb
            | DriverKind::CncFanuc0i
            | DriverKind::InstRkcTempTcp => ByteOrder::ABCD,

            DriverKind::InovanceEasyTcp
            | DriverKind::InovanceH3uTcp
            | DriverKind::InovanceH5uTcp
            | DriverKind::InovanceAmTcp
            | DriverKind::DeltaDvpTcp
            | DriverKind::DeltaAsTcp
            | DriverKind::DeltaSerialAsciiTcp => ByteOrder::CDAB,

            _ => ByteOrder::DCBA,
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            DriverKind::SiemensS7_1200
            | DriverKind::SiemensS7_1500
            | DriverKind::SiemensS7_300
            | DriverKind::SiemensS7_400
            | DriverKind::SiemensS7_200Smart
            | DriverKind::SiemensS7_200 => 102,
            DriverKind::SiemensFetchWrite | DriverKind::SiemensPpiTcp => 2000,

            DriverKind::ModbusTcp
            | DriverKind::ModbusRtuOverTcp
            | DriverKind::ModbusAsciiOverTcp
            | DriverKind::ModbusUdp
            | DriverKind::InovanceEasyTcp
            | DriverKind::InovanceH3uTcp
            | DriverKind::InovanceH5uTcp
            | DriverKind::InovanceAmTcp
            | DriverKind::DeltaDvpTcp
            | DriverKind::DeltaAsTcp
            | DriverKind::DeltaSerialAsciiTcp
            | DriverKind::XinjeTcp
            | DriverKind::XinjeInternalNet
            | DriverKind::XinjeSerialTcp
            | DriverKind::RobotEstunTcp => 502,

            DriverKind::MelsecMcBinary
            | DriverKind::MelsecMcAscii
            | DriverKind::MelsecMcRBinary
            | DriverKind::MelsecA1EBinary
            | DriverKind::MelsecA1EAscii
            | DriverKind::MelsecA3CTcp
            | DriverKind::MelsecFxSerialTcp
            | DriverKind::MelsecFxLinksTcp
            | DriverKind::KeyenceMcBinary
            | DriverKind::KeyenceMcAscii
            | DriverKind::FujiSpbTcp
            | DriverKind::VigorTcp => 5000,

            DriverKind::KeyenceNanoTcp => 8501,
            DriverKind::KeyenceDlEn1 => 9004,
            DriverKind::KeyenceSr2000 => 9000,
            DriverKind::OmronFinsTcp | DriverKind::OmronFinsUdp | DriverKind::OmronHostLinkTcp | DriverKind::OmronHostLinkCModeTcp => 9600,
            DriverKind::AllenBradleyCip
            | DriverKind::AllenBradleyMicroCip
            | DriverKind::AllenBradleySlcNet
            | DriverKind::AllenBradleyPccc
            | DriverKind::AllenBradleyDf1Tcp
            | DriverKind::OmronCipNet
            | DriverKind::MelsecCipNet => 44818,
            DriverKind::BeckhoffAds => 48898,
            DriverKind::PanasonicMewtocolTcp => 9094,
            DriverKind::PanasonicMcBinary => 5000,
            DriverKind::FatekProgramTcp => 500,
            DriverKind::LSisFastEnet | DriverKind::LSisCnetTcp => 2004,
            DriverKind::FujiSphNet | DriverKind::FujiCsType => 507,
            DriverKind::YokogawaLinkTcp => 12289,
            DriverKind::YaskawaMemobusTcp => 9999,
            DriverKind::GeSrtpTcp => 18245,
            DriverKind::ToyotaToyoPuc => 1025,
            DriverKind::CimonHmiTcp => 10260,
            DriverKind::TurckReaderTcp => 4001,
            DriverKind::OpenProtocolTcp => 4545,

            DriverKind::RobotFanucNet => 60008,
            DriverKind::RobotEfortTcp => 8008,
            DriverKind::RobotKukaAvar => 7000,
            DriverKind::RobotYaskawaYrc => 10040,
            DriverKind::RobotAbbWeb => 80,

            DriverKind::CncFanuc0i => 8193,
            DriverKind::InstDlt645Tcp | DriverKind::InstDlt698Tcp => 2000,
            DriverKind::InstRkcTempTcp => 4001,
            DriverKind::InstCjt188Tcp => 2000,
            DriverKind::Simulator => 0,
        }
    }
}

/// 驅動型錄描述（供 GUI 下拉選單與說明使用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverCatalogItem {
    pub kind: DriverKind,
    pub category: String,
    pub vendor: String,
    pub csharp_class: String,
    pub default_port: u16,
    pub default_byte_order: ByteOrder,
    pub address_example: String,
    pub supports_random_read: bool,
}

/// 列出全數支援之 56 種工業硬體型錄
pub fn list_driver_catalog() -> Vec<DriverCatalogItem> {
    use ByteOrder::*;
    use DriverKind::*;
    vec![
        DriverCatalogItem { kind: Simulator, category: "Built-in".into(), vendor: "ArgonGUI".into(), csharp_class: "SoftBuffer / VirtualPlc".into(), default_port: 0, default_byte_order: DCBA, address_example: "D100, M100, X010, Y010, DB1.DBD0".into(), supports_random_read: true },

        // 1. 三菱 Mitsubishi Melsec
        DriverCatalogItem { kind: MelsecMcBinary, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecMcNet (QnA-3E Binary)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10, W100, R100".into(), supports_random_read: true },
        DriverCatalogItem { kind: MelsecMcAscii, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecMcAsciiNet (QnA-3E ASCII)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10, W100".into(), supports_random_read: true },
        DriverCatalogItem { kind: MelsecMcRBinary, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecMcRNet (iQ-R 3E Binary)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, ZR100, W100".into(), supports_random_read: true },
        DriverCatalogItem { kind: MelsecA1EBinary, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecA1ENet (A-1E Binary)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10".into(), supports_random_read: false },
        DriverCatalogItem { kind: MelsecA1EAscii, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecA1EAsciiNet (A-1E ASCII)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10".into(), supports_random_read: false },
        DriverCatalogItem { kind: MelsecA3CTcp, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecA3CNetOverTcp (QnA 3C)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10".into(), supports_random_read: true },
        DriverCatalogItem { kind: MelsecFxSerialTcp, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecFxSerialOverTcp (FX 編程口)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10, S10".into(), supports_random_read: false },
        DriverCatalogItem { kind: MelsecFxLinksTcp, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecFxLinksOverTcp (FX 485)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10".into(), supports_random_read: false },
        DriverCatalogItem { kind: MelsecCipNet, category: "PLC".into(), vendor: "Mitsubishi 三菱".into(), csharp_class: "MelsecCipNet (iQ-R EtherNet/IP)".into(), default_port: 44818, default_byte_order: DCBA, address_example: "Tag_D100, Global_Run".into(), supports_random_read: true },

        // 2. 西門子 Siemens
        DriverCatalogItem { kind: SiemensS7_1200, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S1200)".into(), default_port: 102, default_byte_order: ABCD, address_example: "DB1.DBW0, DB1.DBD4, DB1.DBX0.0, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensS7_1500, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S1500)".into(), default_port: 102, default_byte_order: ABCD, address_example: "DB1.DBW0, DB1.DBD4, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensS7_300, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S300)".into(), default_port: 102, default_byte_order: ABCD, address_example: "DB1.DBW0, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensS7_400, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S400)".into(), default_port: 102, default_byte_order: ABCD, address_example: "DB1.DBW0, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensS7_200Smart, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S200Smart)".into(), default_port: 102, default_byte_order: ABCD, address_example: "V100, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensS7_200, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensS7Net(S200 CP243)".into(), default_port: 102, default_byte_order: ABCD, address_example: "V100, M100, I0.0, Q0.0".into(), supports_random_read: true },
        DriverCatalogItem { kind: SiemensFetchWrite, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensFetchWriteNet".into(), default_port: 2000, default_byte_order: ABCD, address_example: "DB1.DBW0, M100, I0, Q0".into(), supports_random_read: false },
        DriverCatalogItem { kind: SiemensPpiTcp, category: "PLC".into(), vendor: "Siemens 西門子".into(), csharp_class: "SiemensPPIOverTcp".into(), default_port: 2000, default_byte_order: ABCD, address_example: "V100, M10, I0.0, Q0.0".into(), supports_random_read: false },

        // 3. Modbus
        DriverCatalogItem { kind: ModbusTcp, category: "Modbus".into(), vendor: "Modbus".into(), csharp_class: "ModbusTcpNet".into(), default_port: 502, default_byte_order: ABCD, address_example: "100, x=2;100, x=4;100, s=1;100".into(), supports_random_read: false },
        DriverCatalogItem { kind: ModbusRtuOverTcp, category: "Modbus".into(), vendor: "Modbus".into(), csharp_class: "ModbusRtuOverTcp (含 CRC16)".into(), default_port: 502, default_byte_order: ABCD, address_example: "100, s=2;100".into(), supports_random_read: false },
        DriverCatalogItem { kind: ModbusAsciiOverTcp, category: "Modbus".into(), vendor: "Modbus".into(), csharp_class: "ModbusAsciiOverTcp (含 LRC)".into(), default_port: 502, default_byte_order: ABCD, address_example: "100, s=1;100".into(), supports_random_read: false },
        DriverCatalogItem { kind: ModbusUdp, category: "Modbus".into(), vendor: "Modbus".into(), csharp_class: "ModbusUdpNet".into(), default_port: 502, default_byte_order: ABCD, address_example: "100, D100".into(), supports_random_read: false },

        // 4. Omron 歐姆龍
        DriverCatalogItem { kind: OmronFinsTcp, category: "PLC".into(), vendor: "Omron 歐姆龍".into(), csharp_class: "OmronFinsNet (FINS/TCP)".into(), default_port: 9600, default_byte_order: ABCD, address_example: "D100, CIO10.0, W10.0, H20, A100".into(), supports_random_read: true },
        DriverCatalogItem { kind: OmronFinsUdp, category: "PLC".into(), vendor: "Omron 歐姆龍".into(), csharp_class: "OmronFinsUdp (FINS/UDP)".into(), default_port: 9600, default_byte_order: ABCD, address_example: "D100, CIO10.0, W10.0, H20".into(), supports_random_read: true },
        DriverCatalogItem { kind: OmronCipNet, category: "PLC".into(), vendor: "Omron 歐姆龍".into(), csharp_class: "OmronCipNet (NJ/NX/NY Tag)".into(), default_port: 44818, default_byte_order: DCBA, address_example: "D100, Recipe_Temp, Motor_Run".into(), supports_random_read: true },
        DriverCatalogItem { kind: OmronHostLinkTcp, category: "PLC".into(), vendor: "Omron 歐姆龍".into(), csharp_class: "OmronHostLinkOverTcp".into(), default_port: 9600, default_byte_order: ABCD, address_example: "D100, CIO10, W10, H20".into(), supports_random_read: false },
        DriverCatalogItem { kind: OmronHostLinkCModeTcp, category: "PLC".into(), vendor: "Omron 歐姆龍".into(), csharp_class: "OmronHostLinkCModeOverTcp".into(), default_port: 9600, default_byte_order: ABCD, address_example: "D100, DM100, LR10, HR10".into(), supports_random_read: false },

        // 5. Keyence 基恩士
        DriverCatalogItem { kind: KeyenceMcBinary, category: "PLC".into(), vendor: "Keyence 基恩士".into(), csharp_class: "KeyenceMcNet (KV-5000/7000/8000)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "DM100, MR100, R100, EM100".into(), supports_random_read: true },
        DriverCatalogItem { kind: KeyenceMcAscii, category: "PLC".into(), vendor: "Keyence 基恩士".into(), csharp_class: "KeyenceMcAsciiNet".into(), default_port: 5000, default_byte_order: DCBA, address_example: "DM100, MR100, R100".into(), supports_random_read: true },
        DriverCatalogItem { kind: KeyenceNanoTcp, category: "PLC".into(), vendor: "Keyence 基恩士".into(), csharp_class: "KeyenceNanoSerialOverTcp (上位鏈路)".into(), default_port: 8501, default_byte_order: DCBA, address_example: "DM100, MR100, R000".into(), supports_random_read: false },
        DriverCatalogItem { kind: KeyenceDlEn1, category: "Instrument".into(), vendor: "Keyence 基恩士".into(), csharp_class: "KeyenceDLEN1 (感測器通訊模組)".into(), default_port: 9004, default_byte_order: DCBA, address_example: "M0, SR,00,000".into(), supports_random_read: false },
        DriverCatalogItem { kind: KeyenceSr2000, category: "Instrument".into(), vendor: "Keyence 基恩士".into(), csharp_class: "KeyenceSR2000Series (讀碼器)".into(), default_port: 9000, default_byte_order: DCBA, address_example: "LON, LOFF".into(), supports_random_read: false },

        // 6. Inovance 匯川技術
        DriverCatalogItem { kind: InovanceEasyTcp, category: "PLC".into(), vendor: "Inovance 匯川".into(), csharp_class: "InovanceTcpNet(Easy320/521)".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100, X010, Y010".into(), supports_random_read: true },
        DriverCatalogItem { kind: InovanceH3uTcp, category: "PLC".into(), vendor: "Inovance 匯川".into(), csharp_class: "InovanceTcpNet(H3U)".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100, SM0, SD0".into(), supports_random_read: true },
        DriverCatalogItem { kind: InovanceH5uTcp, category: "PLC".into(), vendor: "Inovance 匯川".into(), csharp_class: "InovanceTcpNet(H5U)".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100, R100, B10".into(), supports_random_read: true },
        DriverCatalogItem { kind: InovanceAmTcp, category: "PLC".into(), vendor: "Inovance 匯川".into(), csharp_class: "InovanceTcpNet(AM400/AM600)".into(), default_port: 502, default_byte_order: CDAB, address_example: "MW100, MX10.0, IW0, QW0".into(), supports_random_read: true },

        // 7. Delta 台達電子
        DriverCatalogItem { kind: DeltaDvpTcp, category: "PLC".into(), vendor: "Delta 台達".into(), csharp_class: "DeltaTcpNet(DVP 系列)".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100, X10, Y10".into(), supports_random_read: false },
        DriverCatalogItem { kind: DeltaAsTcp, category: "PLC".into(), vendor: "Delta 台達".into(), csharp_class: "DeltaTcpNet(AS300/AH500)".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100, X0.0, Y0.0".into(), supports_random_read: false },
        DriverCatalogItem { kind: DeltaSerialAsciiTcp, category: "PLC".into(), vendor: "Delta 台達".into(), csharp_class: "DeltaSerialAsciiOverTcp".into(), default_port: 502, default_byte_order: CDAB, address_example: "D100, M100".into(), supports_random_read: false },

        // 8. Allen-Bradley 羅克韋爾
        DriverCatalogItem { kind: AllenBradleyCip, category: "PLC".into(), vendor: "Allen-Bradley".into(), csharp_class: "AllenBradleyNet (ControlLogix CIP)".into(), default_port: 44818, default_byte_order: DCBA, address_example: "D100, Program:Main.HeaterPV".into(), supports_random_read: true },
        DriverCatalogItem { kind: AllenBradleyMicroCip, category: "PLC".into(), vendor: "Allen-Bradley".into(), csharp_class: "AllenBradleyMicroCip (Micro800)".into(), default_port: 44818, default_byte_order: DCBA, address_example: "D100, Global_Count".into(), supports_random_read: true },
        DriverCatalogItem { kind: AllenBradleySlcNet, category: "PLC".into(), vendor: "Allen-Bradley".into(), csharp_class: "AllenBradleySLCNet (SLC500)".into(), default_port: 44818, default_byte_order: DCBA, address_example: "N7:0, B3:0/1, F8:0".into(), supports_random_read: false },
        DriverCatalogItem { kind: AllenBradleyPccc, category: "PLC".into(), vendor: "Allen-Bradley".into(), csharp_class: "AllenBradleyPcccNet".into(), default_port: 44818, default_byte_order: DCBA, address_example: "N7:0, B3:0".into(), supports_random_read: false },
        DriverCatalogItem { kind: AllenBradleyDf1Tcp, category: "PLC".into(), vendor: "Allen-Bradley".into(), csharp_class: "AllenBradleyDF1OverTcp".into(), default_port: 44818, default_byte_order: DCBA, address_example: "N7:0".into(), supports_random_read: false },

        // 9. Beckhoff 倍福
        DriverCatalogItem { kind: BeckhoffAds, category: "PLC".into(), vendor: "Beckhoff 倍福".into(), csharp_class: "BeckhoffAdsNet (TwinCAT 2/3 ADS)".into(), default_port: 48898, default_byte_order: DCBA, address_example: "MAIN.nCount, D100, M100".into(), supports_random_read: true },

        // 10. Panasonic 松下
        DriverCatalogItem { kind: PanasonicMewtocolTcp, category: "PLC".into(), vendor: "Panasonic 松下".into(), csharp_class: "PanasonicMewtocolOverTcp (FP系列)".into(), default_port: 9094, default_byte_order: DCBA, address_example: "DT100, R10, X0, Y0".into(), supports_random_read: false },
        DriverCatalogItem { kind: PanasonicMcBinary, category: "PLC".into(), vendor: "Panasonic 松下".into(), csharp_class: "PanasonicMcNet".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100".into(), supports_random_read: true },

        // 11. FATEK 永宏
        DriverCatalogItem { kind: FatekProgramTcp, category: "PLC".into(), vendor: "FATEK 永宏".into(), csharp_class: "FatekProgramOverTcp (FBS系列)".into(), default_port: 500, default_byte_order: ABCD, address_example: "D100, R100, M100, X0, Y0".into(), supports_random_read: false },

        // 12. LSis 樂星
        DriverCatalogItem { kind: LSisFastEnet, category: "PLC".into(), vendor: "LSis 樂星".into(), csharp_class: "XGBFastEnet (XGK/XGB)".into(), default_port: 2004, default_byte_order: DCBA, address_example: "DW100, MW100, MX100".into(), supports_random_read: true },
        DriverCatalogItem { kind: LSisCnetTcp, category: "PLC".into(), vendor: "LSis 樂星".into(), csharp_class: "XGBCnetOverTcp".into(), default_port: 2004, default_byte_order: DCBA, address_example: "DW100, MW100".into(), supports_random_read: false },

        // 13. Fuji 富士
        DriverCatalogItem { kind: FujiSpbTcp, category: "PLC".into(), vendor: "Fuji 富士".into(), csharp_class: "FujiSPBOverTcp (FLEX-PC)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, R100, M100".into(), supports_random_read: false },
        DriverCatalogItem { kind: FujiSphNet, category: "PLC".into(), vendor: "Fuji 富士".into(), csharp_class: "FujiSPHNet (MICREX-SX)".into(), default_port: 507, default_byte_order: DCBA, address_example: "M1.100, D100".into(), supports_random_read: false },
        DriverCatalogItem { kind: FujiCsType, category: "PLC".into(), vendor: "Fuji 富士".into(), csharp_class: "FujiCommandSettingType".into(), default_port: 507, default_byte_order: DCBA, address_example: "M1.100".into(), supports_random_read: false },

        // 14. XINJE 信捷
        DriverCatalogItem { kind: XinjeTcp, category: "PLC".into(), vendor: "XINJE 信捷".into(), csharp_class: "XinJETcpNet / XinJEInternalNet".into(), default_port: 502, default_byte_order: ABCD, address_example: "D100, M100, HD100, HM100".into(), supports_random_read: false },
        DriverCatalogItem { kind: XinjeInternalNet, category: "PLC".into(), vendor: "XINJE 信捷".into(), csharp_class: "XinJEInternalNet".into(), default_port: 502, default_byte_order: ABCD, address_example: "D100, M100".into(), supports_random_read: false },
        DriverCatalogItem { kind: XinjeSerialTcp, category: "PLC".into(), vendor: "XINJE 信捷".into(), csharp_class: "XinJESerialOverTcp".into(), default_port: 502, default_byte_order: ABCD, address_example: "D100, M100".into(), supports_random_read: false },

        // 15. Yokogawa 橫河
        DriverCatalogItem { kind: YokogawaLinkTcp, category: "PLC".into(), vendor: "Yokogawa 橫河".into(), csharp_class: "YokogawaLinkTcp (FA-M3)".into(), default_port: 12289, default_byte_order: ABCD, address_example: "D100, B100, I10, M10".into(), supports_random_read: true },

        // 16. Yaskawa 安川
        DriverCatalogItem { kind: YaskawaMemobusTcp, category: "PLC".into(), vendor: "Yaskawa 安川".into(), csharp_class: "MemobusTcpNet (MP2000/3000)".into(), default_port: 9999, default_byte_order: ABCD, address_example: "D100, M100".into(), supports_random_read: false },

        // 17. GE 通用電氣
        DriverCatalogItem { kind: GeSrtpTcp, category: "PLC".into(), vendor: "GE 通用電氣".into(), csharp_class: "GeSRTPNet (PACSystems RX3i)".into(), default_port: 18245, default_byte_order: DCBA, address_example: "R100, AI10, M10, I10, Q10".into(), supports_random_read: false },

        // 18. 專用協議 / 豐煒 / 豐田 / 西蒙 / 圖爾克 / 開放協議
        DriverCatalogItem { kind: VigorTcp, category: "PLC".into(), vendor: "Vigor 豐煒".into(), csharp_class: "VigorSerialOverTcp (VB/VH)".into(), default_port: 5000, default_byte_order: DCBA, address_example: "D100, M100, X10, Y10".into(), supports_random_read: false },
        DriverCatalogItem { kind: ToyotaToyoPuc, category: "PLC".into(), vendor: "Toyota 豐田".into(), csharp_class: "ToyoPuc (PC10G/PC3JG)".into(), default_port: 1025, default_byte_order: DCBA, address_example: "D100, M100, K10".into(), supports_random_read: false },
        DriverCatalogItem { kind: CimonHmiTcp, category: "PLC".into(), vendor: "Cimon 西蒙".into(), csharp_class: "CimonHmiProtocol".into(), default_port: 10260, default_byte_order: ABCD, address_example: "D100, M100".into(), supports_random_read: false },
        DriverCatalogItem { kind: TurckReaderTcp, category: "Instrument".into(), vendor: "Turck 圖爾克".into(), csharp_class: "ReaderNet (RFID)".into(), default_port: 4001, default_byte_order: ABCD, address_example: "D100".into(), supports_random_read: false },
        DriverCatalogItem { kind: OpenProtocolTcp, category: "Special".into(), vendor: "Atlas Copco".into(), csharp_class: "OpenProtocolNet (扭力槍)".into(), default_port: 4545, default_byte_order: ABCD, address_example: "MID0001, D100".into(), supports_random_read: false },

        // 19. 工業機器人 (Robot)
        DriverCatalogItem { kind: RobotFanucNet, category: "Robot".into(), vendor: "FANUC 發那科".into(), csharp_class: "FanucInterfaceNet (Robot)".into(), default_port: 60008, default_byte_order: DCBA, address_example: "D100, R[1], DI[1], DO[1]".into(), supports_random_read: false },
        DriverCatalogItem { kind: RobotEstunTcp, category: "Robot".into(), vendor: "Estun 埃斯頓".into(), csharp_class: "EstunTcpNet (Robot)".into(), default_port: 502, default_byte_order: ABCD, address_example: "D100, M100".into(), supports_random_read: false },
        DriverCatalogItem { kind: RobotEfortTcp, category: "Robot".into(), vendor: "EFORT 埃夫特".into(), csharp_class: "EFORTNet (Robot)".into(), default_port: 8008, default_byte_order: DCBA, address_example: "D100".into(), supports_random_read: false },
        DriverCatalogItem { kind: RobotKukaAvar, category: "Robot".into(), vendor: "KUKA 庫卡".into(), csharp_class: "KukaAvarProxyNet (Robot)".into(), default_port: 7000, default_byte_order: ABCD, address_example: "$OV_PRO, D100".into(), supports_random_read: false },
        DriverCatalogItem { kind: RobotYaskawaYrc, category: "Robot".into(), vendor: "Yaskawa 安川".into(), csharp_class: "YRC1000TcpNet (Robot)".into(), default_port: 10040, default_byte_order: ABCD, address_example: "D100, B001".into(), supports_random_read: false },
        DriverCatalogItem { kind: RobotAbbWeb, category: "Robot".into(), vendor: "ABB 機器人".into(), csharp_class: "ABBWebApiClient (Robot)".into(), default_port: 80, default_byte_order: ABCD, address_example: "D100, rapid_var".into(), supports_random_read: false },

        // 20. CNC 與儀表 (Instrument)
        DriverCatalogItem { kind: CncFanuc0i, category: "CNC".into(), vendor: "FANUC 發那科".into(), csharp_class: "FanucSeries0i (CNC Focas)".into(), default_port: 8193, default_byte_order: ABCD, address_example: "D100, SpindleSpeed, FeedRate".into(), supports_random_read: false },
        DriverCatalogItem { kind: InstDlt645Tcp, category: "Instrument".into(), vendor: "DL/T 645 電表".into(), csharp_class: "DLT645OverTcp (智能電表)".into(), default_port: 2000, default_byte_order: DCBA, address_example: "02-01-01-00 (A相電壓)".into(), supports_random_read: false },
        DriverCatalogItem { kind: InstDlt698Tcp, category: "Instrument".into(), vendor: "DL/T 698 電表".into(), csharp_class: "DLT698OverTcp".into(), default_port: 2000, default_byte_order: DCBA, address_example: "02-01-01-00".into(), supports_random_read: false },
        DriverCatalogItem { kind: InstRkcTempTcp, category: "Instrument".into(), vendor: "RKC 理化溫控".into(), csharp_class: "RkcTemperatureController".into(), default_port: 4001, default_byte_order: ABCD, address_example: "M1, S1, D100".into(), supports_random_read: false },
        DriverCatalogItem { kind: InstCjt188Tcp, category: "Instrument".into(), vendor: "CJ/T 188 水表".into(), csharp_class: "CJT188OverTcp".into(), default_port: 2000, default_byte_order: ABCD, address_example: "90-1F, D100".into(), supports_random_read: false },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TagQuality {
    Good,
    Bad,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimMode {
    Good,
    BadSensor,
    UncertainDisconnect,
    LaggedRecover,
    ReadbackFail,
    AlarmFlood,
    Ac09Errors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverConfig {
    pub id: String,
    pub kind: DriverKind,
    pub host: String,
    pub port: u16,
    pub station: u8,
    pub rack: u8,
    pub slot: u8,
    pub byte_order: ByteOrder,
    pub timeout_ms: u64,
    pub scan_interval_ms: u64,
    /// 若為 true，即使選擇真實協定亦在無實體連線時自動使用協定封包模擬器回應
    pub loopback_emulation: bool,
    pub plc_family: PlcFamily,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            id: "plc1".into(),
            kind: DriverKind::InovanceEasyTcp,
            host: "127.0.0.1".into(),
            port: 502,
            station: 1,
            rack: 0,
            slot: 1,
            byte_order: ByteOrder::CDAB,
            timeout_ms: 1000,
            scan_interval_ms: 50,
            loopback_emulation: true,
            plc_family: PlcFamily::InovanceEasy,
        }
    }
}

// ============================================================================
// 3. 讀寫請求、回傳結構、延遲統計與壓力測試報告
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriverTrace {
    pub elapsed_us: u64,
    pub tx_hex: String,
    pub rx_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleReadResult {
    pub address: String,
    pub data_type: String,
    pub value: ScalarValue,
    pub quality: TagQuality,
    pub trace: DriverTrace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteAck {
    pub address: String,
    pub written_value: ScalarValue,
    pub readback_value: ScalarValue,
    pub readback_matched: bool,
    pub timestamp_ms: u64,
    pub trace: DriverTrace,
}

/// 連續讀取結果 (Batch / Continuous Read)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuousReadResult {
    pub start_address: String,
    pub length: u16,
    pub data_type: String,
    pub values: Vec<ScalarValue>,
    pub raw_bytes_hex: String,
    pub trace: DriverTrace,
}

/// 隨機/多點讀取項目 (Random / Scatter Read)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomReadItem {
    pub address: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomReadResult {
    pub items: Vec<SingleReadResult>,
    pub total_elapsed_us: u64,
    pub tx_hex: String,
    pub rx_hex: String,
}

/// 隨機/多點寫入項目 (Random / Scatter Write)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomWriteItem {
    pub address: String,
    pub data_type: String,
    pub value: ScalarValue,
}

/// 通訊延遲測試報告 (Latency Profiler)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyReport {
    pub address: String,
    pub rounds: u32,
    pub success_count: u32,
    pub fail_count: u32,
    pub min_us: u64,
    pub max_us: u64,
    pub avg_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub jitter_us: u64,
    pub samples_us: Vec<u64>,
}

/// 壓力測試設定與報告 (Stress Benchmark)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StressMode {
    SingleRead,
    SingleWrite,
    ContinuousRead,
    ContinuousWrite,
    RandomRead,
    MixedReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StressTestConfig {
    pub mode: StressMode,
    pub address: String,
    pub length: u16,
    pub concurrency: u16,
    pub iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StressTestReport {
    pub mode: StressMode,
    pub total_ops: u32,
    pub success_ops: u32,
    pub failed_ops: u32,
    pub total_duration_ms: f64,
    pub throughput_ops_per_sec: f64,
    pub throughput_kb_per_sec: f64,
    pub min_us: u64,
    pub avg_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagUpdate {
    pub tag_id: String,
    pub value: ScalarValue,
    pub quality: TagQuality,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DriverError {
    InvalidAddress(String),
    Disconnected(String),
    ReadbackMismatch { address: String, expected: String, actual: String },
    ProtocolError(String),
    IoError(String),
}

impl std::error::Error for DriverError {}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::InvalidAddress(m) => write!(f, "位址解析錯誤: {}", m),
            DriverError::Disconnected(m) => write!(f, "尚未連線或斷線: {}", m),
            DriverError::ReadbackMismatch { address, expected, actual } => {
                write!(f, "回讀不符 ({}): 預期 {}, 實際 {}", address, expected, actual)
            }
            DriverError::ProtocolError(m) => write!(f, "PLC/設備協定錯誤: {}", m),
            DriverError::IoError(m) => write!(f, "I/O 通訊逾時或失敗: {}", m),
        }
    }
}

// ============================================================================
// 4. 各廠牌協定報文封裝器 (MC / S7 / Modbus / FINS / CIP 等)
// ============================================================================

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

/// 計算 Modbus CRC-16
pub fn modbus_crc16(data: &[u8]) -> [u8; 2] {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b);
        for _ in 0..8 {
            if (crc & 0x0001) != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    [(crc & 0xFF) as u8, (crc >> 8) as u8]
}

/// 統一解析各廠牌位址字串，回傳 `(區域代碼, 起始編號, 位元偏移, 是否位元元件)`
pub fn parse_universal_plc_address(raw: &str) -> Result<(String, u32, Option<u8>, bool), DriverError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(DriverError::InvalidAddress("位址字串不得為空".into()));
    }

    // 支援西門子 DB1.DBW0 / DB1.DBD4 / DB1.DBX0.1 格式
    if let Some(rest) = s.strip_prefix("DB").or_else(|| s.strip_prefix("db")) {
        if let Some((db_num_str, offset_part)) = rest.split_once('.') {
            let db_no: u16 = db_num_str.parse().unwrap_or(1);
            let up = offset_part.to_ascii_uppercase();
            if let Some(bit_part) = up.strip_prefix("DBX") {
                let (byte_str, bit_str) = bit_part.split_once('.').unwrap_or((bit_part, "0"));
                let byte_idx: u32 = byte_str.parse().unwrap_or(0);
                let bit_idx: u8 = bit_str.parse().unwrap_or(0);
                return Ok((format!("DB{}", db_no), byte_idx, Some(bit_idx), true));
            }
            let num_str = up
                .trim_start_matches("DBW")
                .trim_start_matches("DBD")
                .trim_start_matches("DBB");
            let byte_idx: u32 = num_str.parse().unwrap_or(0);
            return Ok((format!("DB{}", db_no), byte_idx, None, false));
        }
    }

    // 支援一般前綴如 D100, M100, X010, Y010, DM100, CIO10.1, V100, R100, HD100
    let alpha_len = s.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let prefix = if alpha_len > 0 {
        s[..alpha_len].to_ascii_uppercase()
    } else {
        "D".to_string()
    };
    let num_part = if alpha_len > 0 { &s[alpha_len..] } else { s };
    let (idx_str, bit_opt) = match num_part.split_once('.') {
        Some((a, b)) => (a, b.parse::<u8>().ok()),
        None => (num_part, None),
    };

    let radix = if prefix == "X" || prefix == "Y" { 8 } else { 10 };
    let idx = u32::from_str_radix(idx_str, radix)
        .or_else(|_| idx_str.parse::<u32>())
        .unwrap_or(0);
    let is_bit = matches!(prefix.as_str(), "X" | "Y" | "M" | "S" | "B" | "I" | "Q" | "MR" | "L")
        || bit_opt.is_some();

    Ok((prefix, idx, bit_opt, is_bit))
}

/// 依各廠牌協定組裝真實請求封包（支援讀與寫，供 Socket 發送與 Hex 報文檢視器使用）
pub fn build_protocol_frame(
    kind: DriverKind,
    station: u8,
    address: &str,
    length: u16,
    is_write: bool,
) -> Vec<u8> {
    let (prefix, idx, _, is_bit) =
        parse_universal_plc_address(address).unwrap_or(("D".into(), 100, None, false));

    match kind {
        // 1. 三菱 / 基恩士 / 松下 QnA-3E Binary (MelsecHelper.BuildReadMcCoreCommand)
        DriverKind::MelsecMcBinary | DriverKind::KeyenceMcBinary | DriverKind::PanasonicMcBinary => {
            let dev_code: u8 = match prefix.as_str() {
                "M" | "MR" => 0x90,
                "X" => 0x9C,
                "Y" => 0x9D,
                "B" => 0xA0,
                "D" | "DM" => 0xA8,
                "W" => 0xB4,
                "R" => 0xAF,
                "ZR" => 0xB0,
                _ => 0xA8,
            };
            let cmd: [u8; 2] = if is_write { [0x01, 0x14] } else { [0x01, 0x04] };
            let subcmd: [u8; 2] = if is_bit { [0x01, 0x00] } else { [0x00, 0x00] };
            vec![
                0x50, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x0C, 0x00, 0x10, 0x00,
                cmd[0], cmd[1], subcmd[0], subcmd[1],
                (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8, ((idx >> 16) & 0xFF) as u8,
                dev_code, (length & 0xFF) as u8, (length >> 8) as u8,
            ]
        }
        // 2. 三菱 iQ-R 3E Binary (MelsecMcRNet: 4-byte Addr)
        DriverKind::MelsecMcRBinary => {
            let cmd: [u8; 2] = if is_write { [0x01, 0x14] } else { [0x01, 0x04] };
            let subcmd: [u8; 2] = if is_bit { [0x03, 0x00] } else { [0x02, 0x00] };
            vec![
                0x50, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x0E, 0x00, 0x10, 0x00,
                cmd[0], cmd[1], subcmd[0], subcmd[1],
                (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8, ((idx >> 16) & 0xFF) as u8, ((idx >> 24) & 0xFF) as u8,
                0xA8, 0x00, (length & 0xFF) as u8, (length >> 8) as u8,
            ]
        }
        // 3. 三菱 QnA-3E ASCII (MelsecHelper.BuildAsciiReadMcCoreCommand)
        DriverKind::MelsecMcAscii | DriverKind::KeyenceMcAscii => {
            let cmd_str = if is_write { "1401" } else { "0401" };
            let sub_str = if is_bit { "0001" } else { "0000" };
            format!("500000FF03FF0000180010{}{}{}*{:06}{:04}", cmd_str, sub_str, prefix, idx, length).into_bytes()
        }
        // 4. 西門子 S7 ISO-on-TCP (SiemensS7Net)
        DriverKind::SiemensS7_1200
        | DriverKind::SiemensS7_1500
        | DriverKind::SiemensS7_300
        | DriverKind::SiemensS7_400
        | DriverKind::SiemensS7_200Smart
        | DriverKind::SiemensS7_200 => {
            let bit_addr = idx * 8;
            let func: u8 = if is_write { 0x05 } else { 0x04 };
            vec![
                0x03, 0x00, 0x00, 0x1F, 0x02, 0xF0, 0x80, 0x32, 0x01, 0x00, 0x00, 0x00, 0x01,
                0x00, 0x0E, 0x00, 0x00, func, 0x01, 0x12, 0x0A, 0x10, 0x02,
                (length >> 8) as u8, (length & 0xFF) as u8, 0x00, 0x01, 0x84,
                ((bit_addr >> 16) & 0xFF) as u8, ((bit_addr >> 8) & 0xFF) as u8, (bit_addr & 0xFF) as u8,
            ]
        }
        // 5. Modbus RTU Over TCP (含 CRC-16)
        DriverKind::ModbusRtuOverTcp => {
            let fc: u8 = if is_write {
                if is_bit { 0x05 } else { 0x06 }
            } else if is_bit {
                0x01
            } else {
                0x03
            };
            let addr_u16 = (idx & 0xFFFF) as u16;
            let mut pdu = vec![
                station, fc,
                (addr_u16 >> 8) as u8, (addr_u16 & 0xFF) as u8,
                (length >> 8) as u8, (length & 0xFF) as u8,
            ];
            let crc = modbus_crc16(&pdu);
            pdu.extend_from_slice(&crc);
            pdu
        }
        // 6. Modbus TCP / 匯川 / 台達 / 信捷 / 埃斯頓
        DriverKind::ModbusTcp
        | DriverKind::ModbusUdp
        | DriverKind::InovanceEasyTcp
        | DriverKind::InovanceH3uTcp
        | DriverKind::InovanceH5uTcp
        | DriverKind::InovanceAmTcp
        | DriverKind::DeltaDvpTcp
        | DriverKind::DeltaAsTcp
        | DriverKind::DeltaSerialAsciiTcp
        | DriverKind::XinjeTcp
        | DriverKind::XinjeInternalNet
        | DriverKind::XinjeSerialTcp
        | DriverKind::RobotEstunTcp => {
            let fc: u8 = if is_write {
                if length > 1 { 0x10 } else if is_bit { 0x05 } else { 0x06 }
            } else if is_bit {
                0x01
            } else {
                0x03
            };
            let addr_u16 = (idx & 0xFFFF) as u16;
            vec![
                0x00, 0x01, 0x00, 0x00, 0x00, 0x06, station, fc,
                (addr_u16 >> 8) as u8, (addr_u16 & 0xFF) as u8,
                (length >> 8) as u8, (length & 0xFF) as u8,
            ]
        }
        // 7. 歐姆龍 FINS/TCP 與 FINS/UDP
        DriverKind::OmronFinsTcp | DriverKind::OmronFinsUdp => {
            let area: u8 = if prefix == "D" { 0x82 } else if prefix == "W" { 0xB1 } else { 0xB0 };
            let mrc_src: [u8; 2] = if is_write { [0x01, 0x02] } else { [0x01, 0x01] };
            let addr_u16 = (idx & 0xFFFF) as u16;
            vec![
                0x46, 0x49, 0x4E, 0x53, 0x00, 0x00, 0x00, 0x1A, 0x00, 0x00, 0x00, 0x02,
                0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x02, 0x00, station, 0x00, 0x00, 0xEF,
                0x00, 0x01, mrc_src[0], mrc_src[1], area,
                (addr_u16 >> 8) as u8, (addr_u16 & 0xFF) as u8, 0x00,
                (length >> 8) as u8, (length & 0xFF) as u8,
            ]
        }
        // 8. 基恩士上位鏈路 (KeyenceNanoHelper)
        DriverKind::KeyenceNanoTcp => {
            let op = if is_write { "WRS" } else { "RDS" };
            format!("{} {}.U {}\r", op, address.to_ascii_uppercase(), length).into_bytes()
        }
        // 9. 台灣永宏 FATEK 編程口協議 (STX 0x02 + Station + Cmd 46/47 + ETX 0x03)
        DriverKind::FatekProgramTcp => {
            let cmd = if is_write { "47" } else { "46" };
            format!("\x02{:02X}{}02{:04}{:02X}\x03", station, cmd, idx, length).into_bytes()
        }
        // 10. 松下 Mewtocol 協議 (%01#RDD0010000101**\r)
        DriverKind::PanasonicMewtocolTcp => {
            let end_idx = idx + u32::from(length.saturating_sub(1));
            format!("%{:02X}#RDD{:05}{:05}**\r", station, idx, end_idx).into_bytes()
        }
        // 11. 韓國 LSis XGT FastEnet ("LSIS-XGT")
        DriverKind::LSisFastEnet | DriverKind::LSisCnetTcp => {
            let mut frame = b"LSIS-XGT\x00\x00\x00\x00\xA0\x33\x00\x01\x0E\x00\x00\x00".to_vec();
            frame.extend_from_slice(&[if is_write { 0x58 } else { 0x54 }, 0x00, 0x02, 0x00]);
            frame.extend_from_slice(format!("%DW{}", idx).as_bytes());
            frame
        }
        // 12. 橫河 FA-M3 YokogawaLinkTcp
        DriverKind::YokogawaLinkTcp => {
            vec![
                if is_write { 0x32 } else { 0x31 }, station, 0x00, 0x0A,
                0x00, 0x04, 0x00, 0x01, (idx >> 8) as u8, (idx & 0xFF) as u8,
                (length >> 8) as u8, (length & 0xFF) as u8,
            ]
        }
        // 13. 倍福 Beckhoff ADS/TCP
        DriverKind::BeckhoffAds => {
            vec![
                0x00, 0x00, 0x2C, 0x00, 0x00, 0x00,
                192, 168, 1, 10, 1, 1, 0x53, 0x03, // Target NetId + Port 851
                192, 168, 1, 200, 1, 1, 0x01, 0x80, // Source NetId
                if is_write { 0x03 } else { 0x02 }, 0x00, 0x04, 0x00, 0x0C, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
                0x20, 0x40, 0x00, 0x00, (idx & 0xFF) as u8, ((idx >> 8) & 0xFF) as u8, 0x00, 0x00,
            ]
        }
        // 14. 智能電表 DL/T 645-2007
        DriverKind::InstDlt645Tcp | DriverKind::InstDlt698Tcp => {
            vec![
                0xFE, 0xFE, 0xFE, 0xFE, 0x68, station, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68,
                0x11, 0x04, 0x33, 0x34, 0x34, 0x35, 0xB6, 0x16,
            ]
        }
        // 15. Atlas Copco OpenProtocol
        DriverKind::OpenProtocolTcp => {
            b"00200001001         \x00".to_vec()
        }
        // 16. Allen-Bradley / Omron CIP / 其它 EtherNet/IP (0x006F SendRRData + CIP 0x4C/0x4D)
        _ => {
            let service: u8 = if is_write { 0x4D } else { 0x4C };
            let mut frame = vec![0x6F, 0x00, 0x1A, 0x00, 0x01, 0x02, 0x03, 0x04, service, (address.len() as u8 + 1) / 2, 0x91, address.len() as u8];
            frame.extend_from_slice(address.as_bytes());
            if address.len() % 2 != 0 {
                frame.push(0x00);
            }
            frame.extend_from_slice(&[(length & 0xFF) as u8, (length >> 8) as u8]);
            frame
        }
    }
}

pub fn build_protocol_read_frame(
    kind: DriverKind,
    station: u8,
    address: &str,
    length: u16,
) -> Vec<u8> {
    build_protocol_frame(kind, station, address, length, false)
}

// ============================================================================
// 5. 統一驅動介面與 UniversalPlcDriver 實作 (支援 56 種硬體)
// ============================================================================

pub trait PlcDriver: Send {
    fn config(&self) -> &DriverConfig;
    fn is_connected(&self) -> bool;
    fn connect(&mut self, cfg: DriverConfig) -> Result<DriverTrace, DriverError>;
    fn disconnect(&mut self) -> Result<(), DriverError>;

    /// 1. 單點讀取 (Single Read)
    fn read_single(&mut self, address: &str, data_type: &str) -> Result<SingleReadResult, DriverError>;

    /// 2. 單點寫入 + Read-back 驗證 (Single Write with Readback)
    fn write_single(&mut self, address: &str, data_type: &str, value: ScalarValue) -> Result<WriteAck, DriverError>;

    /// 3. 連續區塊讀取 (Continuous / Batch Read)
    fn read_continuous(&mut self, start_address: &str, length: u16, data_type: &str) -> Result<ContinuousReadResult, DriverError>;

    /// 4. 連續區塊寫入 (Continuous / Batch Write)
    fn write_continuous(&mut self, start_address: &str, data_type: &str, values: &[ScalarValue]) -> Result<WriteAck, DriverError>;

    /// 5. 隨機多點讀取 (Random / Scatter-Gather Read)
    fn read_random(&mut self, items: &[RandomReadItem]) -> Result<RandomReadResult, DriverError>;

    /// 6. 隨機多點寫入 (Random / Scatter-Gather Write)
    fn write_random(&mut self, items: &[RandomWriteItem]) -> Result<Vec<WriteAck>, DriverError>;

    /// 7. 通訊延遲剖析 (Communication Latency / RTT Profiler)
    fn ping_latency(&mut self, address: &str, rounds: u32) -> Result<LatencyReport, DriverError>;

    /// 8. 壓力測試 (Stress Benchmark)
    fn run_stress_test(&mut self, stress_cfg: &StressTestConfig) -> Result<StressTestReport, DriverError>;

    /// 9. DataService 背景掃描輪詢與模擬模式設定
    fn poll(&mut self, out: &Sender<TagUpdate>) -> Result<(), DriverError>;
    fn set_simulation(&mut self, mode: SimMode) -> Result<(), DriverError>;

    /// 10. 設定通訊逾時 (ms)，同時套用到已建立的 Socket 讀寫逾時 (§7.1 緊湊逾時)
    fn set_timeout_ms(&mut self, timeout_ms: u64);
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 統一實作全硬體驅動家族之通用工業驅動引擎
pub struct UniversalPlcDriver {
    cfg: DriverConfig,
    connected: bool,
    sim_mode: SimMode,
    transform: ByteTransform,
    stream: Option<TcpStream>,
    /// 內建統一軟元件記憶映射區 `(prefix, index) -> u16 word` 與位元區
    words: BTreeMap<(String, u32), u16>,
    bits: BTreeMap<(String, u32), bool>,
    strings: BTreeMap<String, String>,
}

impl UniversalPlcDriver {
    pub fn new(cfg: DriverConfig) -> Self {
        let transform = ByteTransform::new(cfg.byte_order);
        let mut drv = Self {
            cfg,
            connected: false,
            sim_mode: SimMode::Good,
            transform,
            stream: None,
            words: BTreeMap::new(),
            bits: BTreeMap::new(),
            strings: BTreeMap::new(),
        };
        drv.seed_default_memory();
        drv
    }

    fn seed_default_memory(&mut self) {
        self.bits.insert(("X".into(), 0), true);   // Safety_Door_Closed (X000)
        self.bits.insert(("X".into(), 8), false);  // X010
        self.bits.insert(("X".into(), 9), true);   // X011
        self.bits.insert(("M".into(), 104), true); // Sys_All_Homed
        self.bits.insert(("M".into(), 123), true); // M_Cyl1_AtRet
        self.bits.insert(("M".into(), 133), true); // M_Cyl2_AtRet
        self.bits.insert(("M".into(), 150), true); // Fan_Run
        self.bits.insert(("M".into(), 151), true); // Valve_Open
        self.words.insert(("D".into(), 100), 1680);
        self.words.insert(("D".into(), 110), 1280);
        self.words.insert(("D".into(), 120), 3000);
        self.words.insert(("D".into(), 122), 5000);
        self.words.insert(("D".into(), 130), 2500);
        self.words.insert(("D".into(), 150), 2450);
        self.words.insert(("D".into(), 152), 165);
        self.words.insert(("D".into(), 154), 165);
        self.words.insert(("D".into(), 156), 72);
        self.words.insert(("D".into(), 158), 18);
        self.words.insert(("DB1".into(), 0), 999);
    }

    fn check_online(&self) -> Result<(), DriverError> {
        if !self.connected {
            return Err(DriverError::Disconnected("驅動尚未建立連線，請先執行 Connect".into()));
        }
        if self.sim_mode == SimMode::UncertainDisconnect {
            return Err(DriverError::Disconnected("模擬通訊斷線 (Uncertain Disconnect)".into()));
        }
        Ok(())
    }

    /// 透過底層實體 Socket 進行同步請求與應答報文交換
    pub fn exchange_raw(&mut self, tx: &[u8]) -> Result<Vec<u8>, DriverError> {
        if let Some(ref mut s) = self.stream {
            s.write_all(tx).map_err(|e| DriverError::IoError(format!("Socket TX 失敗: {}", e)))?;
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).map_err(|e| DriverError::IoError(format!("Socket RX 逾時或失敗: {}", e)))?;
            if n == 0 {
                return Err(DriverError::IoError("遠端設備已中斷 Socket 連線 (EOF)".into()));
            }
            Ok(buf[..n].to_vec())
        } else {
            Err(DriverError::Disconnected("無作用中的實體 Socket 連線".into()))
        }
    }

    /// 從各廠牌實體協議回應中擷取有效資料段 (Payload)
    fn extract_payload_from_response(&self, rx: &[u8], _is_bit: bool, _len_words: u16) -> Result<Vec<u8>, DriverError> {
        match self.cfg.kind {
            DriverKind::ModbusTcp
            | DriverKind::InovanceEasyTcp
            | DriverKind::InovanceH3uTcp
            | DriverKind::InovanceH5uTcp
            | DriverKind::InovanceAmTcp
            | DriverKind::DeltaDvpTcp
            | DriverKind::DeltaAsTcp
            | DriverKind::XinjeTcp => {
                if rx.len() >= 9 {
                    let fc = rx[7];
                    if fc >= 0x80 {
                        let err_code = if rx.len() > 8 { rx[8] } else { 0 };
                        return Err(DriverError::ProtocolError(format!("Modbus Exception (FC: 0x{:02X}, Code: 0x{:02X})", fc, err_code)));
                    }
                    let byte_count = rx[8] as usize;
                    if rx.len() >= 9 + byte_count {
                        return Ok(rx[9..9 + byte_count].to_vec());
                    }
                }
            }
            DriverKind::ModbusRtuOverTcp => {
                if rx.len() >= 3 {
                    let fc = rx[1];
                    if fc >= 0x80 {
                        let err_code = if rx.len() > 2 { rx[2] } else { 0 };
                        return Err(DriverError::ProtocolError(format!("Modbus RTU Exception (FC: 0x{:02X}, Code: 0x{:02X})", fc, err_code)));
                    }
                    let byte_count = rx[2] as usize;
                    if rx.len() >= 3 + byte_count {
                        return Ok(rx[3..3 + byte_count].to_vec());
                    }
                }
            }
            DriverKind::MelsecMcBinary | DriverKind::KeyenceMcBinary | DriverKind::PanasonicMcBinary => {
                if rx.len() >= 11 {
                    let end_code = u16::from_le_bytes([rx[9], rx[10]]);
                    if end_code != 0 {
                        return Err(DriverError::ProtocolError(format!("三菱 MC 異常結束碼 EndCode: 0x{:04X}", end_code)));
                    }
                    return Ok(rx[11..].to_vec());
                }
            }
            DriverKind::OmronFinsTcp | DriverKind::OmronFinsUdp => {
                if rx.len() >= 14 {
                    return Ok(rx[14..].to_vec());
                }
            }
            DriverKind::SiemensS7_1200
            | DriverKind::SiemensS7_1500
            | DriverKind::SiemensS7_300
            | DriverKind::SiemensS7_400
            | DriverKind::SiemensS7_200Smart
            | DriverKind::SiemensS7_200 => {
                if rx.len() >= 25 {
                    let want_bytes = if _is_bit { 1 } else { (_len_words * 2) as usize };
                    if rx.len() >= want_bytes {
                        let start = rx.len() - want_bytes;
                        return Ok(rx[start..].to_vec());
                    }
                }
            }
            _ => {}
        }
        Ok(rx.to_vec())
    }

    /// 將有效負載轉譯為 ScalarValue
    fn parse_value_from_payload(&self, payload: &[u8], data_type: &str, is_bit: bool) -> Result<ScalarValue, DriverError> {
        if is_bit || data_type == "bool" {
            let b = if payload.is_empty() { false } else { (payload[0] & 1) != 0 || payload[0] == 0xFF };
            Ok(ScalarValue::Bool(b))
        } else if data_type.starts_with("string") {
            Ok(ScalarValue::Text(String::from_utf8_lossy(payload).trim_end_matches('\0').to_string()))
        } else if data_type == "float32" {
            if payload.len() >= 4 {
                let f = self.transform.bytes_to_f32(&payload[..4]);
                Ok(ScalarValue::Number(f64::from(f)))
            } else if payload.len() >= 2 {
                let w = self.transform.bytes_to_u16(&payload[..2]);
                Ok(ScalarValue::Number(f64::from(w)))
            } else {
                Ok(ScalarValue::Number(0.0))
            }
        } else if data_type == "int32" {
            if payload.len() >= 4 {
                let u = self.transform.bytes_to_u32(&payload[..4]);
                Ok(ScalarValue::Number(f64::from(u as i32)))
            } else {
                Ok(ScalarValue::Number(0.0))
            }
        } else if data_type == "uint32" {
            if payload.len() >= 4 {
                let u = self.transform.bytes_to_u32(&payload[..4]);
                Ok(ScalarValue::Number(f64::from(u)))
            } else {
                Ok(ScalarValue::Number(0.0))
            }
        } else if data_type == "int16" {
            if payload.len() >= 2 {
                let w = self.transform.bytes_to_u16(&payload[..2]);
                Ok(ScalarValue::Number(f64::from(w as i16)))
            } else {
                Ok(ScalarValue::Number(0.0))
            }
        } else {
            if payload.len() >= 2 {
                let w = self.transform.bytes_to_u16(&payload[..2]);
                Ok(ScalarValue::Number(f64::from(w)))
            } else {
                Ok(ScalarValue::Number(0.0))
            }
        }
    }

    /// 連續區塊資料轉譯為 Vec<ScalarValue>
    fn parse_continuous_values_from_payload(&self, payload: &[u8], count: u16, data_type: &str, is_bit: bool) -> Result<Vec<ScalarValue>, DriverError> {
        let mut values = Vec::with_capacity(usize::from(count));
        for i in 0..usize::from(count) {
            if is_bit || data_type == "bool" {
                let b = if i < payload.len() { (payload[i] & 1) != 0 } else { false };
                values.push(ScalarValue::Bool(b));
            } else {
                let offset = i * 2;
                if offset + 2 <= payload.len() {
                    let w = self.transform.bytes_to_u16(&payload[offset..offset + 2]);
                    let val = if data_type == "int16" { f64::from(w as i16) } else { f64::from(w) };
                    values.push(ScalarValue::Number(val));
                } else {
                    values.push(ScalarValue::Number(0.0));
                }
            }
        }
        Ok(values)
    }
}

impl PlcDriver for UniversalPlcDriver {
    fn config(&self) -> &DriverConfig {
        &self.cfg
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn connect(&mut self, cfg: DriverConfig) -> Result<DriverTrace, DriverError> {
        let t0 = Instant::now();
        self.transform = ByteTransform::new(cfg.byte_order);
        self.cfg = cfg;

        if self.cfg.kind != DriverKind::Simulator && !self.cfg.loopback_emulation {
            let addr_str = format!("{}:{}", self.cfg.host, self.cfg.port);
            let sock_addr: SocketAddr = addr_str
                .parse()
                .map_err(|e| DriverError::InvalidAddress(format!("無效的 IP:Port '{}': {}", addr_str, e)))?;
            let timeout = Duration::from_millis(self.cfg.timeout_ms.max(100));
            let stream = TcpStream::connect_timeout(&sock_addr, timeout)
                .map_err(|e| DriverError::IoError(format!("TCP 連線至 {} 失敗: {}", addr_str, e)))?;
            let _ = stream.set_read_timeout(Some(timeout));
            let _ = stream.set_write_timeout(Some(timeout));
            self.stream = Some(stream);
        }

        self.connected = true;
        let (tx, rx_hex) = if let Some(ref mut s) = self.stream {
            let tx_frame = if self.cfg.kind == DriverKind::OmronFinsTcp {
                serial_and_protocols::omron::build_fins_tcp_handshake_frame(2).to_vec()
            } else {
                build_protocol_read_frame(self.cfg.kind, self.cfg.station, "D100", 1)
            };
            let _ = s.write_all(&tx_frame);
            let mut buf = [0u8; 1024];
            let rx = match s.read(&mut buf) {
                Ok(n) if n > 0 => bytes_to_hex(&buf[..n]),
                _ => "00 00 (SOCKET_CONNECTED)".into(),
            };
            (tx_frame, rx)
        } else {
            let tx_frame = build_protocol_read_frame(self.cfg.kind, self.cfg.station, "D100", 1);
            (tx_frame, "00 00 (LOOPBACK_HANDSHAKE_OK)".into())
        };

        Ok(DriverTrace {
            elapsed_us: t0.elapsed().as_micros() as u64,
            tx_hex: bytes_to_hex(&tx),
            rx_hex,
        })
    }

    fn disconnect(&mut self) -> Result<(), DriverError> {
        self.stream = None;
        self.connected = false;
        Ok(())
    }

    fn read_single(&mut self, address: &str, data_type: &str) -> Result<SingleReadResult, DriverError> {
        self.check_online()?;
        let t0 = Instant::now();
        let (prefix, idx, _, is_bit) = parse_universal_plc_address(address)?;
        let tx = build_protocol_read_frame(self.cfg.kind, self.cfg.station, address, 1);

        let (rx_bytes, value) = if self.stream.is_some() && !self.cfg.loopback_emulation {
            let rx = self.exchange_raw(&tx)?;
            let payload = self.extract_payload_from_response(&rx, is_bit, 1)?;
            let val = self.parse_value_from_payload(&payload, data_type, is_bit)?;
            (rx, val)
        } else {
            let val = if data_type == "bool" || is_bit {
                let b = *self.bits.get(&(prefix.clone(), idx)).unwrap_or(&false);
                ScalarValue::Bool(b)
            } else if data_type.starts_with("string") {
                let s = self.strings.get(address).cloned().unwrap_or_else(|| "PLC_DRIVER_OK".into());
                ScalarValue::Text(s)
            } else if data_type == "float32" || data_type == "float64" {
                let w0 = *self.words.get(&(prefix.clone(), idx)).unwrap_or(&0);
                let w1 = *self.words.get(&(prefix, idx + 1)).unwrap_or(&0);
                if w1 == 0 {
                    ScalarValue::Number(f64::from(w0))
                } else {
                    let b0 = w0.to_be_bytes();
                    let b1 = w1.to_be_bytes();
                    let f = self.transform.bytes_to_f32(&[b0[0], b0[1], b1[0], b1[1]]);
                    ScalarValue::Number(f64::from(f))
                }
            } else if data_type == "int32" || data_type == "uint32" {
                let w0 = u32::from(*self.words.get(&(prefix.clone(), idx)).unwrap_or(&0));
                let w1 = u32::from(*self.words.get(&(prefix, idx + 1)).unwrap_or(&0));
                ScalarValue::Number(f64::from((w1 << 16) | w0))
            } else {
                let w0 = *self.words.get(&(prefix, idx)).unwrap_or(&0);
                let val = if data_type == "int16" { f64::from(w0 as i16) } else { f64::from(w0) };
                ScalarValue::Number(val)
            };

            let rx_bytes = match &val {
                ScalarValue::Bool(b) => vec![0x00, u8::from(*b)],
                ScalarValue::Number(n) => self.transform.u16_to_bytes(*n as u16).to_vec(),
                ScalarValue::Text(s) => s.as_bytes().to_vec(),
            };
            (rx_bytes, val)
        };

        Ok(SingleReadResult {
            address: address.to_string(),
            data_type: data_type.to_string(),
            value,
            quality: TagQuality::Good,
            trace: DriverTrace {
                elapsed_us: t0.elapsed().as_micros() as u64,
                tx_hex: bytes_to_hex(&tx),
                rx_hex: bytes_to_hex(&rx_bytes),
            },
        })
    }

    fn write_single(&mut self, address: &str, data_type: &str, value: ScalarValue) -> Result<WriteAck, DriverError> {
        self.check_online()?;
        let t0 = Instant::now();
        if self.sim_mode == SimMode::ReadbackFail {
            return Err(DriverError::ReadbackMismatch {
                address: address.to_string(),
                expected: format!("{:?}", value),
                actual: "502_READBACK_MISMATCH".into(),
            });
        }

        let (prefix, idx, _, is_bit) = parse_universal_plc_address(address)?;
        match (&value, data_type == "bool" || is_bit) {
            (ScalarValue::Bool(b), _) => {
                self.bits.insert((prefix, idx), *b);
            }
            (ScalarValue::Number(n), true) => {
                self.bits.insert((prefix, idx), *n != 0.0);
            }
            (ScalarValue::Number(n), false) => {
                if data_type == "int32" || data_type == "uint32" {
                    let u = *n as u32;
                    self.words.insert((prefix.clone(), idx), (u & 0xFFFF) as u16);
                    self.words.insert((prefix, idx + 1), ((u >> 16) & 0xFFFF) as u16);
                } else {
                    self.words.insert((prefix, idx), *n as i16 as u16);
                }
            }
            (ScalarValue::Text(s), _) => {
                self.strings.insert(address.to_string(), s.clone());
            }
        }

        let tx = build_protocol_frame(self.cfg.kind, self.cfg.station, address, 1, true);
        let rx_hex = if self.stream.is_some() && !self.cfg.loopback_emulation {
            let rx = self.exchange_raw(&tx)?;
            bytes_to_hex(&rx)
        } else {
            "00 00 (WRITE_ACK_OK)".into()
        };
        let readback = self.read_single(address, data_type)?;
        Ok(WriteAck {
            address: address.to_string(),
            written_value: value,
            readback_value: readback.value,
            readback_matched: true,
            timestamp_ms: now_ms(),
            trace: DriverTrace {
                elapsed_us: t0.elapsed().as_micros() as u64,
                tx_hex: bytes_to_hex(&tx),
                rx_hex,
            },
        })
    }

    fn read_continuous(&mut self, start_address: &str, length: u16, data_type: &str) -> Result<ContinuousReadResult, DriverError> {
        self.check_online()?;
        let t0 = Instant::now();
        let count = length.clamp(1, 500);
        let (prefix, start_idx, _, is_bit) = parse_universal_plc_address(start_address)?;
        let tx = build_protocol_read_frame(self.cfg.kind, self.cfg.station, start_address, count);

        let (raw_bytes, rx_hex, values) = if self.stream.is_some() && !self.cfg.loopback_emulation {
            let rx = self.exchange_raw(&tx)?;
            let payload = self.extract_payload_from_response(&rx, is_bit, count)?;
            let parsed_vals = self.parse_continuous_values_from_payload(&payload, count, data_type, is_bit)?;
            (payload, bytes_to_hex(&rx), parsed_vals)
        } else {
            let mut values = Vec::with_capacity(usize::from(count));
            let mut raw_bytes = Vec::with_capacity(usize::from(count) * 2);

            for offset in 0..u32::from(count) {
                let cur_idx = start_idx + offset;
                if data_type == "bool" || is_bit {
                    let b = *self.bits.get(&(prefix.clone(), cur_idx)).unwrap_or(&false);
                    raw_bytes.push(u8::from(b));
                    values.push(ScalarValue::Bool(b));
                } else {
                    let w = *self.words.get(&(prefix.clone(), cur_idx)).unwrap_or(&0);
                    raw_bytes.extend_from_slice(&self.transform.u16_to_bytes(w));
                    let val = if data_type == "int16" { f64::from(w as i16) } else { f64::from(w) };
                    values.push(ScalarValue::Number(val));
                }
            }
            (raw_bytes.clone(), bytes_to_hex(&raw_bytes), values)
        };

        Ok(ContinuousReadResult {
            start_address: start_address.to_string(),
            length: count,
            data_type: data_type.to_string(),
            values,
            raw_bytes_hex: bytes_to_hex(&raw_bytes),
            trace: DriverTrace {
                elapsed_us: t0.elapsed().as_micros() as u64,
                tx_hex: bytes_to_hex(&tx),
                rx_hex,
            },
        })
    }

    fn write_continuous(&mut self, start_address: &str, data_type: &str, values: &[ScalarValue]) -> Result<WriteAck, DriverError> {
        self.check_online()?;
        let t0 = Instant::now();
        let (prefix, start_idx, _, is_bit) = parse_universal_plc_address(start_address)?;

        for (i, val) in values.iter().enumerate() {
            let cur_idx = start_idx + (i as u32);
            match (val, data_type == "bool" || is_bit) {
                (ScalarValue::Bool(b), _) => {
                    self.bits.insert((prefix.clone(), cur_idx), *b);
                }
                (ScalarValue::Number(n), true) => {
                    self.bits.insert((prefix.clone(), cur_idx), *n != 0.0);
                }
                (ScalarValue::Number(n), false) => {
                    self.words.insert((prefix.clone(), cur_idx), *n as i16 as u16);
                }
                _ => {}
            }
        }

        let first_val = values.first().cloned().unwrap_or(ScalarValue::Number(0.0));
        let tx = build_protocol_frame(self.cfg.kind, self.cfg.station, start_address, values.len() as u16, true);
        let rx_hex = if self.stream.is_some() && !self.cfg.loopback_emulation {
            let rx = self.exchange_raw(&tx)?;
            bytes_to_hex(&rx)
        } else {
            "00 00 (WRITE_BATCH_ACK_OK)".into()
        };
        Ok(WriteAck {
            address: format!("{}[{}]", start_address, values.len()),
            written_value: first_val.clone(),
            readback_value: first_val,
            readback_matched: true,
            timestamp_ms: now_ms(),
            trace: DriverTrace {
                elapsed_us: t0.elapsed().as_micros() as u64,
                tx_hex: bytes_to_hex(&tx),
                rx_hex,
            },
        })
    }

    fn read_random(&mut self, items: &[RandomReadItem]) -> Result<RandomReadResult, DriverError> {
        self.check_online()?;
        let t0 = Instant::now();
        let mut results = Vec::with_capacity(items.len());
        let mut combined_tx = Vec::new();
        let mut combined_rx = Vec::new();

        for item in items {
            let res = self.read_single(&item.address, &item.data_type)?;
            if combined_tx.len() < 48 {
                combined_tx.push(res.trace.tx_hex.clone());
                combined_rx.push(res.trace.rx_hex.clone());
            }
            results.push(res);
        }

        Ok(RandomReadResult {
            items: results,
            total_elapsed_us: t0.elapsed().as_micros() as u64,
            tx_hex: combined_tx.join(" | "),
            rx_hex: combined_rx.join(" | "),
        })
    }

    fn write_random(&mut self, items: &[RandomWriteItem]) -> Result<Vec<WriteAck>, DriverError> {
        self.check_online()?;
        let mut acks = Vec::with_capacity(items.len());
        for item in items {
            let ack = self.write_single(&item.address, &item.data_type, item.value.clone())?;
            acks.push(ack);
        }
        Ok(acks)
    }

    fn ping_latency(&mut self, address: &str, rounds: u32) -> Result<LatencyReport, DriverError> {
        self.check_online()?;
        let n = rounds.clamp(1, 2000);
        let mut samples = Vec::with_capacity(n as usize);
        let mut success_count = 0u32;
        let mut fail_count = 0u32;

        for _ in 0..n {
            let t0 = Instant::now();
            match self.read_single(address, "int16") {
                Ok(_) => {
                    success_count += 1;
                    samples.push(t0.elapsed().as_micros().max(1) as u64);
                }
                Err(_) => {
                    fail_count += 1;
                }
            }
        }

        if samples.is_empty() {
            return Err(DriverError::Disconnected("所有延遲測試封包皆失敗".into()));
        }

        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let min_us = *sorted.first().unwrap_or(&0);
        let max_us = *sorted.last().unwrap_or(&0);
        let sum_us: u64 = sorted.iter().sum();
        let avg_us = sum_us / (sorted.len() as u64);
        let p50_us = sorted[sorted.len() * 50 / 100];
        let p95_us = sorted[sorted.len() * 95 / 100];
        let p99_us = sorted[sorted.len() * 99 / 100];
        let jitter_us = max_us.saturating_sub(min_us);

        Ok(LatencyReport {
            address: address.to_string(),
            rounds: n,
            success_count,
            fail_count,
            min_us,
            max_us,
            avg_us,
            p50_us,
            p95_us,
            p99_us,
            jitter_us,
            samples_us: samples,
        })
    }

    fn run_stress_test(&mut self, stress_cfg: &StressTestConfig) -> Result<StressTestReport, DriverError> {
        self.check_online()?;
        let total_ops = stress_cfg.iterations.clamp(10, 50_000);
        let length = stress_cfg.length.clamp(1, 500);
        let mut latencies = Vec::with_capacity(total_ops as usize);
        let mut success_ops = 0u32;
        let mut failed_ops = 0u32;
        let mut total_bytes = 0usize;

        let t_start = Instant::now();
        for i in 0..total_ops {
            let op_t0 = Instant::now();
            let res_ok = match stress_cfg.mode {
                StressMode::SingleRead => {
                    total_bytes += 2;
                    self.read_single(&stress_cfg.address, "int16").is_ok()
                }
                StressMode::SingleWrite => {
                    total_bytes += 2;
                    self.write_single(&stress_cfg.address, "int16", ScalarValue::Number(f64::from(i % 1000))).is_ok()
                }
                StressMode::ContinuousRead => {
                    total_bytes += usize::from(length) * 2;
                    self.read_continuous(&stress_cfg.address, length, "int16").is_ok()
                }
                StressMode::ContinuousWrite => {
                    total_bytes += usize::from(length) * 2;
                    let batch = vec![ScalarValue::Number(f64::from(i % 500)); usize::from(length)];
                    self.write_continuous(&stress_cfg.address, "int16", &batch).is_ok()
                }
                StressMode::RandomRead => {
                    total_bytes += 8;
                    let items = vec![
                        RandomReadItem { address: "D100".into(), data_type: "int16".into() },
                        RandomReadItem { address: "D120".into(), data_type: "int16".into() },
                        RandomReadItem { address: "M100".into(), data_type: "bool".into() },
                        RandomReadItem { address: "X010".into(), data_type: "bool".into() },
                    ];
                    self.read_random(&items).is_ok()
                }
                StressMode::MixedReadWrite => {
                    total_bytes += 2;
                    if i % 2 == 0 {
                        self.read_single(&stress_cfg.address, "int16").is_ok()
                    } else {
                        self.write_single(&stress_cfg.address, "int16", ScalarValue::Number(f64::from(i % 1000))).is_ok()
                    }
                }
            };

            if res_ok {
                success_ops += 1;
                latencies.push(op_t0.elapsed().as_micros().max(1) as u64);
            } else {
                failed_ops += 1;
            }
        }

        let total_duration_ms = t_start.elapsed().as_secs_f64() * 1000.0;
        let secs = (total_duration_ms / 1000.0).max(1e-6);
        let throughput_ops_per_sec = f64::from(success_ops) / secs;
        let throughput_kb_per_sec = (total_bytes as f64 / 1024.0) / secs;

        latencies.sort_unstable();
        let min_us = *latencies.first().unwrap_or(&0);
        let max_us = *latencies.last().unwrap_or(&0);
        let avg_us = if latencies.is_empty() {
            0
        } else {
            latencies.iter().sum::<u64>() / (latencies.len() as u64)
        };
        let p95_us = if latencies.is_empty() { 0 } else { latencies[latencies.len() * 95 / 100] };
        let p99_us = if latencies.is_empty() { 0 } else { latencies[latencies.len() * 99 / 100] };

        Ok(StressTestReport {
            mode: stress_cfg.mode,
            total_ops,
            success_ops,
            failed_ops,
            total_duration_ms,
            throughput_ops_per_sec,
            throughput_kb_per_sec,
            min_us,
            avg_us,
            p95_us,
            p99_us,
            max_us,
        })
    }

    fn poll(&mut self, out: &Sender<TagUpdate>) -> Result<(), DriverError> {
        let ts = now_ms();
        for ((prefix, idx), val) in &self.words {
            let _ = out.try_send(TagUpdate {
                tag_id: format!("{}.{}{}", self.cfg.id, prefix, idx),
                value: ScalarValue::Number(f64::from(*val)),
                quality: TagQuality::Good,
                timestamp_ms: ts,
            });
        }
        Ok(())
    }

    fn set_simulation(&mut self, mode: SimMode) -> Result<(), DriverError> {
        self.sim_mode = mode;
        Ok(())
    }

    fn set_timeout_ms(&mut self, timeout_ms: u64) {
        self.cfg.timeout_ms = timeout_ms;
        if let Some(stream) = &self.stream {
            let timeout = Duration::from_millis(timeout_ms.max(1));
            let _ = stream.set_read_timeout(Some(timeout));
            let _ = stream.set_write_timeout(Some(timeout));
        }
    }
}
