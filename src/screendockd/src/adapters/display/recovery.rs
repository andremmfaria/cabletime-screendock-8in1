//! The recovery file: written before the first change to the X server and removed when the
//! change is undone, so a killed daemon's monitor can be removed later.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::types::{
    RandrBackend, RecoveryOutcome, RecoveryState, RecoveryStore, XRandr, RECOVERY_DIR,
    RECOVERY_FILE, RECOVERY_MAX_BYTES,
};
use crate::ports::display::{OutputId, VdisplayError};

fn recovery_err(e: impl std::fmt::Display) -> VdisplayError {
    VdisplayError::Recovery(e.to_string())
}

impl RecoveryStore {
    /// A store at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default location under `$XDG_RUNTIME_DIR`, when that is set.
    pub fn default_location() -> Option<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
        Some(Self::new(
            PathBuf::from(runtime)
                .join(RECOVERY_DIR)
                .join(RECOVERY_FILE),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes `state`, creating the directory with mode 0700 and the file with mode 0600.
    pub fn save(&self, state: &RecoveryState) -> Result<(), VdisplayError> {
        if let Some(dir) = self.path.parent() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(recovery_err)?;
        }
        let json = serde_json::to_vec(state).map_err(recovery_err)?;
        let temp = self.path.with_extension("tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)
            .map_err(recovery_err)?;
        file.write_all(&json).map_err(recovery_err)?;
        file.sync_all().map_err(recovery_err)?;
        std::fs::rename(&temp, &self.path).map_err(recovery_err)
    }

    /// The stored state; `None` when no file exists. A file that is too large or does not
    /// parse is an error.
    pub fn load(&self) -> Result<Option<RecoveryState>, VdisplayError> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(recovery_err(e)),
        };
        if meta.len() > RECOVERY_MAX_BYTES {
            return Err(recovery_err("the recovery file is too large"));
        }
        let bytes = std::fs::read(&self.path).map_err(recovery_err)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(recovery_err)
    }

    /// Removes the file; a missing file is fine.
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Undoes what `state` records on `backend`: the bare monitor, the output and its mode, and
/// the screen size. Every step runs even when an earlier one fails; the failures come back.
pub(super) fn undo<B: RandrBackend>(backend: &mut B, state: &RecoveryState) -> Vec<VdisplayError> {
    let mut errors = Vec::new();
    let mut note = |result: Result<(), VdisplayError>| {
        if let Err(e) = result {
            errors.push(e);
        }
    };
    if let Some(name) = &state.monitor {
        note(backend.delete_monitor(name));
    }
    if let Some(output) = state.output {
        note(backend.hide_output(OutputId(output)));
        if let Some(mode) = &state.mode {
            note(backend.drop_mode(OutputId(output), mode));
        }
    }
    match backend.screen_size() {
        Ok(now) if now != state.screen => note(backend.set_screen_size(state.screen)),
        Ok(_) => {}
        Err(e) => note(Err(e)),
    }
    errors
}

fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// Undoes the change the recovery file records, when its daemon is gone. Used by
/// `vdisplay cleanup` and at daemon start.
pub fn recover(store: &RecoveryStore) -> Result<RecoveryOutcome, VdisplayError> {
    let Some(state) = store.load()? else {
        return Ok(RecoveryOutcome::NothingToDo);
    };
    if state.pid != std::process::id() && process_alive(state.pid) {
        return Ok(RecoveryOutcome::StillRunning(state.pid));
    }
    match XRandr::connect(state.display.as_deref()) {
        Ok(mut backend) => {
            let errors = undo(&mut backend, &state);
            if let Some(first) = errors.into_iter().next() {
                return Err(first);
            }
        }
        // The X server is gone, so there is nothing left to restore.
        Err(VdisplayError::Connection(reason)) => {
            tracing::info!(%reason, "the X server is gone; dropping the recovery file");
        }
        Err(e) => return Err(e),
    }
    store.remove();
    Ok(RecoveryOutcome::Restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::display::types::ScreenSize;

    fn state() -> RecoveryState {
        RecoveryState {
            display: Some(":101".into()),
            pid: 4_000_000_000,
            screen: ScreenSize {
                width: 1280,
                height: 720,
                width_mm: 339,
                height_mm: 190,
            },
            monitor: Some("ScreenDock".into()),
            output: None,
            mode: None,
        }
    }

    #[test]
    fn state_round_trips_and_the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path().join("run/vdisplay-state.json"));
        assert_eq!(store.load().unwrap(), None);
        store.save(&state()).unwrap();
        assert_eq!(store.load().unwrap(), Some(state()));
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.path()), 0o600);
        assert_eq!(mode(store.path().parent().unwrap()), 0o700);
        store.remove();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn a_corrupt_or_oversized_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let store = RecoveryStore::new(&path);
        std::fs::write(&path, b"{not json").unwrap();
        assert!(store.load().is_err());
        std::fs::write(&path, vec![b' '; 70_000]).unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn a_running_daemon_keeps_its_recovery_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path().join("state.json"));
        let mut live = state();
        let other = std::os::unix::process::parent_id();
        live.pid = other;
        store.save(&live).unwrap();
        assert_eq!(recover(&store), Ok(RecoveryOutcome::StillRunning(other)));
        assert!(store.path().exists());
    }

    #[test]
    fn no_file_means_nothing_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(dir.path().join("state.json"));
        assert_eq!(recover(&store), Ok(RecoveryOutcome::NothingToDo));
    }
}
