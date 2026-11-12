//! `state.json` load/save (spec §4.8).

use std::fs;
use std::path::Path;

use crate::room::PersistedState;
use crate::tokens::write_private_atomic;

const FILE_NAME: &str = "state.json";

/// Load the saved room. A corrupt file is renamed to `state.json.bad` and ignored.
pub fn load_state(data_dir: &Path) -> Option<PersistedState> {
    let path = data_dir.join(FILE_NAME);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(state) => Some(state),
        Err(e) => {
            let bad = data_dir.join("state.json.bad");
            tracing::warn!("corrupt {}: {e}; moved to {}", path.display(), bad.display());
            if let Err(e) = fs::rename(&path, &bad) {
                tracing::warn!("cannot rename corrupt state: {e}");
            }
            None
        }
    }
}

pub fn save_state(data_dir: &Path, state: &PersistedState) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let json = serde_json::to_vec_pretty(state).expect("state always serializes");
    write_private_atomic(&data_dir.join(FILE_NAME), &json)
}
