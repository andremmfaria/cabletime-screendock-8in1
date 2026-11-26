//! The dock EDID cache: one file in the state directory, written atomically with owner-only
//! permissions.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub use super::types::FileEdidStore;
use super::types::MAX_CACHED_EDID;
use crate::ports::transport::EdidStore;

/// The cache file below the user's state directory (`$XDG_STATE_HOME`, else
/// `~/.local/state`).
pub fn default_path() -> Option<PathBuf> {
    dirs::state_dir().map(|dir| dir.join("screendockd").join("dock-edid.bin"))
}

impl FileEdidStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EdidStore for FileEdidStore {
    fn load(&self) -> Option<Vec<u8>> {
        let meta = fs::metadata(&self.path).ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_CACHED_EDID {
            return None;
        }
        fs::read(&self.path).ok()
    }

    fn save(&self, bytes: &[u8]) -> io::Result<()> {
        let dir = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "cache path has no directory")
        })?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let temp = self.path.with_extension("tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn saved_bytes_load_back_and_the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEdidStore::new(dir.path().join("state/screendockd/dock-edid.bin"));
        assert_eq!(store.load(), None);
        store.save(&[1, 2, 3]).unwrap();
        assert_eq!(store.load(), Some(vec![1, 2, 3]));
        store.save(&[9]).unwrap();
        assert_eq!(store.load(), Some(vec![9]));
        let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn empty_oversized_and_non_file_entries_load_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edid.bin");
        let store = FileEdidStore::new(&path);
        fs::write(&path, []).unwrap();
        assert_eq!(store.load(), None);
        fs::write(&path, vec![0u8; MAX_CACHED_EDID as usize + 1]).unwrap();
        assert_eq!(store.load(), None);
        let as_dir = FileEdidStore::new(dir.path());
        assert_eq!(as_dir.load(), None);
    }
}
