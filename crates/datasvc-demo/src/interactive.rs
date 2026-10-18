// crates/datasvc-demo/src/interactive.rs
//! 終端互動 HMI：以文字指令操作 PlcDataService（讀寫、訂閱、模擬站點故障與驅動模式）

use std::io::{self, BufRead, Write};

use machine_model::model::ScalarValue;
use mm_drivers::SimMode;
use plc_data_service::{PlcDataService, now_ms};

use crate::model::STATION_ID;

const OPERATOR_ID: &str = "TERMINAL_HMI";
const CLIENT_IP: &str = "127.0.0.1";
const CLIENT_ID: &str = "terminal";

const HELP: &str = "\
指令:
  list                               列出所有點位快照
  get <TAG>                          讀取單一點位
  write <TAG> <VALUE>                寫入點位 (VALUE 為數字或 true/false)
  enter <VIEW> <TAG>...              進入視角並訂閱點位
  leave <VIEW>                       離開視角 (啟動 3 秒退訂冷卻)
  blocks                             顯示目前的拼包區塊
  stats                              顯示雙軌訂閱統計 (JSON)
  station <ID> ok|fail               模擬站點通訊結果
  sim good|uncertain|readback-fail   設定驅動模擬模式
  audit                              顯示審計日誌 (新到舊)
  help                               顯示本說明
  quit                               離開";

pub fn run(mut svc: PlcDataService) -> io::Result<()> {
    println!("{HELP}");
    let mut input = io::stdin().lock();
    let mut line = String::new();
    loop {
        print!("> ");
        io::stdout().flush()?;
        line.clear();
        if input.read_line(&mut line)? == 0 {
            break;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some((&cmd, args)) = parts.split_first() else {
            continue;
        };
        match cmd {
            "quit" | "exit" => break,
            "help" => println!("{HELP}"),
            "list" => list(&svc),
            "get" => get(&svc, args),
            "write" => write(&mut svc, args),
            "enter" => enter(&mut svc, args),
            "leave" => leave(&mut svc, args),
            "blocks" => blocks(&svc),
            "stats" => stats(&svc),
            "station" => station(&mut svc, args),
            "sim" => sim(&mut svc, args),
            "audit" => audit(&svc),
            other => println!("未知指令 '{other}'，輸入 help 查看說明"),
        }
    }
    Ok(())
}

fn list(svc: &PlcDataService) {
    for s in svc.store.get_snapshots(&[]).values() {
        println!("  {:<36} {:?} | {:?} | v{}", s.tag_id, s.value, s.quality, s.version);
    }
}

fn get(svc: &PlcDataService, args: &[&str]) {
    let Some(tag) = args.first() else {
        println!("用法: get <TAG>");
        return;
    };
    match svc.store.get_snapshots(&[tag.to_string()]).get(*tag) {
        Some(s) => println!("  {:?} | {:?} | v{}", s.value, s.quality, s.version),
        None => println!("  找不到點位 {tag}"),
    }
}

fn write(svc: &mut PlcDataService, args: &[&str]) {
    let (Some(tag), Some(raw)) = (args.first(), args.get(1)) else {
        println!("用法: write <TAG> <VALUE>");
        return;
    };
    let Some(value) = parse_value(raw) else {
        println!("  值必須為 true/false 或數字: {raw}");
        return;
    };
    match svc.write_tag(tag, value, OPERATOR_ID, CLIENT_IP) {
        Ok(res) => println!("  OK  {} = {:?} ({} µs)", res.tag_id, res.written_value, res.elapsed_us),
        Err(e) => println!("  拒絕 {e}"),
    }
}

fn parse_value(raw: &str) -> Option<ScalarValue> {
    if raw.eq_ignore_ascii_case("true") {
        Some(ScalarValue::Bool(true))
    } else if raw.eq_ignore_ascii_case("false") {
        Some(ScalarValue::Bool(false))
    } else {
        raw.parse::<f64>().ok().map(ScalarValue::Number)
    }
}

fn enter(svc: &mut PlcDataService, args: &[&str]) {
    let Some((view, tags)) = args.split_first() else {
        println!("用法: enter <VIEW> <TAG>...");
        return;
    };
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    let count = tags.len();
    let was_cooling = svc.subscriptions.enter_viewport(CLIENT_ID, *view, tags, now_ms());
    let note = if was_cooling { "（取消冷卻中的退訂）" } else { "" };
    println!("  視角 {view} 已訂閱 {count} 個點位{note}");
}

fn leave(svc: &mut PlcDataService, args: &[&str]) {
    let Some(view) = args.first() else {
        println!("用法: leave <VIEW>");
        return;
    };
    svc.subscriptions.leave_viewport(CLIENT_ID, view, now_ms());
    println!("  視角 {view} 進入 3 秒退訂冷卻");
}

fn blocks(svc: &PlcDataService) {
    let blocks = svc.compute_active_coalesced_blocks();
    if blocks.is_empty() {
        println!("  （沒有活躍點位）");
    }
    for b in blocks {
        println!(
            "  {} ({} words，間隙補讀 {}) 成員 {:?}",
            b.start_address, b.total_words, b.gap_words_bridged, b.member_tags
        );
    }
}

fn stats(svc: &PlcDataService) {
    match serde_json::to_string_pretty(&svc.subscriptions.stats(now_ms())) {
        Ok(json) => println!("{json}"),
        Err(e) => println!("  序列化失敗: {e}"),
    }
}

fn station(svc: &mut PlcDataService, args: &[&str]) {
    let (Some(id), Some(result)) = (args.first(), args.get(1)) else {
        println!("用法: station <ID> ok|fail");
        return;
    };
    let success = match *result {
        "ok" => true,
        "fail" => false,
        _ => {
            println!("  結果必須為 ok 或 fail");
            return;
        }
    };
    let state = svc.report_station_comm_result(id, success, now_ms());
    println!("  站點 {id} 狀態: {state:?}");
}

fn sim(svc: &mut PlcDataService, args: &[&str]) {
    let Some(mode) = args.first().and_then(|name| parse_sim(name)) else {
        println!("用法: sim good|uncertain|readback-fail");
        return;
    };
    match svc.drivers.get_mut(STATION_ID) {
        Some(driver) => match driver.set_simulation(mode) {
            Ok(()) => println!("  驅動模擬模式已切換"),
            Err(e) => println!("  切換失敗: {e}"),
        },
        None => println!("  找不到站點 {STATION_ID} 的驅動"),
    }
}

fn parse_sim(name: &str) -> Option<SimMode> {
    match name {
        "good" => Some(SimMode::Good),
        "uncertain" => Some(SimMode::UncertainDisconnect),
        "readback-fail" => Some(SimMode::ReadbackFail),
        _ => None,
    }
}

fn audit(svc: &PlcDataService) {
    if svc.audit_trail.is_empty() {
        println!("  （尚無紀錄）");
    }
    for e in &svc.audit_trail {
        println!(
            "  [{}] {} {} {:?} -> {:?} 允許={} {}",
            e.timestamp_ms, e.operator_id, e.tag_id, e.old_value, e.new_value, e.permitted, e.note
        );
    }
}
