//! Running under the Windows Service Control Manager (`rdbackend --config <path> --service`).

use rdbackend::Config;
use std::{
    ffi::OsString,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::Notify;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

/// For an own-process service the SCM ignores this name, so the service may be
/// registered under any name.
const SERVICE_NAME: &str = "rdbackend";

static CONFIG: OnceLock<Config> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

pub fn run(cfg: Config) -> Result<(), String> {
    CONFIG
        .set(cfg)
        .map_err(|_| "service already started".to_owned())?;
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| format!("cannot connect to the service control manager: {e}"))
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!("service failed: {e}");
    }
}

fn status(state: ServiceState, exit_code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(exit_code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service() -> Result<(), String> {
    let cfg = CONFIG.get().cloned().ok_or("missing configuration")?;
    let _guard = crate::init_logging(&cfg, false)?;
    let stop = Arc::new(Notify::new());
    let stop_handler = stop.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |ev| match ev {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop_handler.notify_one();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| format!("cannot register service control handler: {e}"))?;
    handle
        .set_service_status(status(ServiceState::Running, 0))
        .map_err(|e| e.to_string())?;
    tracing::info!("running as a Windows service");
    let result = crate::run_blocking(cfg, async move {
        stop.notified().await;
        tracing::info!("service stop requested");
    });
    if let Err(e) = &result {
        tracing::error!("{e}");
    }
    let code = if result.is_ok() { 0 } else { 1 };
    handle
        .set_service_status(status(ServiceState::Stopped, code))
        .map_err(|e| e.to_string())?;
    result
}
