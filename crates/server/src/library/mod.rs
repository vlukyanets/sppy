//! Media library: scan of configured folders, subtitles, file ids, fingerprints (spec §4.6).

mod cache;
mod scan;

pub use cache::FingerprintCache;
pub use scan::{Library, LibraryFile, file_id};
