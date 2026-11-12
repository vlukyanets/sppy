//! Media file rules shared by the server library and the client's local index.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sppy_proto::Fingerprint;

pub const VIDEO_EXTS: &[&str] =
    &["mkv", "mp4", "avi", "webm", "mov", "m4v", "ts", "wmv", "flv", "mpg", "mpeg", "ogv"];
pub const SUB_EXTS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "idx"];

/// Bytes hashed from each end of the file.
const EDGE: u64 = 1024 * 1024;

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

pub fn is_video(path: &Path) -> bool {
    has_ext(path, VIDEO_EXTS)
}

pub fn is_subtitle(path: &Path) -> bool {
    has_ext(path, SUB_EXTS)
}

/// Size + `blake3(first 1 MiB ‖ last 1 MiB)`; the whole file when it is under 2 MiB.
pub fn fingerprint(path: &Path) -> io::Result<Fingerprint> {
    let mut f = File::open(path)?;
    let size = f.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    if size <= 2 * EDGE {
        io::copy(&mut f, &mut hasher)?;
    } else {
        let mut buf = vec![0u8; EDGE as usize];
        f.read_exact(&mut buf)?;
        hasher.update(&buf);
        f.seek(SeekFrom::Start(size - EDGE))?;
        f.read_exact(&mut buf)?;
        hasher.update(&buf);
    }
    Ok(Fingerprint { size, hash: hasher.finalize().to_hex().to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn extension_checks() {
        assert!(is_video(std::path::Path::new("a/Movie.MKV")));
        assert!(!is_video(std::path::Path::new("a/Movie.srt")));
        assert!(is_subtitle(std::path::Path::new("Movie.en.ass")));
        assert!(!is_video(std::path::Path::new("noext")));
    }

    #[test]
    fn fingerprint_small_file_hashes_whole() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "a.bin", b"hello");
        let fp = fingerprint(&p).unwrap();
        assert_eq!(fp.size, 5);
        assert_eq!(fp.hash, blake3::hash(b"hello").to_hex().to_string());
        assert_eq!(fp.hash.len(), 64);
    }

    #[test]
    fn fingerprint_large_file_uses_head_tail() {
        let dir = tempfile::tempdir().unwrap();
        let mut data: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        let a = fingerprint(&write(dir.path(), "a.bin", &data)).unwrap();
        data[3 * 1024 * 1024 / 2] ^= 0xff; // middle byte
        let b = fingerprint(&write(dir.path(), "b.bin", &data)).unwrap();
        assert_eq!(a, b, "middle bytes are not hashed");
        let last = data.len() - 1;
        data[last] ^= 0xff;
        let c = fingerprint(&write(dir.path(), "c.bin", &data)).unwrap();
        assert_ne!(a, c);
        data.push(0);
        let d = fingerprint(&write(dir.path(), "d.bin", &data)).unwrap();
        assert_ne!(d.size, c.size);
    }
}
