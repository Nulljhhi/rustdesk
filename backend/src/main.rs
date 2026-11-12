use rdbackend::Config;
use std::path::PathBuf;

#[cfg(windows)]
mod service;

const USAGE: &str = "Usage: rdbackend --config <path-to-config.toml> [--service]

  --config <path>  TOML configuration file (required)
  --service        run under the Windows Service Control Manager (Windows only)";

struct Args {
    config: PathBuf,
    service: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut config = None;
    let mut service = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" | "-c" => config = Some(it.next().ok_or("--config needs a path")?),
            "--service" => service = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--version" | "-V" => {
                println!("rdbackend {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let config = config.ok_or("--config is required")?;
    Ok(Args {
        config: PathBuf::from(config),
        service,
    })
}

/// Logs to a daily-rotated file in `log_dir` and, unless running as a service, to stdout.
/// The returned guard flushes the file writer when dropped.
pub fn init_logging(
    cfg: &Config,
    console: bool,
) -> Result<tracing_appender::non_blocking::WorkerGuard, String> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    std::fs::create_dir_all(&cfg.log_dir)
        .map_err(|e| format!("cannot create log dir {}: {e}", cfg.log_dir.display()))?;
    let (file, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(
        &cfg.log_dir,
        "rdbackend.log",
    ));
    let filter =
        EnvFilter::try_from_env("RDBACKEND_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file).with_ansi(false))
        .with(console.then(fmt::layer))
        .try_init()
        .map_err(|e| e.to_string())?;
    Ok(guard)
}

pub fn run_blocking(
    cfg: Config,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(rdbackend::run(cfg, shutdown))
}

async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!("cannot listen for Ctrl-C: {e}");
        std::future::pending::<()>().await;
    }
    tracing::info!("shutting down");
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    let cfg = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let result = if args.service {
        run_service(cfg)
    } else {
        init_logging(&cfg, true).and_then(|_guard| run_blocking(cfg, ctrl_c()))
    };
    if let Err(e) = result {
        tracing::error!("{e}");
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run_service(cfg: Config) -> Result<(), String> {
    service::run(cfg)
}

#[cfg(not(windows))]
fn run_service(_cfg: Config) -> Result<(), String> {
    Err("--service is only supported on Windows".to_owned())
}
