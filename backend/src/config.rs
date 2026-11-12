use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Address to listen on. Prefer the machine's Tailscale IP, e.g. "100.64.0.1:21114".
    pub bind: String,
    /// SQLite database file. Relative paths are resolved against the config file's directory.
    pub db_path: PathBuf,
    /// Directory for daily-rotated log files. Relative paths as for `db_path`.
    pub log_dir: PathBuf,
    /// Lifetime of a login token in days.
    pub token_ttl_days: i64,
    /// A device counts as online if it sent a heartbeat within this many seconds.
    pub online_timeout_secs: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:21114".to_owned(),
            db_path: PathBuf::from("rdbackend.sqlite3"),
            log_dir: PathBuf::from("logs"),
            token_ttl_days: 30,
            online_timeout_secs: 60,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
        let mut cfg: Config =
            toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
        let base = path
            .canonicalize()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        cfg.db_path = resolve(&base, &cfg.db_path);
        cfg.log_dir = resolve(&base, &cfg.log_dir);
        if cfg.token_ttl_days <= 0 || cfg.online_timeout_secs <= 0 {
            return Err("token_ttl_days and online_timeout_secs must be positive".to_owned());
        }
        Ok(cfg)
    }
}

fn resolve(base: &Path, p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}
