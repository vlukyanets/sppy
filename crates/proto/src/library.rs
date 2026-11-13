use serde::{Deserialize, Serialize};

use crate::ids::FileId;

/// Cheap file identity: size plus `blake3(first 1 MiB ‖ last 1 MiB)` as 64 lowercase hex chars.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub hash: String,
}

/// External subtitle of a video file (next to it or in a `Subs/` folder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subtitle {
    /// Stable identifier (hash of the subtitle's path), not a position: it survives rescans
    /// and other subtitles being added. Used in `/media/{file_id}/sub/{index}`.
    pub index: u32,
    /// Normalized remainder of the file name after the video's name, e.g. `en`, `ru.forced`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub ext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LibraryNode {
    Dir {
        name: String,
        children: Vec<LibraryNode>,
    },
    File {
        file_id: FileId,
        name: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<Fingerprint>,
        subtitles: Vec<Subtitle>,
    },
}
