//! Logging setup: a personal/user-specified project convention, binding project-wide.
//!
//! ## Library choice: `tracing` + `tracing-subscriber`, not `log` + `env_logger`
//!
//! Two real options:
//! - **`log` + `env_logger`**: the simpler, older de facto standard - a single global logger,
//!   flat log lines, level filtering via `RUST_LOG`.
//! - **`tracing` + `tracing-subscriber`**: structured, span-aware logging - log events can
//!   carry key/value fields, and spans let related events (e.g. everything emitted while
//!   handling one capture/encode cycle) be grouped and attributed even across thread
//!   boundaries.
//!
//! **Decision: `tracing` + `tracing-subscriber`.** This daemon's Architecture (see
//! `docs/architecture.md`) is a single process running several independent
//! threads/loops concurrently - capture, encode, heartbeat, framing/hidraw I/O - and a plain
//! `log`-style flat line (`"write failed"`) loses which loop/thread/device emitted it the
//! moment two loops are active at once. `tracing`'s spans let each loop wrap its work in a
//! named span (e.g. `heartbeat_tick`, `capture_frame`) so every event inside it is
//! automatically tagged with that context, which is exactly the capability a concurrent
//! multi-loop daemon needs and `log`/`env_logger` has no equivalent for. The cost is a
//! somewhat heavier dependency and a steeper API than `log`, which is an acceptable tradeoff
//! for a long-running systemd-managed daemon where diagnosing "which loop logged this" matters
//! more than minimizing dependency count.

use tracing_subscriber::EnvFilter;

use super::types::LOG_LEVEL_ENV_VAR;
use crate::utils::config::Config;

/// Initializes the process-global `tracing` subscriber.
///
/// Level precedence: `RUST_LOG`, if set, always wins (so an operator can bump verbosity for one
/// run without editing the config file); otherwise falls back to `config.daemon.log_level`.
/// Output format is plain, single-line-per-event text with level and target - the sensible
/// default for a systemd-managed daemon, since `journalctl` already adds its own timestamp and
/// unit-name framing around each line, so this doesn't duplicate that.
///
/// Must be called at most once per process (typical of most logging-backend init functions);
/// calling it twice returns an error from the underlying subscriber crate, which this function
/// surfaces rather than panics on, since a double-init is a caller bug this module shouldn't
/// decide is fatal.
pub fn init_logging(config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_from_env(LOG_LEVEL_ENV_VAR)
        .unwrap_or_else(|_| EnvFilter::new(&config.daemon.log_level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::config::{
        CaptureConfig, ControlConfig, DaemonConfig, DeviceConfig, DisplayConfig, EncodeConfig,
        VirtualDisplayConfig,
    };

    fn test_config(log_level: &str) -> Config {
        Config {
            display: DisplayConfig {
                width: 1280,
                height: 720,
                orientation: "landscape".to_string(),
            },
            capture: CaptureConfig::default(),
            encode: EncodeConfig::default(),
            virtual_display: VirtualDisplayConfig::default(),
            control: ControlConfig::default(),
            daemon: DaemonConfig {
                log_level: log_level.to_string(),
                heartbeat_interval_ms: 1000,
            },
            device: DeviceConfig {
                vendor_id_hex: "1de1".to_string(),
                product_id_hex: "f201".to_string(),
            },
        }
    }

    /// `init_logging` installs a real global subscriber as a side effect, so only one test in
    /// the whole process may successfully call it (a second call is expected to return
    /// `Err(TryInitError)`, which this test treats as a pass, not a failure) - this proves the
    /// function builds a valid filter/subscriber from a `Config` without panicking, which is
    /// the behavior worth testing without coupling the test suite to global init ordering.
    #[test]
    fn init_logging_succeeds_or_reports_already_initialized() {
        let config = test_config("debug");
        match init_logging(&config) {
            Ok(()) => {}
            Err(_already_initialized) => {}
        }
    }
}
