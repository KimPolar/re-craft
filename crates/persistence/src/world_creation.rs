//! Read-only proposal of a new world's folder name.
//!
//! The proposal is not a reservation. A writer must create the directory
//! atomically and retry if another process claims the name in the meantime.
use std::{fs, io::ErrorKind, path::Path};

use anyhow::{Context, Result, ensure};

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\n' | '\r' | '\t' | '\0' | '\u{c}' | '`' | '?' | '*' | '\\' | '<' | '>'
            | '|' | '"' | ':' | '.' => '_',
            other => other,
        })
        .collect()
}

fn portable(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper.ends_with('.') {
        return false;
    }
    let stem = upper.split('.').next().unwrap_or("");
    if matches!(stem, "COM" | "CLOCK$" | "CON" | "PRN" | "AUX" | "NUL") {
        return false;
    }
    for prefix in ["COM", "LPT"] {
        if let Some(suffix) = stem.strip_prefix(prefix)
            && suffix.len() == 1
            && matches!(suffix.as_bytes()[0], b'1'..=b'9')
        {
            return false;
        }
    }
    true
}

fn truncate_utf16(value: &str, limit: usize) -> Result<&str> {
    let mut length = 0;
    for (index, ch) in value.char_indices() {
        if length == limit {
            return Ok(&value[..index]);
        }
        let next = length + ch.len_utf16();
        ensure!(
            next <= limit,
            "folder truncation splits a UTF-16 surrogate pair"
        );
        length = next;
    }
    Ok(value)
}

fn candidate_for(name: &str, mut exists: impl FnMut(&str) -> Result<bool>) -> Result<String> {
    let mut sanitized = sanitize(name);
    if !portable(&sanitized) {
        sanitized = format!("_{sanitized}_");
    }
    // 26.3's installed FileUtil preserves an existing parenthesized suffix
    // as part of the requested name when picking an available folder.
    let base = truncate_utf16(&sanitized, 255)?;
    let mut count: i32 = 0;
    loop {
        let suffix = if count == 0 {
            String::new()
        } else {
            format!(" ({count})")
        };
        let candidate = format!("{}{suffix}", truncate_utf16(base, 255 - suffix.len())?);
        // Windows rejects control characters outside Java's replacement list.
        ensure!(
            !candidate.is_empty() && !candidate.chars().any(|c| c < ' '),
            "invalid Windows folder name"
        );
        if !exists(&candidate)? {
            return Ok(candidate);
        }
        count = count.wrapping_add(1);
    }
}

/// Compute the name Java's create-world screen proposes, without creating or
/// deleting files. The caller supplies the localized default world name.
pub fn propose_world_folder(saves: &Path, name: &str, default_name: &str) -> Result<String> {
    match fs::metadata(saves) {
        Ok(metadata) => ensure!(metadata.is_dir(), "saves path is not a directory"),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect saves directory"),
    }
    let trimmed = name.trim_matches(|c| c <= '\u{20}');
    let name = if trimmed.is_empty() {
        default_name
    } else {
        trimmed
    };
    let probe = |candidate: &str| -> Result<bool> {
        match fs::symlink_metadata(saves.join(candidate)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).context("cannot inspect world folder"),
        }
    };
    match candidate_for(name, probe) {
        Ok(candidate) => Ok(candidate),
        Err(_) => candidate_for("World", probe).context("cannot select a world folder"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names_and_does_not_modify_existing_paths() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("World")).unwrap();
        fs::write(dir.path().join("World (1)"), b"existing").unwrap();
        assert_eq!(
            propose_world_folder(dir.path(), "World", "New World").unwrap(),
            "World (2)"
        );
        assert_eq!(
            propose_world_folder(dir.path(), " .COM1/ ", "New World").unwrap(),
            "_COM1_"
        );
        assert_eq!(
            propose_world_folder(dir.path(), "CON", "New World").unwrap(),
            "_CON_"
        );
        assert_eq!(
            propose_world_folder(dir.path(), "World (0)", "New World").unwrap(),
            "World (0)"
        );
        assert_eq!(
            propose_world_folder(dir.path(), "   ", "New World").unwrap(),
            "New World"
        );
        assert_eq!(fs::read(dir.path().join("World (1)")).unwrap(), b"existing");
        assert!(!dir.path().join("World (2)").exists());
    }

    #[test]
    fn parenthesized_suffixes_are_preserved_and_invalid_names_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        for input in ["Foo ()", "Foo (2147483648)"] {
            assert_eq!(
                propose_world_folder(dir.path(), input, "New World").unwrap(),
                input
            );
        }
        assert_eq!(
            propose_world_folder(dir.path(), "Bad\u{1}name", "New World").unwrap(),
            "World"
        );
    }

    #[test]
    fn proposal_never_creates_the_saves_directory_and_rejects_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert_eq!(
            propose_world_folder(&missing, "World", "New World").unwrap(),
            "World"
        );
        assert!(!missing.exists());
        fs::write(&missing, b"keep").unwrap();
        assert!(propose_world_folder(&missing, "World", "New World").is_err());
        assert_eq!(fs::read(&missing).unwrap(), b"keep");
    }
}
