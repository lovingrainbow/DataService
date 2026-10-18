// crates/datasvc-demo/src/model.rs
//! 示範用機台模型與服務實例（與 plc-data-service 測試使用的模型相同）

use machine_model::family::PlcFamily;
use machine_model::model::{MachineModelBuilder, UnitInstanceBuilder};
use machine_model::resolver::{ResolvedMachineModel, resolve_machine_model};
use machine_model::unit_type::TypeCatalog;
use mm_drivers::{DriverConfig, PlcDriver, UniversalPlcDriver};
use plc_data_service::PlcDataService;
use plc_data_service::config::ServiceConfig;
use plc_data_service::poller::CoalescingPolicy;

/// 示範站點 ID（點位 ID 的第一段需與之相同）
pub const STATION_ID: &str = "plc1";

pub fn demo_resolved_model() -> ResolvedMachineModel {
    let mf = MachineModelBuilder::new("demo_line")
        .add_plc("plc1", PlcFamily::InovanceEasy)
        .add_simple_tag("plc1.X_Cyl1_Ext_LS", "X010", "bool")
        .add_simple_tag("plc1.Y_Cyl1_Ext", "Y010", "bool")
        .add_simple_tag("plc1.M_Cyl1_ManExt", "M100", "bool")
        .add_simple_tag("plc1.M_Cyl1_ErrExt", "M200", "bool")
        .add_simple_tag("plc1.M_Cyl1_ErrRet", "M201", "bool")
        .add_simple_tag("plc1.D_Cyl1_ExtTime", "D100", "int16")
        .add_simple_tag("plc1.D_Cyl1_ErrTime", "D103", "int16")
        .add_simple_tag("plc1.M_St1_ErrJam", "M210", "bool")
        .add_simple_tag("plc1.M_Mach_ErrEstop", "M220", "bool")
        .add_simple_tag("plc1.D_Mach_ManMode", "D106", "int16")
        .add_unit(
            UnitInstanceBuilder::new("cyl1", "cylinder_double", 1)
                .bind_target("lim_ext", "plc1.X_Cyl1_Ext_LS")
                .bind_null("lim_ret")
                .bind_target("valve_ext", "plc1.Y_Cyl1_Ext")
                .bind_null("valve_ret")
                .bind_target("man_ext", "plc1.M_Cyl1_ManExt")
                .bind_target("par_ext_time", "plc1.D_Cyl1_ExtTime")
                .bind_target("par_err_time", "plc1.D_Cyl1_ErrTime")
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
        .build();
    let catalog = TypeCatalog::with_embedded_standards();
    resolve_machine_model(&mf, &catalog).expect("示範機台模型應可解析")
}

/// 依機台模型與 `service.toml` 設定建構服務，並附掛已連線（loopback 模擬）的驅動
pub fn build_service_with(resolved: &ResolvedMachineModel, config: &ServiceConfig) -> Result<PlcDataService, String> {
    let (mut svc, _rx) = PlcDataService::from_resolved_model(resolved, &[], CoalescingPolicy::rs485_default(), config)?;
    let cfg = DriverConfig {
        id: STATION_ID.into(),
        ..DriverConfig::default()
    };
    let mut driver = UniversalPlcDriver::new(cfg.clone());
    driver.connect(cfg).expect("loopback 模擬連線應成功");
    svc.attach_driver(STATION_ID, Box::new(driver));
    Ok(svc)
}

/// 使用預設設定建構服務（驗證報告使用）
pub fn build_service_from(resolved: &ResolvedMachineModel) -> PlcDataService {
    build_service_with(resolved, &ServiceConfig::default()).expect("預設設定應可套用於示範模型")
}

/// 使用指定的 `service.toml` 設定建構示範服務（互動與 Daemon 模式）
pub fn build_service(config: &ServiceConfig) -> Result<PlcDataService, String> {
    build_service_with(&demo_resolved_model(), config)
}
