use tracing::info;
use tracing_subscriber::EnvFilter;

pub mod datasvc_commands;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    info!("ArgonGUI Industrial PLC DataService Tauri Backend Initialized.");
}
