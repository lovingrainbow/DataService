// ArgonGUI/crates/mm-drivers/src/serial_and_protocols.rs
#![forbid(unsafe_code)]

use std::time::Duration;
use serde::{Deserialize, Serialize};
use crate::{DriverError};

// ============================================================================
// 1. RS-232 / RS-485 (tokio-serial) 通訊參數與傳輸設定
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Tcp,
    Udp,
    SerialRs232,
    SerialRs485,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerialParity {
    None,
    Even,
    Odd,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerialPortConfig {
    /// 例如 Windows: "COM3"，Linux: "/dev/ttyUSB0"
    pub port_name: String,
    /// 波特率：9600, 19200, 38400, 57600, 115200
    pub baud_rate: u32,
    /// 資料位元：7 (三菱 FX / Modbus ASCII / Omron HostLink 常用) 或 8 (Modbus RTU / 匯川常用)
    pub data_bits: u8,
    /// 同位元檢查：None, Even, Odd
    pub parity: SerialParity,
    /// 停止位元：1 或 2
    pub stop_bits: u8,
    /// RS-485 硬體方向控制 (RTS Toggle Before Send)
    pub rs485_rts_enable: bool,
    /// 幀間靜默收包等待時間 (Modbus RTU t3.5 預設 5~15 ms)
    pub inter_frame_silence_ms: u64,
}

impl Default for SerialPortConfig {
    fn default() -> Self {
        Self {
            port_name: "COM3".into(),
            baud_rate: 19200,
            data_bits: 8,
            parity: SerialParity::Even,
            stop_bits: 1,
            rs485_rts_enable: true,
            inter_frame_silence_ms: 10,
        }
    }
}

/// 透過 `tokio_serial` 建立跨平台序列埠連線（支援非同步與同步收發）
pub fn open_tokio_serial_builder(cfg: &SerialPortConfig) -> tokio_serial::SerialPortBuilder {
    let data_bits = match cfg.data_bits {
        7 => tokio_serial::DataBits::Seven,
        _ => tokio_serial::DataBits::Eight,
    };
    let parity = match cfg.parity {
        SerialParity::None => tokio_serial::Parity::None,
        SerialParity::Even => tokio_serial::Parity::Even,
        SerialParity::Odd => tokio_serial::Parity::Odd,
    };
    let stop_bits = match cfg.stop_bits {
        2 => tokio_serial::StopBits::Two,
        _ => tokio_serial::StopBits::One,
    };

    tokio_serial::new(&cfg.port_name, cfg.baud_rate)
        .data_bits(data_bits)
        .parity(parity)
        .stop_bits(stop_bits)
        .flow_control(tokio_serial::FlowControl::None)
        .timeout(Duration::from_millis(1000))
}

// ============================================================================
// 2. 檢核碼引擎 (CRC-16 / LRC / Mitsubishi SumCheck / Omron HostLink FCS)
// ============================================================================

pub mod checksum {
    /// 1. 標準 Modbus RTU CRC-16 (多項式 0xA001，回傳 [CRC_Lo, CRC_Hi])
    pub fn crc16_modbus(data: &[u8]) -> [u8; 2] {
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

    pub fn verify_crc16_modbus(frame: &[u8]) -> bool {
        if frame.len() < 4 {
            return false;
        }
        let (payload, crc_bytes) = frame.split_at(frame.len() - 2);
        let expected = crc16_modbus(payload);
        crc_bytes[0] == expected[0] && crc_bytes[1] == expected[1]
    }

    /// 2. 標準 Modbus ASCII LRC 縱向冗餘校驗 (二補數總和)
    pub fn lrc_byte(data: &[u8]) -> u8 {
        let sum: u8 = data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
        (!sum).wrapping_add(1)
    }

    /// 3. 三菱 FX 編程口與 FX Links 專用和校驗 (取總和低 8 位元轉 2 碼大寫 Hex ASCII)
    pub fn melsec_sum_check_ascii(data: &[u8]) -> [u8; 2] {
        let sum: u32 = data.iter().map(|&b| u32::from(b)).sum();
        let hex = format!("{:02X}", sum & 0xFF);
        let bytes = hex.as_bytes();
        [bytes[0], bytes[1]]
    }

    /// 4. 歐姆龍 HostLink FCS (Frame Check Sequence: 8-bit XOR 轉 2 碼大寫 Hex ASCII)
    pub fn omron_fcs_ascii(data: &[u8]) -> [u8; 2] {
        let xor_val: u8 = data.iter().fold(0u8, |acc, &b| acc ^ b);
        let hex = format!("{:02X}", xor_val);
        let bytes = hex.as_bytes();
        [bytes[0], bytes[1]]
    }
}

// ============================================================================
// 3. 三菱 FX 系列：MelsecFxSerial (編程口) & MelsecFxLinks (485BD/ComputerLink)
// ============================================================================

pub mod melsec_fx {
    use super::*;
    use super::checksum::melsec_sum_check_ascii;

    /// 支援 FX3U / FX3G / FX3S 新版編程口
    pub fn fx_serial_word_byte_offset(prefix: &str, index: u32, is_new_version: bool) -> Result<(bool, u16), DriverError> {
        match prefix {
            "D" => {
                if index >= 8000 {
                    Ok((false, 0x0E00 + ((index - 8000) as u16) * 2))
                } else if is_new_version {
                    // 新版 FX3U/FX3G 使用 E00 / E10 擴充指令，D0 起始於 0x4000
                    Ok((true, 0x4000 + (index as u16) * 2))
                } else {
                    Ok((false, 0x1000 + (index as u16) * 2))
                }
            }
            "T" | "TN" => Ok((false, 0x0800 + (index as u16) * 2)),
            "C" | "CN" => {
                if index >= 200 {
                    Ok((false, 0x0C00 + ((index - 200) as u16) * 4))
                } else {
                    Ok((false, 0x0A00 + (index as u16) * 2))
                }
            }
            other => Err(DriverError::InvalidAddress(format!("FX 編程口字元讀寫不支援元件 '{}'", other))),
        }
    }

    /// 用於 Force ON 0x37 / Force OFF 0x38
    pub fn fx_serial_bit_force_offset(prefix: &str, index: u32) -> Result<u16, DriverError> {
        match prefix {
            "X" => Ok(0x0400 + (index as u16)), // index 已由八進位解析
            "Y" => Ok(0x0500 + (index as u16)),
            "M" => {
                if index >= 8000 {
                    Ok(0x0F00 + ((index - 8000) as u16))
                } else {
                    Ok(0x0800 + (index as u16))
                }
            }
            "S" => Ok(0x0000 + (index as u16)),
            "T" | "TS" => Ok(0x0600 + (index as u16)),
            "C" | "CS" => Ok(0x0E00 + (index as u16)),
            other => Err(DriverError::InvalidAddress(format!("FX 編程口位元強制不支援元件 '{}'", other))),
        }
    }

    /// 建立三菱 FX 編程口讀取報文 (`STX(0x02) + Cmd('0' 或 "E00") + Addr(4 Hex) + ByteLen(2 Hex) + ETX(0x03) + SumCheck(2 Hex)`)
    pub fn build_fx_serial_read_frame(address: &str, word_count: u16, is_new_version: bool) -> Result<Vec<u8>, DriverError> {
        let (prefix, idx, _, is_bit) = crate::parse_universal_plc_address(address)?;
        let (use_ext_cmd, byte_offset, byte_len) = if is_bit {
            let bit_addr = fx_serial_bit_force_offset(&prefix, idx)?;
            (false, bit_addr / 8, ((word_count + 7) / 8).max(1) as u8)
        } else {
            let (ext, off) = fx_serial_word_byte_offset(&prefix, idx, is_new_version)?;
            (ext, off, (word_count * 2).min(64) as u8)
        };

        let mut body = Vec::with_capacity(14);
        if use_ext_cmd {
            body.extend_from_slice(b"E00");
        } else {
            body.push(b'0');
        }
        body.extend_from_slice(format!("{:04X}{:02X}", byte_offset, byte_len).as_bytes());
        body.push(0x03); // ETX

        let sum = melsec_sum_check_ascii(&body);
        let mut frame = vec![0x02]; // STX
        frame.extend_from_slice(&body);
        frame.extend_from_slice(&sum);
        Ok(frame)
    }

    /// 建立三菱 FX 編程口寫入報文：
    /// - 單點 Bit：使用 `'7'` (0x37 Force ON) / `'8'` (0x38 Force OFF)，位址高地位元組順序為 `[LoByteHex][HiByteHex]`
    /// - Word 暫存器：使用 `'1'` 或 `"E10"` + `Addr(4 Hex)` + `ByteLen(2 Hex)` + `DataHex` + `ETX(0x03)` + `SumCheck(2 Hex)`
    pub fn build_fx_serial_write_frame(
        address: &str,
        is_bit_write: bool,
        bit_val: bool,
        word_bytes: &[u8],
        is_new_version: bool,
    ) -> Result<Vec<u8>, DriverError> {
        let (prefix, idx, _, _) = crate::parse_universal_plc_address(address)?;
        let mut body = Vec::new();

        if is_bit_write {
            let bit_addr = fx_serial_bit_force_offset(&prefix, idx)?;
            body.push(if bit_val { b'7' } else { b'8' });
            // FX Force ON/OFF 特殊順序：先傳低位元組 2 Hex，再傳高位元組 2 Hex
            body.extend_from_slice(format!("{:02X}{:02X}", bit_addr & 0xFF, (bit_addr >> 8) & 0xFF).as_bytes());
            body.push(0x03);
        } else {
            let (use_ext_cmd, byte_offset) = fx_serial_word_byte_offset(&prefix, idx, is_new_version)?;
            if use_ext_cmd {
                body.extend_from_slice(b"E10");
            } else {
                body.push(b'1');
            }
            body.extend_from_slice(format!("{:04X}{:02X}", byte_offset, word_bytes.len()).as_bytes());
            for &b in word_bytes {
                body.extend_from_slice(format!("{:02X}", b).as_bytes());
            }
            body.push(0x03);
        }

        let sum = melsec_sum_check_ascii(&body);
        let mut frame = vec![0x02];
        frame.extend_from_slice(&body);
        frame.extend_from_slice(&sum);
        Ok(frame)
    }

    /// 建立三菱 FX Computer Link (`MelsecFxLinks` 485BD/485ADP) 報文
    /// 格式：`ENQ(0x05) + Station(2) + PC("FF") + Cmd("BR"/"WR"/"BW"/"WW") + Wait('0') + Addr(5) + Len(2) + [Data] + SumCheck(2)`
    pub fn build_fx_links_frame(
        station: u8,
        address: &str,
        count: u16,
        write_words: Option<&[u16]>,
        sum_check_enable: bool,
    ) -> Result<Vec<u8>, DriverError> {
        let (prefix, idx, _, is_bit) = crate::parse_universal_plc_address(address)?;
        let addr_5 = if prefix == "X" || prefix == "Y" {
            format!("{}{:04o}", prefix, idx)
        } else {
            format!("{}{:04}", prefix, idx)
        };

        let cmd = match (is_bit, write_words.is_some()) {
            (true, false) => "BR",
            (false, false) => "WR",
            (true, true) => "BW",
            (false, true) => "WW",
        };

        let mut body = format!("{:02X}FF{}0{}{:02X}", station, cmd, addr_5, count & 0xFF).into_bytes();
        if let Some(words) = write_words {
            for &w in words {
                if is_bit {
                    body.push(if w != 0 { b'1' } else { b'0' });
                } else {
                    body.extend_from_slice(format!("{:04X}", w).as_bytes());
                }
            }
        }

        let mut frame = vec![0x05]; // ENQ
        frame.extend_from_slice(&body);
        if sum_check_enable {
            let sum = melsec_sum_check_ascii(&body);
            frame.extend_from_slice(&sum);
        }
        Ok(frame)
    }
}

// ============================================================================
// 4. Modbus RTU 與 Modbus ASCII (RS-232 / RS-485) 完整編解碼器
// ============================================================================

pub mod modbus {
    use super::checksum::{crc16_modbus, lrc_byte};

    /// 建立標準 Modbus RTU 二進位報文（自動附加 CRC-16）
    pub fn build_rtu_frame(station: u8, function_code: u8, pdu_body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::with_capacity(2 + pdu_body.len() + 2);
        frame.push(station);
        frame.push(function_code);
        frame.extend_from_slice(pdu_body);
        let crc = crc16_modbus(&frame);
        frame.extend_from_slice(&crc);
        frame
    }

    /// 建立標準 Modbus ASCII 報文（`:` + HexASCII(Station + FC + Body + LRC) + `\r\n`）
    pub fn build_ascii_frame(station: u8, function_code: u8, pdu_body: &[u8]) -> Vec<u8> {
        let mut core = Vec::with_capacity(2 + pdu_body.len());
        core.push(station);
        core.push(function_code);
        core.extend_from_slice(pdu_body);
        let lrc = lrc_byte(&core);
        core.push(lrc);

        let mut ascii = Vec::with_capacity(1 + core.len() * 2 + 2);
        ascii.push(b':');
        for b in core {
            ascii.extend_from_slice(format!("{:02X}", b).as_bytes());
        }
        ascii.extend_from_slice(b"\r\n");
        ascii
    }

    /// 建立讀取線圈 (0x01/0x02) 或保持/輸入暫存器 (0x03/0x04) 之 PDU Body
    pub fn build_read_pdu_body(start_addr: u16, count: u16) -> [u8; 4] {
        [
            (start_addr >> 8) as u8,
            (start_addr & 0xFF) as u8,
            (count >> 8) as u8,
            (count & 0xFF) as u8,
        ]
    }

    /// 建立單點寫入線圈 (0x05) 或單一暫存器 (0x06) 之 PDU Body
    pub fn build_write_single_pdu_body(addr: u16, val: u16, is_coil: bool) -> [u8; 4] {
        let v = if is_coil { if val != 0 { 0xFF00 } else { 0x0000 } } else { val };
        [
            (addr >> 8) as u8,
            (addr & 0xFF) as u8,
            (v >> 8) as u8,
            (v & 0xFF) as u8,
        ]
    }

    /// 建立連續寫入多個保持暫存器 (0x10) 之 PDU Body
    pub fn build_write_multiple_registers_pdu_body(start_addr: u16, word_bytes: &[u8]) -> Vec<u8> {
        let word_count = (word_bytes.len() / 2) as u16;
        let mut body = Vec::with_capacity(5 + word_bytes.len());
        body.push((start_addr >> 8) as u8);
        body.push((start_addr & 0xFF) as u8);
        body.push((word_count >> 8) as u8);
        body.push((word_count & 0xFF) as u8);
        body.push(word_bytes.len() as u8);
        body.extend_from_slice(word_bytes);
        body
    }
}

// ============================================================================
// 5. Omron FINS (TCP / UDP) & Omron HostLink (RS-232/485 Serial) 完整轉譯
// ============================================================================

pub mod omron {
    use super::*;
    use super::checksum::omron_fcs_ascii;

    /// 歐姆龍記憶體區域碼
    pub fn parse_omron_fins_address(address: &str, is_bit: bool) -> Result<(u8, u16, u8), DriverError> {
        let s = address.trim().to_ascii_uppercase();
        let (prefix, rest) = if let Some(r) = s.strip_prefix("CIO") {
            ("CIO", r)
        } else if let Some(r) = s.strip_prefix("DM").or_else(|| s.strip_prefix("D")) {
            ("DM", r)
        } else if let Some(r) = s.strip_prefix("WR").or_else(|| s.strip_prefix("W")) {
            ("WR", r)
        } else if let Some(r) = s.strip_prefix("HR").or_else(|| s.strip_prefix("H")) {
            ("HR", r)
        } else if let Some(r) = s.strip_prefix("AR").or_else(|| s.strip_prefix("A")) {
            ("AR", r)
        } else if let Some(r) = s.strip_prefix("EM").or_else(|| s.strip_prefix("E")) {
            ("EM", r)
        } else {
            ("DM", s.as_str())
        };

        let area_code: u8 = match (prefix, is_bit) {
            ("DM", false) => 0x82,
            ("DM", true) => 0x02,
            ("CIO", false) => 0xB0,
            ("CIO", true) => 0x30,
            ("WR", false) => 0xB1,
            ("WR", true) => 0x31,
            ("HR", false) => 0xB2,
            ("HR", true) => 0x32,
            ("AR", false) => 0xB3,
            ("AR", true) => 0x33,
            ("EM", false) => 0xA0,
            ("EM", true) => 0x20,
            _ => 0x82,
        };

        let (word_str, bit_str) = rest.split_once('.').unwrap_or((rest, "0"));
        let word_addr: u16 = word_str.parse().unwrap_or(0);
        let bit_addr: u8 = bit_str.parse().unwrap_or(0);
        Ok((area_code, word_addr, bit_addr))
    }

    /// FINS/TCP 連線節點自動握手請求封包 (20 Bytes)
    pub fn build_fins_tcp_handshake_frame(client_node: u8) -> [u8; 20] {
        [
            0x46, 0x49, 0x4E, 0x53, // "FINS"
            0x00, 0x00, 0x00, 0x0C, // Length = 12
            0x00, 0x00, 0x00, 0x00, // Command = 0 (Node Address Data Send)
            0x00, 0x00, 0x00, 0x00, // Error Code = 0
            0x00, 0x00, 0x00, client_node,
        ]
    }

    /// 建立 FINS 核心指令 (`01 01` 讀取 / `01 02` 寫入)
    pub fn build_fins_core_command(
        address: &str,
        count: u16,
        is_bit: bool,
        write_data: Option<&[u8]>,
    ) -> Result<Vec<u8>, DriverError> {
        let (area, word_addr, bit_addr) = parse_omron_fins_address(address, is_bit)?;
        let mut cmd = Vec::with_capacity(8 + write_data.map(|d| d.len()).unwrap_or(0));
        cmd.push(0x01); // MRC
        cmd.push(if write_data.is_some() { 0x02 } else { 0x01 }); // SRC
        cmd.push(area);
        cmd.push((word_addr >> 8) as u8);
        cmd.push((word_addr & 0xFF) as u8);
        cmd.push(bit_addr);
        cmd.push((count >> 8) as u8);
        cmd.push((count & 0xFF) as u8);
        if let Some(data) = write_data {
            cmd.extend_from_slice(data);
        }
        Ok(cmd)
    }

    /// 建立 FINS `01 04` 多點隨機讀取指令 (Multiple Memory Area Read)
    pub fn build_fins_multiple_read_command(addresses: &[String]) -> Result<Vec<u8>, DriverError> {
        let mut cmd = Vec::with_capacity(2 + addresses.len() * 4);
        cmd.push(0x01); // MRC
        cmd.push(0x04); // SRC = 0x04 (Multiple Memory Area Read)
        for addr in addresses {
            let is_bit = addr.contains('.');
            let (area, word_addr, bit_addr) = parse_omron_fins_address(addr, is_bit)?;
            cmd.push(area);
            cmd.push((word_addr >> 8) as u8);
            cmd.push((word_addr & 0xFF) as u8);
            cmd.push(bit_addr);
        }
        Ok(cmd)
    }

    /// 將 FINS 核心指令包裝為 **Omron HostLink (RS-232/485 Serial)** 報文
    /// 格式：`@` + `UnitNo(2 Hex)` + `FA` + `ResponseWait('0')` + `ICF(00)DA2(00)SA2(00)SID(00)` + `FINS_Hex` + `FCS(2 Hex)` + `*\r`
    pub fn pack_omron_hostlink_serial(unit_no: u8, fins_core: &[u8]) -> Vec<u8> {
        let mut body = format!("@{:02X}FA000000000", unit_no).into_bytes();
        for &b in fins_core {
            body.extend_from_slice(format!("{:02X}", b).as_bytes());
        }
        let fcs = omron_fcs_ascii(&body);
        body.extend_from_slice(&fcs);
        body.extend_from_slice(b"*\r");
        body
    }
}

// ============================================================================
// 6. 匯川 Inovance 全系列 (Easy / H3U / H5U / AM) 位址轉譯與 RS-232/485 封裝
// ============================================================================

pub mod inovance {
    use super::*;
    use super::modbus::{
        build_ascii_frame, build_read_pdu_body, build_rtu_frame,
        build_write_multiple_registers_pdu_body, build_write_single_pdu_body,
    };

    /// 匯川 PLC 系列別
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum InovanceSeries {
        /// Easy320 / Easy521 系列
        Easy,
        /// H3U 標準系列
        H3U,
        /// H5U 高階系列
        H5U,
        /// AM400 / AM600 / AM800 中大型系列 (IEC 61131-3 定址 MW/MX/IW/QW)
        AM,
    }

    /// 將匯川軟元件位址 (`D100`, `M100`, `X010`, `Y010`, `R100`, `B10`, `S10`, `SM0`, `SD0`, `MW100`)
    /// 轉換為底層 Modbus `(是否位元元件, Modbus 0-based 偏移位址)`
    pub fn translate_inovance_address(
        series: InovanceSeries,
        address: &str,
    ) -> Result<(bool, u16), DriverError> {
        let s = address.trim().to_ascii_uppercase();
        if s.is_empty() {
            return Err(DriverError::InvalidAddress("匯川位址不得為空".into()));
        }

        match series {
            InovanceSeries::AM => {
                if let Some(rest) = s.strip_prefix("MX") {
                    let (word_s, bit_s) = rest.split_once('.').unwrap_or((rest, "0"));
                    let w: u16 = word_s.parse().unwrap_or(0);
                    let b: u16 = bit_s.parse().unwrap_or(0);
                    return Ok((true, w * 16 + b));
                }
                if let Some(rest) = s.strip_prefix("MW").or_else(|| s.strip_prefix("D")) {
                    let w: u16 = rest.parse().unwrap_or(0);
                    return Ok((false, w));
                }
                if let Some(rest) = s.strip_prefix("M") {
                    let b: u16 = rest.parse().unwrap_or(0);
                    return Ok((true, b));
                }
            }
            InovanceSeries::Easy | InovanceSeries::H5U => {
                // Easy & H5U 軟元件映射表
                if let Some(r) = s.strip_prefix("X") {
                    let oct = u16::from_str_radix(r, 8).map_err(|_| {
                        DriverError::InvalidAddress(format!("匯川 X 輸入 '{}' 必須為八進位", address))
                    })?;
                    return Ok((true, 0xF800 + oct));
                }
                if let Some(r) = s.strip_prefix("Y") {
                    let oct = u16::from_str_radix(r, 8).map_err(|_| {
                        DriverError::InvalidAddress(format!("匯川 Y 輸出 '{}' 必須為八進位", address))
                    })?;
                    return Ok((true, 0xFC00 + oct));
                }
                if let Some(r) = s.strip_prefix("M") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((true, idx)); // M0..M7999 -> 0x0000..0x1F3F
                }
                if let Some(r) = s.strip_prefix("B") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((true, 0x3000 + idx));
                }
                if let Some(r) = s.strip_prefix("S") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((true, 0xE000 + idx));
                }
                if let Some(r) = s.strip_prefix("D") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, idx)); // D0..D7999 -> 0x0000..0x1F3F
                }
                if let Some(r) = s.strip_prefix("R") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, 0x3000 + idx));
                }
            }
            InovanceSeries::H3U => {
                // H3U 軟元件映射表
                if let Some(r) = s.strip_prefix("X") {
                    let oct = u16::from_str_radix(r, 8).map_err(|_| {
                        DriverError::InvalidAddress(format!("H3U X 輸入 '{}' 必須為八進位", address))
                    })?;
                    return Ok((true, 0xF800 + oct));
                }
                if let Some(r) = s.strip_prefix("Y") {
                    let oct = u16::from_str_radix(r, 8).map_err(|_| {
                        DriverError::InvalidAddress(format!("H3U Y 輸出 '{}' 必須為八進位", address))
                    })?;
                    return Ok((true, 0xFC00 + oct));
                }
                if let Some(r) = s.strip_prefix("SM") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((true, 0x2400 + idx));
                }
                if let Some(r) = s.strip_prefix("SD") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, 0x2400 + idx));
                }
                if let Some(r) = s.strip_prefix("M") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    let offset = if idx >= 8000 { 0x1F40 + (idx - 8000) } else { idx };
                    return Ok((true, offset));
                }
                if let Some(r) = s.strip_prefix("S") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((true, 0xE000 + idx));
                }
                if let Some(r) = s.strip_prefix("T") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, 0xF000 + idx));
                }
                if let Some(r) = s.strip_prefix("C") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, 0xF400 + idx));
                }
                if let Some(r) = s.strip_prefix("D") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, idx));
                }
                if let Some(r) = s.strip_prefix("R") {
                    let idx: u16 = r.parse().unwrap_or(0);
                    return Ok((false, 0x3000 + idx));
                }
            }
        }

        Err(DriverError::InvalidAddress(format!(
            "匯川 {:?} 系列不支援軟元件位址 '{}'",
            series, address
        )))
    }

    /// 產生匯川 RS-232 / RS-485 RTU 或 ASCII 讀取報文
    pub fn build_inovance_serial_read_frame(
        series: InovanceSeries,
        station: u8,
        address: &str,
        count: u16,
        use_ascii_mode: bool,
    ) -> Result<(u16, Vec<u8>), DriverError> {
        let (is_bit, modbus_addr) = translate_inovance_address(series, address)?;
        let fc = if is_bit { 0x01 } else { 0x03 };
        let body = build_read_pdu_body(modbus_addr, count);
        let frame = if use_ascii_mode {
            build_ascii_frame(station, fc, &body)
        } else {
            build_rtu_frame(station, fc, &body)
        };
        Ok((modbus_addr, frame))
    }

    /// 產生匯川 RS-232 / RS-485 RTU 或 ASCII 寫入報文
    pub fn build_inovance_serial_write_frame(
        series: InovanceSeries,
        station: u8,
        address: &str,
        words: &[u16],
        use_ascii_mode: bool,
    ) -> Result<(u16, Vec<u8>), DriverError> {
        let (is_bit, modbus_addr) = translate_inovance_address(series, address)?;
        let frame = if words.len() <= 1 {
            let fc = if is_bit { 0x05 } else { 0x06 };
            let body = build_write_single_pdu_body(modbus_addr, *words.first().unwrap_or(&0), is_bit);
            if use_ascii_mode {
                build_ascii_frame(station, fc, &body)
            } else {
                build_rtu_frame(station, fc, &body)
            }
        } else {
            let mut raw = Vec::with_capacity(words.len() * 2);
            for &w in words {
                raw.extend_from_slice(&w.to_be_bytes());
            }
            let body = build_write_multiple_registers_pdu_body(modbus_addr, &raw);
            if use_ascii_mode {
                build_ascii_frame(station, 0x10, &body)
            } else {
                build_rtu_frame(station, 0x10, &body)
            }
        };
        Ok((modbus_addr, frame))
    }
}

pub use inovance::InovanceSeries;

