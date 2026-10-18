// crates/datasvc-demo/src/main.rs
//! PlcDataService 示範程式
//!
//! - `datasvc-demo`                    規格逐項驗證報告 (DATA_SERVICE_SPEC.md §1 ~ §9)
//! - `datasvc-demo --interactive`      終端互動 HMI
//! - `datasvc-demo --serve --port N`   無頭 Daemon，唯讀 HTTP JSON (預設 port 8080)

mod daemon;
mod interactive;
mod model;
mod verify;

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("用法: datasvc-demo [--interactive | --serve [--port N]]");
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
        return match interactive::run(model::build_service()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("互動模式錯誤: {e}");
                ExitCode::FAILURE
            }
        };
    }

    if serve {
        return match daemon::run(model::build_service(), port).await {
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
