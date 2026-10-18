// crates/datasvc-demo/src/main.rs
//! PlcDataService 示範程式
//!
//! - `datasvc-demo`                    規格逐項驗證報告 (DATA_SERVICE_SPEC.md §1 ~ §9)
//! - `datasvc-demo --interactive`      終端互動 HMI
//! - `datasvc-demo --serve --port N`   無頭 Daemon，唯讀 HTTP JSON (預設 port 8080)
//! - `--config <service.toml>`         套用 `[poll]` 速率與 `[station.<id>]` 逾時（互動與 Daemon 模式）

mod daemon;
mod interactive;
mod model;
mod verify;

use std::process::ExitCode;

use plc_data_service::config::ServiceConfig;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("用法: datasvc-demo [--interactive | --serve [--port N]] [--config <service.toml>]");
        return ExitCode::SUCCESS;
    }

    let interactive = args.iter().any(|a| a == "--interactive");
    let serve = args.iter().any(|a| a == "--serve");
    let port = args
        .windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse::<u16>().ok())
        .unwrap_or(8080);

    if interactive && serve {
        eprintln!("--interactive 與 --serve 不可同時使用");
        return ExitCode::from(2);
    }

    let config = match load_config(&args) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    // 日誌輸出到 stderr，避免與報告或互動輸出混在一起；報告與互動預設只顯示警告以上
    let default_filter = if serve {
        "info"
    } else if interactive {
        "warn"
    } else {
        "error"
    };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)))
        .with_writer(std::io::stderr)
        .init();

    if interactive {
        let svc = match model::build_service(&config) {
            Ok(svc) => svc,
            Err(e) => {
                eprintln!("service.toml 與機台模型不一致: {e}");
                return ExitCode::from(2);
            }
        };
        return match interactive::run(svc) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("互動模式錯誤: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if serve {
        let svc = match model::build_service(&config) {
            Ok(svc) => svc,
            Err(e) => {
                eprintln!("service.toml 與機台模型不一致: {e}");
                return ExitCode::from(2);
            }
        };
        return match daemon::run(svc, port).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Daemon 錯誤: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if verify::run_report() == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 讀取 `--config <path>` 指定的 `service.toml`；未指定時使用預設設定
fn load_config(args: &[String]) -> Result<ServiceConfig, String> {
    let Some(pos) = args.iter().position(|a| a == "--config") else {
        return Ok(ServiceConfig::default());
    };
    let path = args.get(pos + 1).ok_or("--config 需要指定 service.toml 路徑")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("無法讀取 {path}: {e}"))?;
    ServiceConfig::from_toml_str(&text)
}
