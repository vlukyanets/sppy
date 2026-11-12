//! Host and invite tokens, stored in `<data_dir>/tokens.toml` (spec §4.2).

use std::fs;
use std::io::Write;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

use crate::error::ServerError;

const FILE_NAME: &str = "tokens.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub host: String,
    pub invite: String,
}

/// 32 random bytes, base64url without padding (43 chars).
pub fn generate_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Load tokens, creating them on first start. Returns `(tokens, created)`.
pub fn load_or_create(data_dir: &Path) -> Result<(Tokens, bool), ServerError> {
    let path = data_dir.join(FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let tokens = toml::from_str(&text)
                .map_err(|e| ServerError::State(format!("{}: {e}", path.display())))?;
            Ok((tokens, false))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let tokens = Tokens { host: generate_token(), invite: generate_token() };
            save(data_dir, &tokens)?;
            Ok((tokens, true))
        }
        Err(e) => Err(e.into()),
    }
}

pub fn rotate_host(data_dir: &Path) -> Result<Tokens, ServerError> {
    let (mut tokens, _) = load_or_create(data_dir)?;
    tokens.host = generate_token();
    save(data_dir, &tokens)?;
    Ok(tokens)
}

pub fn rotate_invite(data_dir: &Path) -> Result<Tokens, ServerError> {
    let (mut tokens, _) = load_or_create(data_dir)?;
    tokens.invite = generate_token();
    save(data_dir, &tokens)?;
    Ok(tokens)
}

/// Constant-time comparison of two tokens.
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn save(data_dir: &Path, tokens: &Tokens) -> Result<(), ServerError> {
    fs::create_dir_all(data_dir)?;
    let text = toml::to_string(tokens).expect("tokens always serialize");
    write_private_atomic(&data_dir.join(FILE_NAME), text.as_bytes())?;
    Ok(())
}

/// Write via a temp file + rename; on Unix the file is created with mode 0600.
pub(crate) fn write_private_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_43_chars_url_safe() {
        let t = generate_token();
        assert_eq!(t.len(), 43);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(t, generate_token());
    }

    #[test]
    fn load_or_create_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (first, created) = load_or_create(dir.path()).unwrap();
        assert!(created);
        let (second, created) = load_or_create(dir.path()).unwrap();
        assert!(!created);
        assert_eq!(first, second);
        assert_ne!(first.host, first.invite);
    }

    #[test]
    fn rotate_invite_keeps_host() {
        let dir = tempfile::tempdir().unwrap();
        let (before, _) = load_or_create(dir.path()).unwrap();
        let after = rotate_invite(dir.path()).unwrap();
        assert_eq!(after.host, before.host);
        assert_ne!(after.invite, before.invite);
        assert_eq!(load_or_create(dir.path()).unwrap().0, after);
    }

    #[test]
    fn rotate_host_keeps_invite() {
        let dir = tempfile::tempdir().unwrap();
        let (before, _) = load_or_create(dir.path()).unwrap();
        let after = rotate_host(dir.path()).unwrap();
        assert_ne!(after.host, before.host);
        assert_eq!(after.invite, before.invite);
    }

    #[test]
    fn token_eq_compares() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }

    #[test]
    fn corrupt_tokens_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tokens.toml"), "host = 1").unwrap();
        assert!(matches!(load_or_create(dir.path()), Err(ServerError::State(_))));
    }

    #[cfg(unix)]
    #[test]
    fn tokens_file_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        load_or_create(dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join("tokens.toml")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
