//! The API key, in its own file in the settings folder: `ai-key` beside the settings file, readable by its owner
//! only (mode 0600 on Unix). It is not a preference, so `prefs.get`, exported settings and agents never see it.
//! The browser build keeps no key.

use std::path::{Path, PathBuf};

use crate::AiError;

/// The key file's name in the settings folder.
pub const FILE_NAME: &str = "ai-key";

/// Longest key accepted, in bytes.
pub const MAX_KEY: usize = 1024;

/// The key file in `dir`.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// `key` trimmed, or an error when it could not be a key (empty, too long, spaces or control characters inside).
pub fn check(key: &str) -> Result<&str, AiError> {
    let k = key.trim();
    if k.is_empty() {
        return Err(AiError::Invalid("The API key is empty.".into()));
    }
    if k.len() > MAX_KEY || k.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AiError::Invalid("That does not look like an API key (no spaces, at most 1024 characters).".into()));
    }
    Ok(k)
}

/// The key stored in `dir`, or `None` when there is none.
#[cfg(not(target_arch = "wasm32"))]
pub fn load(dir: &Path) -> Result<Option<String>, AiError> {
    let path = path_in(dir);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AiError::Storage(e.to_string())),
    };
    // A key file is a line; anything large is not ours.
    if meta.len() > (MAX_KEY as u64) * 2 {
        return Err(AiError::Storage("the key file is too large".into()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| AiError::Storage(e.to_string()))?;
    Ok(check(&text).ok().map(str::to_string))
}

/// Stores `key` in `dir`, readable by its owner only. The file is written beside its final name and renamed, so a
/// failed write leaves the old key.
#[cfg(not(target_arch = "wasm32"))]
pub fn save(dir: &Path, key: &str) -> Result<(), AiError> {
    let key = check(key)?;
    std::fs::create_dir_all(dir).map_err(|e| AiError::Storage(e.to_string()))?;
    let tmp = dir.join(format!("{FILE_NAME}.tmp"));
    write_private(&tmp, key).map_err(|e| AiError::Storage(e.to_string()))?;
    std::fs::rename(&tmp, path_in(dir)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AiError::Storage(e.to_string())
    })
}

/// Removes the stored key; no key is not an error.
#[cfg(not(target_arch = "wasm32"))]
pub fn clear(dir: &Path) -> Result<(), AiError> {
    match std::fs::remove_file(path_in(dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AiError::Storage(e.to_string())),
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

#[cfg(all(not(unix), not(target_arch = "wasm32")))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    // Windows: the settings folder is in the user's profile, which only the user can read.
    std::fs::write(path, text)
}

/// The key as shown in a field: its first and last four characters, the rest hidden.
pub fn masked(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 10 {
        return "•".repeat(chars.len().max(8));
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}••••••••{tail}")
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn save_load_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        save(dir.path(), "  sk-or-v1-abc123  ").unwrap();
        assert_eq!(load(dir.path()).unwrap().as_deref(), Some("sk-or-v1-abc123"));
        clear(dir.path()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        clear(dir.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn key_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "sk-test").unwrap();
        let mode = std::fs::metadata(path_in(dir.path())).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn bad_keys_are_refused_and_a_huge_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save(dir.path(), "").is_err());
        assert!(save(dir.path(), "has space").is_err());
        assert!(save(dir.path(), &"k".repeat(MAX_KEY + 1)).is_err());
        std::fs::write(path_in(dir.path()), "k".repeat(MAX_KEY * 3)).unwrap();
        assert!(load(dir.path()).is_err());
    }

    #[test]
    fn masking_hides_the_middle() {
        assert_eq!(masked("sk-or-v1-0123456789"), "sk-o••••••••6789");
        assert_eq!(masked("short"), "••••••••");
        assert_eq!(masked(""), "••••••••");
    }
}
