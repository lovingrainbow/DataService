//! # `crates/mm-drivers/tests/plc_driver_tests.rs`
//!
//! Comprehensive unit & integration tests for Industrial Protocol Driver engine.

use machine_model::model::ScalarValue;
use mm_drivers::{
    ByteOrder, ByteTransform, DriverConfig, DriverError, DriverKind,
    PlcDriver, RandomReadItem, RandomWriteItem, SimMode, StressMode,
    StressTestConfig, UniversalPlcDriver, build_protocol_frame,
    build_protocol_read_frame, list_driver_catalog, modbus_crc16,
    parse_universal_plc_address,
    checksum::{crc16_modbus, lrc_byte, melsec_sum_check_ascii, omron_fcs_ascii, verify_crc16_modbus},
    inovance::{build_inovance_serial_read_frame, translate_inovance_address, InovanceSeries},
    melsec_fx::{build_fx_links_frame, build_fx_serial_read_frame, build_fx_serial_write_frame},
    omron::{build_fins_core_command, build_fins_multiple_read_command, build_fins_tcp_handshake_frame, pack_omron_hostlink_serial},
};
use tokio::sync::mpsc;

#[test]
fn test_byte_transform_all_orders() {
    let val_u16 = 0x1234u16;

    // ABCD
    let tf_abcd = ByteTransform::new(ByteOrder::ABCD);
    assert_eq!(tf_abcd.u16_to_bytes(val_u16), [0x12, 0x34]);
    assert_eq!(tf_abcd.bytes_to_u16(&[0x12, 0x34]), 0x1234);

    // DCBA
    let tf_dcba = ByteTransform::new(ByteOrder::DCBA);
    assert_eq!(tf_dcba.u16_to_bytes(val_u16), [0x34, 0x12]);
    assert_eq!(tf_dcba.bytes_to_u16(&[0x34, 0x12]), 0x1234);

    // 32-bit test: 0x12345678
    let val_u32 = 0x12345678u32;
    assert_eq!(tf_abcd.u32_to_bytes(val_u32), [0x12, 0x34, 0x56, 0x78]);
    assert_eq!(tf_abcd.bytes_to_u32(&[0x12, 0x34, 0x56, 0x78]), val_u32);

    let tf_badc = ByteTransform::new(ByteOrder::BADC);
    assert_eq!(tf_badc.u32_to_bytes(val_u32), [0x34, 0x12, 0x78, 0x56]);
    assert_eq!(tf_badc.bytes_to_u32(&[0x34, 0x12, 0x78, 0x56]), val_u32);

    let tf_cdab = ByteTransform::new(ByteOrder::CDAB);
    assert_eq!(tf_cdab.u32_to_bytes(val_u32), [0x56, 0x78, 0x12, 0x34]);
    assert_eq!(tf_cdab.bytes_to_u32(&[0x56, 0x78, 0x12, 0x34]), val_u32);

    assert_eq!(tf_dcba.u32_to_bytes(val_u32), [0x78, 0x56, 0x34, 0x12]);
    assert_eq!(tf_dcba.bytes_to_u32(&[0x78, 0x56, 0x34, 0x12]), val_u32);

    // Float32 test
    let f_val = 123.456f32;
    let f_bytes = tf_abcd.f32_to_bytes(f_val);
    let f_recovered = tf_abcd.bytes_to_f32(&f_bytes);
    assert!((f_recovered - f_val).abs() < 1e-4);
}

#[test]
fn test_modbus_crc16() {
    let test_pdu = [0x01, 0x03, 0x00, 0x64, 0x00, 0x01];
    let crc = modbus_crc16(&test_pdu);
    assert_eq!(crc.len(), 2);
    // Verified Modbus CRC-16 calculation: little endian [low, high]
    let f_modbus_rtu = build_protocol_frame(DriverKind::ModbusRtuOverTcp, 1, "100", 1, false);
    assert_eq!(f_modbus_rtu.len(), 8);
    assert_eq!(&f_modbus_rtu[6..8], &crc);
}

#[test]
fn test_checksum_engines() {
    let data = b"123456789";
    let crc = crc16_modbus(data);
    let mut frame = data.to_vec();
    frame.extend_from_slice(&crc);
    assert!(verify_crc16_modbus(&frame));

    // Modbus ASCII LRC
    let lrc = lrc_byte(&[0x01, 0x03, 0x00, 0x64, 0x00, 0x01]);
    assert_eq!(lrc, 0x97);

    // Mitsubishi FX SumCheck: 48+49+48+48+48+48+50 = 339 = 0x153 & 0xFF = 0x53 -> "53"
    let sum = melsec_sum_check_ascii(b"0100002");
    assert_eq!(&sum, b"53");

    // Omron HostLink FCS (XOR)
    let fcs = omron_fcs_ascii(b"@00FA0000000000101820064000001");
    assert_eq!(fcs.len(), 2);
}

#[test]
fn test_address_parsing() {
    // Siemens S7 DB addresses
    let (prefix, byte_idx, bit_opt, is_bit) = parse_universal_plc_address("DB1.DBW0").unwrap();
    assert_eq!(prefix, "DB1");
    assert_eq!(byte_idx, 0);
    assert_eq!(bit_opt, None);
    assert!(!is_bit);

    let (prefix, byte_idx, bit_opt, is_bit) = parse_universal_plc_address("DB2.DBD4").unwrap();
    assert_eq!(prefix, "DB2");
    assert_eq!(byte_idx, 4);
    assert_eq!(bit_opt, None);
    assert!(!is_bit);

    let (prefix, byte_idx, bit_opt, is_bit) = parse_universal_plc_address("DB1.DBX0.3").unwrap();
    assert_eq!(prefix, "DB1");
    assert_eq!(byte_idx, 0);
    assert_eq!(bit_opt, Some(3));
    assert!(is_bit);

    // Mitsubishi & general addresses
    let (prefix, idx, _, is_bit) = parse_universal_plc_address("D100").unwrap();
    assert_eq!(prefix, "D");
    assert_eq!(idx, 100);
    assert!(!is_bit);

    let (prefix, idx, _, is_bit) = parse_universal_plc_address("M104").unwrap();
    assert_eq!(prefix, "M");
    assert_eq!(idx, 104);
    assert!(is_bit);

    // Octal addressing for X and Y
    let (prefix, idx, _, is_bit) = parse_universal_plc_address("X010").unwrap();
    assert_eq!(prefix, "X");
    assert_eq!(idx, 8); // 010 in octal is 8 in decimal
    assert!(is_bit);

    // Error on empty address
    assert!(matches!(parse_universal_plc_address(""), Err(DriverError::InvalidAddress(_))));
}

#[test]
fn test_protocol_frame_builders_all_families() {
    // 1. Modbus TCP & RTU
    let f_modbus = build_protocol_read_frame(DriverKind::ModbusTcp, 1, "D100", 2);
    assert_eq!(f_modbus.len(), 12);
    assert_eq!(f_modbus[0..6], [0x00, 0x01, 0x00, 0x00, 0x00, 0x06]);
    assert_eq!(f_modbus[6], 1); // Station
    assert_eq!(f_modbus[7], 0x03); // Read Holding Registers

    // 2. Mitsubishi QnA-3E Binary & ASCII
    let f_mc = build_protocol_read_frame(DriverKind::MelsecMcBinary, 0, "D100", 1);
    assert_eq!(f_mc[0..2], [0x50, 0x00]); // Subheader
    assert_eq!(f_mc[11..13], [0x01, 0x04]); // 0401 Read command

    let f_mc_ascii = build_protocol_read_frame(DriverKind::MelsecMcAscii, 0, "D100", 1);
    let s_mc_ascii = String::from_utf8(f_mc_ascii).unwrap();
    assert!(s_mc_ascii.starts_with("500000FF03FF000018001004010000D*"));

    // 3. Mitsubishi iQ-R 3E Binary (4-byte address)
    let f_iqr = build_protocol_read_frame(DriverKind::MelsecMcRBinary, 0, "D100", 1);
    assert_eq!(f_iqr[0..2], [0x50, 0x00]);
    assert_eq!(f_iqr[11..13], [0x01, 0x04]);

    // 4. Siemens S7 ISO-on-TCP
    let f_s7 = build_protocol_read_frame(DriverKind::SiemensS7_1200, 0, "DB1.DBW0", 1);
    assert_eq!(f_s7[0..4], [0x03, 0x00, 0x00, 0x1F]); // TPKT header

    // 5. Omron FINS
    let f_fins = build_protocol_read_frame(DriverKind::OmronFinsTcp, 1, "D100", 1);
    assert_eq!(&f_fins[0..4], b"FINS");

    // 6. Keyence Nano Upper Link ASCII
    let f_nano = build_protocol_read_frame(DriverKind::KeyenceNanoTcp, 0, "DM100", 2);
    let s_nano = String::from_utf8(f_nano).unwrap();
    assert_eq!(s_nano, "RDS DM100.U 2\r");

    // 7. FATEK STX/ETX
    let f_fatek = build_protocol_read_frame(DriverKind::FatekProgramTcp, 1, "D100", 2);
    assert_eq!(f_fatek.first(), Some(&0x02));
    assert_eq!(f_fatek.last(), Some(&0x03));

    // 8. Panasonic Mewtocol
    let f_pana = build_protocol_read_frame(DriverKind::PanasonicMewtocolTcp, 1, "DT100", 2);
    let s_pana = String::from_utf8(f_pana).unwrap();
    assert!(s_pana.starts_with("%01#RDD"));

    // 9. LSis FastEnet
    let f_lsis = build_protocol_read_frame(DriverKind::LSisFastEnet, 0, "DW100", 1);
    assert!(f_lsis.starts_with(b"LSIS-XGT"));

    // 10. Beckhoff ADS (6B AMS/TCP + 32B AMS + 8B Read = 46B)
    let f_ads = build_protocol_read_frame(DriverKind::BeckhoffAds, 0, "D100", 1);
    assert_eq!(f_ads.len(), 46);

    // 11. DL/T 645-2007 電表
    let f_dlt = build_protocol_read_frame(DriverKind::InstDlt645Tcp, 1, "02-01-01-00", 1);
    assert_eq!(&f_dlt[0..4], &[0xFE, 0xFE, 0xFE, 0xFE]);
    assert_eq!(f_dlt[4], 0x68);
    assert_eq!(f_dlt.last(), Some(&0x16));

    // 12. OpenProtocol
    let f_op = build_protocol_read_frame(DriverKind::OpenProtocolTcp, 1, "MID0001", 1);
    assert!(f_op.starts_with(b"00200001001"));
}

#[test]
fn test_serial_and_protocols_melsec_fx() {
    // FX Serial read frame
    let f_read = build_fx_serial_read_frame("D100", 2, false).unwrap();
    assert_eq!(f_read[0], 0x02); // STX
    assert_eq!(f_read[1], b'0'); // Command '0'

    // FX Serial bit write frame (Force ON)
    let f_bit_write = build_fx_serial_write_frame("M100", true, true, &[], false).unwrap();
    assert_eq!(f_bit_write[0], 0x02);
    assert_eq!(f_bit_write[1], b'7'); // Force ON

    // FX Links frame
    let f_links = build_fx_links_frame(0, "D100", 1, None, true).unwrap();
    assert_eq!(f_links[0], 0x05); // ENQ
    let s_links = String::from_utf8(f_links).unwrap();
    assert!(s_links.contains("FFWR0D010001"));
}

#[test]
fn test_serial_and_protocols_inovance() {
    // Easy / H5U address translation
    let (is_bit, addr) = translate_inovance_address(InovanceSeries::Easy, "X010").unwrap();
    assert!(is_bit);
    assert_eq!(addr, 0xF800 + 8); // Octal 10 = Decimal 8

    let (is_bit, addr) = translate_inovance_address(InovanceSeries::H3U, "SD0").unwrap();
    assert!(!is_bit);
    assert_eq!(addr, 0x2400);

    let (is_bit, addr) = translate_inovance_address(InovanceSeries::AM, "MW100").unwrap();
    assert!(!is_bit);
    assert_eq!(addr, 100);

    // Frame generation
    let (modbus_addr, frame) = build_inovance_serial_read_frame(InovanceSeries::H5U, 1, "D100", 2, false).unwrap();
    assert_eq!(modbus_addr, 100);
    assert_eq!(frame.len(), 8); // RTU PDU (6) + CRC (2)
}

#[test]
fn test_serial_and_protocols_omron() {
    // FINS TCP Handshake
    let handshake = build_fins_tcp_handshake_frame(10);
    assert_eq!(&handshake[0..4], b"FINS");
    assert_eq!(handshake[19], 10);

    // FINS Core Command
    let cmd = build_fins_core_command("D100", 1, false, None).unwrap();
    assert_eq!(cmd[0..2], [0x01, 0x01]); // 01 01 Memory Read

    // FINS 01 04 Multiple Read
    let multi_cmd = build_fins_multiple_read_command(&["D100".into(), "CIO10.0".into()]).unwrap();
    assert_eq!(multi_cmd[0..2], [0x01, 0x04]);

    // HostLink Serial Packaging
    let hostlink = pack_omron_hostlink_serial(0, &cmd);
    assert_eq!(hostlink[0], b'@');
    assert!(hostlink.ends_with(b"*\r"));
}

#[test]
fn test_catalog_listing_56_devices() {
    let list = list_driver_catalog();
    assert!(list.len() >= 56, "Catalog should have at least 56 supported devices");

    // Check categories
    let categories: Vec<&str> = list.iter().map(|d| d.category.as_str()).collect();
    assert!(categories.contains(&"PLC"));
    assert!(categories.contains(&"Modbus"));
    assert!(categories.contains(&"Robot"));
    assert!(categories.contains(&"CNC"));
    assert!(categories.contains(&"Instrument"));
    assert!(categories.contains(&"Special"));

    // Check robot devices
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotFanucNet));
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotEstunTcp));
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotEfortTcp));
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotKukaAvar));
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotYaskawaYrc));
    assert!(list.iter().any(|d| d.kind == DriverKind::RobotAbbWeb));

    // Check CNC & Instruments
    assert!(list.iter().any(|d| d.kind == DriverKind::CncFanuc0i));
    assert!(list.iter().any(|d| d.kind == DriverKind::InstDlt645Tcp));
    assert!(list.iter().any(|d| d.kind == DriverKind::InstRkcTempTcp));
}

#[test]
fn test_driver_read_write_lifecycle() {
    let mut drv = UniversalPlcDriver::new(DriverConfig::default());
    assert!(!drv.is_connected());

    // Connect
    let trace = drv.connect(DriverConfig::default()).unwrap();
    assert!(drv.is_connected());
    assert!(!trace.tx_hex.is_empty());

    // Single Read & Write bool
    let ack_b = drv.write_single("M200", "bool", ScalarValue::Bool(true)).unwrap();
    assert!(ack_b.readback_matched);
    let r_b = drv.read_single("M200", "bool").unwrap();
    assert_eq!(r_b.value, ScalarValue::Bool(true));

    // Single Read & Write int16
    let ack_w = drv.write_single("D200", "int16", ScalarValue::Number(1234.0)).unwrap();
    assert!(ack_w.readback_matched);
    let r_w = drv.read_single("D200", "int16").unwrap();
    assert_eq!(r_w.value, ScalarValue::Number(1234.0));

    // Single Read & Write string
    let ack_s = drv.write_single("D500", "string:16", ScalarValue::Text("RUN_OK".into())).unwrap();
    assert!(ack_s.readback_matched);
    let r_s = drv.read_single("D500", "string:16").unwrap();
    assert_eq!(r_s.value, ScalarValue::Text("RUN_OK".into()));

    // Continuous Read & Write
    let batch = vec![ScalarValue::Number(10.0), ScalarValue::Number(20.0), ScalarValue::Number(30.0)];
    let ack_cont = drv.write_continuous("D300", "int16", &batch).unwrap();
    assert!(ack_cont.readback_matched);
    let r_cont = drv.read_continuous("D300", 3, "int16").unwrap();
    assert_eq!(r_cont.values.len(), 3);
    assert_eq!(r_cont.values, batch);

    // Random Read & Write
    let r_items = vec![
        RandomReadItem { address: "D200".into(), data_type: "int16".into() },
        RandomReadItem { address: "M200".into(), data_type: "bool".into() },
    ];
    let res_random = drv.read_random(&r_items).unwrap();
    assert_eq!(res_random.items.len(), 2);
    assert_eq!(res_random.items[0].value, ScalarValue::Number(1234.0));
    assert_eq!(res_random.items[1].value, ScalarValue::Bool(true));

    let w_items = vec![
        RandomWriteItem { address: "D200".into(), data_type: "int16".into(), value: ScalarValue::Number(999.0) },
        RandomWriteItem { address: "M200".into(), data_type: "bool".into(), value: ScalarValue::Bool(false) },
    ];
    let ack_w_items = drv.write_random(&w_items).unwrap();
    assert_eq!(ack_w_items.len(), 2);

    // Disconnect
    drv.disconnect().unwrap();
    assert!(!drv.is_connected());
    assert!(matches!(drv.read_single("D100", "int16"), Err(DriverError::Disconnected(_))));
}

#[test]
fn test_driver_latency_and_stress() {
    let mut drv = UniversalPlcDriver::new(DriverConfig::default());
    drv.connect(DriverConfig::default()).unwrap();

    // Latency profiling
    let lat = drv.ping_latency("D120", 50).unwrap();
    assert_eq!(lat.rounds, 50);
    assert_eq!(lat.success_count, 50);
    assert!(lat.min_us <= lat.avg_us);
    assert!(lat.avg_us <= lat.max_us);
    assert_eq!(lat.samples_us.len(), 50);

    // Stress testing across modes
    for mode in [
        StressMode::SingleRead,
        StressMode::SingleWrite,
        StressMode::ContinuousRead,
        StressMode::ContinuousWrite,
        StressMode::RandomRead,
        StressMode::MixedReadWrite,
    ] {
        let rep = drv.run_stress_test(&StressTestConfig {
            mode,
            address: "D100".into(),
            length: 16,
            concurrency: 1,
            iterations: 200,
        }).unwrap();

        assert_eq!(rep.total_ops, 200);
        assert_eq!(rep.success_ops, 200);
        assert_eq!(rep.failed_ops, 0);
        assert!(rep.throughput_ops_per_sec > 0.0);
    }
}

#[tokio::test]
async fn test_driver_poll_and_simulation_modes() {
    let mut drv = UniversalPlcDriver::new(DriverConfig::default());
    drv.connect(DriverConfig::default()).unwrap();

    // Poll channel
    let (tx, mut rx) = mpsc::channel(100);
    drv.poll(&tx).unwrap();
    let mut count = 0;
    while let Ok(_update) = rx.try_recv() {
        count += 1;
    }
    assert!(count > 0);

    // Simulation: Uncertain Disconnect
    drv.set_simulation(SimMode::UncertainDisconnect).unwrap();
    assert!(matches!(drv.read_single("D100", "int16"), Err(DriverError::Disconnected(_))));

    // Simulation: Readback Fail
    drv.set_simulation(SimMode::ReadbackFail).unwrap();
    assert!(matches!(
        drv.write_single("D100", "int16", ScalarValue::Number(1.0)),
        Err(DriverError::ReadbackMismatch { .. })
    ));
}

#[tokio::test]
async fn test_real_socket_connection_and_io() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    // 1. 建立本機真實 TCP 模擬 PLC 站點
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let server_handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            // 處理第一次讀取請求 (連線交握 / D100 讀取)
            if let Ok(n) = stream.read(&mut buf) {
                if n > 0 {
                    // 回應 Modbus TCP Response: FC 0x03, 2 bytes data = 0x1234
                    let resp = [0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02, 0x12, 0x34];
                    let _ = stream.write_all(&resp);
                }
            }
            // 處理第二次讀取請求 (Single Read D100)
            if let Ok(n) = stream.read(&mut buf) {
                if n > 0 {
                    let resp = [0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0x01, 0x03, 0x02, 0x56, 0x78];
                    let _ = stream.write_all(&resp);
                }
            }
        }
    });

    // 2. 初始化 DriverConfig 設定為真實硬體連線 (loopback_emulation = false)
    let mut cfg = DriverConfig::default();
    cfg.kind = DriverKind::ModbusTcp;
    cfg.host = "127.0.0.1".into();
    cfg.port = port;
    cfg.loopback_emulation = false; // 關鍵：停用回環模擬，建立真實 Socket！

    let mut drv = UniversalPlcDriver::new(cfg.clone());
    let trace = drv.connect(cfg).unwrap();
    assert!(drv.is_connected());
    assert!(!trace.rx_hex.is_empty());

    // 3. 執行真實現物單點讀取 (讀取來自 TCP 串流的 0x5678 = 22136)
    let read_res = drv.read_single("D100", "uint16").unwrap();
    assert_eq!(read_res.value, ScalarValue::Number(22136.0));
    assert!(read_res.trace.rx_hex.contains("56 78"));

    server_handle.join().unwrap();
}

