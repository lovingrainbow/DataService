// crates/mm-drivers/tests/tight_timeout.rs
//! §7.1 緊湊逾時：以真實 Socket 驗證逾時設定會套用到讀取

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

use mm_drivers::{DriverConfig, PlcDriver, UniversalPlcDriver};

#[test]
fn tight_timeout_bounds_a_silent_socket_read() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local addr").port();

    // 模擬 PLC：回應連線交握後便不再回應讀取請求
    let plc = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 512];
        let _ = socket.read(&mut buf);
        let _ = socket.write_all(&[0u8]);
        thread::sleep(Duration::from_millis(1500));
    });

    let cfg = DriverConfig {
        host: "127.0.0.1".into(),
        port,
        loopback_emulation: false,
        ..DriverConfig::default()
    };
    let mut driver = UniversalPlcDriver::new(cfg.clone());
    driver.connect(cfg).expect("connect");

    // 預設 1000ms 逾時下，沉默的 PLC 會讓讀取等待 1 秒；套用 50ms 緊湊逾時後應快速失敗
    driver.set_timeout_ms(50);
    let started = Instant::now();
    let result = driver.read_single("D100", "int16");
    let elapsed = started.elapsed();

    assert!(result.is_err(), "沉默的 PLC 應回傳錯誤");
    assert!(
        elapsed < Duration::from_millis(500),
        "讀取應在緊湊逾時內失敗，實際 {elapsed:?}"
    );
    drop(driver);
    plc.join().expect("plc thread");
}
