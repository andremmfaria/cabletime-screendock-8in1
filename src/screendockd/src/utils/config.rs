//! Config loading: a personal/user-specified project convention (not literature-derived like
//! the Design Patterns in `docs/implementation-plan.md`), binding project-wide going forward.
//! See that doc's Design Patterns section for the canonical statement of this convention.
//!
//! ## Library choice: `serde` + `toml`, not the `config` crate
//!
//! Two real options were evaluated:
//! - **`serde` + `toml`**: simple, idiomatic `Deserialize`-derive loading of a single TOML
//!   file into a plain Rust struct. No source-layering/merging machinery of its own.
//! - **`config` crate**: layered configuration — merges multiple sources (files, env vars,
//!   defaults) with built-in precedence rules and env-var overrides.
//!
//! **Decision: `serde` + `toml`.** This project's resolution order (below) is explicit,
//! hand-written, and already does exactly the kind of override logic the `config` crate would
//! otherwise provide generically (env var > XDG user path > system path > repo-root dev path).
//! Pulling in `config` would mean using it for only the single-file-deserialization half of
//! its job while reimplementing the precedence logic ourselves anyway — redundant dependency
//! weight for a daemon with one config file and one owner. `serde`/`toml` is the simpler,
//! smaller, and more idiomatic choice for "deserialize one TOML file into a struct", which is
//! all this module actually needs.
//!
//! ## Path resolution order (the "repo root vs. installed-system path" wrinkle)
//!
//! A packaged `.deb`/`.rpm` (built by the existing packaging workflow) cannot assume the config
//! lives at a path relative to the compiled binary's `CARGO_MANIFEST_DIR` — that directory only
//! exists in a development checkout, not on a system where the package was installed. Nor can
//! a system package pre-populate every future user's `$XDG_CONFIG_HOME` at install time, since
//! that directory belongs to a specific user account that may not even exist yet when the
//! package is installed. The resolution order below accounts for both constraints explicitly,
//! checked in this order, first match wins:
//!
//! 1. **`SCREENDOCK_CONFIG_PATH` environment variable**, if set — an explicit, unambiguous
//!    override for testing, scripting, or non-standard layouts. Always wins if present.
//! 2. **`$XDG_CONFIG_HOME/screendockd/screendock_cfg.toml`**, defaulting to
//!    `~/.config/screendockd/screendock_cfg.toml` per the XDG Base Directory spec, resolved via
//!    the `dirs` crate rather than hand-rolled `HOME`-env-var string concatenation (`dirs`
//!    handles the XDG spec's defaulting/override rules correctly, including on platforms where
//!    the convention differs). This is the real per-user installed-system path: the existing
//!    `[package.metadata.deb]`/`[package.metadata.generate-rpm]` asset lists in
//!    `src/screendockd/Cargo.toml` ship `deploy/screendockd.service` as a **per-user** systemd
//!    unit (`usr/lib/systemd/user/`), not a system unit — so the natural "installed" config
//!    home is the user's own XDG config dir, not a single shared system file.
//! 3. **`/etc/screendockd/screendock_cfg.toml`** — a system-wide default/template. This exists
//!    specifically because a `.deb`/`.rpm` install happens once, system-wide, at package-install
//!    time, before any particular user account necessarily exists or has ever run the daemon;
//!    the package cannot write into `~/.config` for users it doesn't know about yet. Shipping a
//!    template to `/etc` gives every user a working default until they choose to copy/customize
//!    their own `~/.config/screendockd/screendock_cfg.toml`. This path is wired into the
//!    packaging asset lists in `src/screendockd/Cargo.toml` (both `[package.metadata.deb]` and
//!    `[package.metadata.generate-rpm]`), which now ship `config/screendock_cfg.toml` from the
//!    repo there.
//! 4. **`CARGO_MANIFEST_DIR/../../config/screendock_cfg.toml`** — the repo-root dev-convenience
//!    path, resolving to `<repo root>/config/screendock_cfg.toml` from this crate's location
//!    at `src/screendockd/` (one level below the repo root). Last-resort fallback, only useful
//!    when running straight out of a git checkout with no installed config anywhere.
//!
//! This module is a from-scratch implementation; it has no bearing on and does not reference
//! any decompiled vendor code.

use std::env;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Subdirectory name used under both `$XDG_CONFIG_HOME` and `/etc` for this daemon's config.
const CONFIG_DIR_NAME: &str = "screendockd";

/// Config file name, consistent across every resolved location.
const CONFIG_FILE_NAME: &str = "screendock_cfg.toml";

/// Environment variable that, if set, overrides every other resolution step.
const CONFIG_PATH_ENV_VAR: &str = "SCREENDOCK_CONFIG_PATH";

/// System-wide fallback config path, as shipped by the `.deb`/`.rpm` packaging asset lists.
const SYSTEM_CONFIG_PATH: &str = "/etc/screendockd/screendock_cfg.toml";

/// Display/capture target settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DisplayConfig {
    /// Target capture/output width in pixels.
    pub width: u32,
    /// Target capture/output height in pixels.
    pub height: u32,
    /// `"landscape"` or `"portrait"`.
    pub orientation: String,
}

/// Daemon-level runtime settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DaemonConfig {
    /// Default log level (`"trace"`/`"debug"`/`"info"`/`"warn"`/`"error"`), used by
    /// `utils::logging::init_logging` unless overridden by the `RUST_LOG` environment variable.
    pub log_level: String,
    /// Heartbeat tick interval for the MI_00 control channel, in milliseconds.
    pub heartbeat_interval_ms: u64,
}

/// Device identification override, for testing against a non-standard hidraw setup without
/// recompiling.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceConfig {
    /// Lowercase hex USB vendor ID, overriding `discovery::VENDOR_ID_HEX`.
    pub vendor_id_hex: String,
    /// Lowercase hex USB product ID, overriding `discovery::PRODUCT_ID_HEX`.
    pub product_id_hex: String,
}

/// Top-level daemon configuration, deserialized directly from `screendock_cfg.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Config {
    pub display: DisplayConfig,
    pub daemon: DaemonConfig,
    pub device: DeviceConfig,
}

/// Errors from resolving or loading the config file.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// None of the four resolution steps found an existing file.
    #[error(
        "no config file found: checked ${CONFIG_PATH_ENV_VAR}, $XDG_CONFIG_HOME/{CONFIG_DIR_NAME}/{CONFIG_FILE_NAME}, {SYSTEM_CONFIG_PATH}, and the repo-root dev path"
    )]
    NotFound,
    /// The resolved file existed but couldn't be read.
    #[error("couldn't read config file {path:?}: {source}")]
    Unreadable {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The resolved file's contents weren't valid TOML matching `Config`'s shape.
    #[error("couldn't parse config file {path:?} as TOML: {source}")]
    InvalidToml {
        path: PathBuf,
        source: toml::de::Error,
    },
}

/// Resolves the config file path per the 4-step order documented at the top of this module,
/// without reading or parsing it. Exposed separately from `load()` so callers/tests can inspect
/// which path would be used without requiring a file to actually exist there.
pub fn resolve_config_path() -> Option<PathBuf> {
    // 1. Explicit environment variable override — always wins if set.
    if let Ok(path) = env::var(CONFIG_PATH_ENV_VAR) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    // 2. $XDG_CONFIG_HOME/screendockd/screendock_cfg.toml (default ~/.config/...), via the
    //    `dirs` crate rather than hand-rolled HOME concatenation.
    if let Some(xdg_config_home) = dirs::config_dir() {
        let path = xdg_config_home.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME);
        if path.is_file() {
            return Some(path);
        }
    }

    // 3. System-wide default/template, as shipped by the .deb/.rpm packaging.
    let system_path = PathBuf::from(SYSTEM_CONFIG_PATH);
    if system_path.is_file() {
        return Some(system_path);
    }

    // 4. Repo-root dev-convenience path: CARGO_MANIFEST_DIR/../../config/screendock_cfg.toml.
    //    CARGO_MANIFEST_DIR is only set at compile time (baked in via env!), which is fine
    //    since this fallback only makes sense when running a binary built from this checkout.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dev_path = manifest_dir
        .join("..")
        .join("..")
        .join("config")
        .join(CONFIG_FILE_NAME);
    if dev_path.is_file() {
        return Some(dev_path);
    }

    None
}

/// Loads and parses the config file resolved by `resolve_config_path`.
pub fn load() -> Result<Config, ConfigError> {
    let path = resolve_config_path().ok_or(ConfigError::NotFound)?;
    load_from_path(&path)
}

/// Loads and parses a specific config path directly, bypassing resolution — used by `load()`
/// and directly by tests that want to load a known fixture path.
pub fn load_from_path(path: &std::path::Path) -> Result<Config, ConfigError> {
    let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&contents).map_err(|source| ConfigError::InvalidToml {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads the real repo-root config/screendock_cfg.toml directly (not via resolution, so
    /// this test doesn't depend on environment state) and asserts on its parsed values,
    /// proving the loader and the `Config` struct's shape actually match the shipped file.
    #[test]
    fn loads_repo_root_config_and_matches_expected_values() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root_config = manifest_dir
            .join("..")
            .join("..")
            .join("config")
            .join(CONFIG_FILE_NAME);

        let config = load_from_path(&repo_root_config).unwrap();

        assert_eq!(config.display.width, 1280);
        assert_eq!(config.display.height, 720);
        assert_eq!(config.display.orientation, "landscape");
        assert_eq!(config.daemon.log_level, "info");
        assert_eq!(config.daemon.heartbeat_interval_ms, 1000);
        assert_eq!(config.device.vendor_id_hex, "1de1");
        assert_eq!(config.device.product_id_hex, "f201");
    }

    #[test]
    fn env_var_override_takes_precedence_over_everything_else() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repo_root_config = manifest_dir
            .join("..")
            .join("..")
            .join("config")
            .join(CONFIG_FILE_NAME);

        // Serialized access to the shared process environment variable: Rust runs tests in
        // parallel threads within one process, so this test sets and clears the var around its
        // own assertion to avoid leaking state into other tests.
        env::set_var(CONFIG_PATH_ENV_VAR, &repo_root_config);
        let resolved = resolve_config_path();
        env::remove_var(CONFIG_PATH_ENV_VAR);

        assert_eq!(resolved, Some(repo_root_config));
    }

    #[test]
    fn missing_file_reports_unreadable() {
        let err = load_from_path(std::path::Path::new(
            "/nonexistent/screendockd/screendock_cfg.toml",
        ))
        .unwrap_err();
        match err {
            ConfigError::Unreadable { .. } => {}
            other => panic!("expected Unreadable, got {other:?}"),
        }
    }
}
