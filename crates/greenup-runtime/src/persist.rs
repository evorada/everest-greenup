//! What survives a restart: the energy counter and an open transaction.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted per-side state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedState {
    /// Imported energy, watt-hours, never decreasing.
    pub energy_wh: f64,
    /// Session uuid of a transaction that was running at the last write.
    pub open_transaction: Option<String>,
}

impl PersistedState {
    /// Load from `path`; a missing file is the default state.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Write atomically (temporary file, then rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, path)
    }
}
