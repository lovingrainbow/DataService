// crates/datasvc-demo/src/daemon.rs
//! 無頭 Daemon：以唯讀 HTTP JSON 提供服務狀態，背景每秒推進訂閱維護 (§4.2)

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use plc_data_service::{PlcDataService, now_ms};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, warn};

type Shared = Arc<Mutex<PlcDataService>>;

pub async fn run(svc: PlcDataService, port: u16) -> std::io::Result<()> {
    let shared: Shared = Arc::new(Mutex::new(svc));

    let maintenance = Arc::clone(&shared);
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            maintenance.lock().subscriptions.tick_maintenance(now_ms());
        }
    });

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    info!(port, "daemon listening on 127.0.0.1 (GET /tags /stations /subscriptions /blocks /audit)");

    loop {
        let (mut socket, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!(error = %e, "accept failed");
                continue;
            }
        };
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let n = match socket.read(&mut buf).await {
                Ok(n) if n > 0 => n,
                _ => return,
            };
            let request = String::from_utf8_lossy(&buf[..n]);
            let reply = route(&shared, &request);
            if let Err(e) = socket.write_all(reply.as_bytes()).await {
                warn!(%peer, error = %e, "failed to write response");
            }
            let _ = socket.shutdown().await;
        });
    }
}

fn route(shared: &Shared, request: &str) -> String {
    let mut parts = request.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    if method != "GET" {
        return response(405, "Method Not Allowed", &json!({ "error": "唯讀介面，只支援 GET" }));
    }

    let svc = shared.lock();
    let body = match path {
        "/tags" => to_value(&svc.store.get_snapshots(&[])),
        "/stations" => to_value(&svc.breakers),
        "/subscriptions" => to_value(&svc.subscriptions.stats(now_ms())),
        "/blocks" => to_value(&svc.compute_active_coalesced_blocks()),
        "/audit" => to_value(&svc.audit_trail),
        _ => return response(404, "Not Found", &json!({ "error": "未知端點", "path": path })),
    };
    response(200, "OK", &body)
}

fn to_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn response(status: u16, reason: &str, body: &Value) -> String {
    let text = serde_json::to_string_pretty(body).unwrap_or_default();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )
}
