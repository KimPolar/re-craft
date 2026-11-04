//! Proposal and exclusive creation of a new world's folder.
//!
//! A proposal does not reserve a name. The explicit reservation operation
//! creates a fresh directory atomically and retries collisions.
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

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

fn requested_name<'a>(name: &'a str, default_name: &'a str) -> &'a str {
    let trimmed = name.trim_matches(|c| c <= '\u{20}');
    if trimmed.is_empty() {
        default_name
    } else {
        trimmed
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
    let name = requested_name(name, default_name);
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

/// A newly created directory held under the vanilla session lock. It does not
/// yet contain a valid world. No automatic cleanup is performed because another
/// process could add data or replace paths after an error.
pub struct ReservedWorldFolder {
    pub name: String,
    pub path: PathBuf,
    session: crate::save::WorldSessionGuard,
}

impl ReservedWorldFolder {
    /// Share this guard with the future world session while initialization runs.
    pub fn session(&self) -> &crate::save::WorldSessionGuard {
        &self.session
    }
}

/// Create an empty folder using exclusive directory creation. A name shown in
/// the UI is only a proposal, so collisions are retried at creation time.
/// The returned guard keeps Java's session lock until its last clone is dropped.
/// The caller must still initialize all required world data.
/// This function never opens or modifies an existing world directory.
pub fn reserve_world_folder(
    saves: &Path,
    name: &str,
    default_name: &str,
) -> Result<ReservedWorldFolder> {
    #[cfg(not(windows))]
    {
        let _ = (saves, name, default_name);
        anyhow::bail!("world writing is currently supported only on Windows");
    }
    #[cfg(windows)]
    {
        match fs::create_dir(saves) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                ensure!(
                    fs::metadata(saves)
                        .context("cannot inspect saves directory")?
                        .is_dir(),
                    "saves path is not a directory"
                );
            }
            Err(error) => return Err(error).context("cannot create saves directory"),
        }
        // Resolve user-selected junctions before deriving a direct child path.
        let root = saves
            .canonicalize()
            .context("cannot resolve saves directory")?;
        ensure!(
            fs::metadata(&root)?.is_dir(),
            "saves path is not a directory"
        );
        let allocate = |candidate: &str| -> Result<bool> {
            match fs::create_dir(root.join(candidate)) {
                Ok(()) => Ok(false),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(true),
                Err(error) => Err(error).context("cannot create world folder"),
            }
        };
        let selected = match candidate_for(requested_name(name, default_name), allocate) {
            Ok(candidate) => candidate,
            Err(_) => candidate_for("World", allocate).context("cannot reserve a world folder")?,
        };
        let path = root.join(&selected);
        let metadata = fs::symlink_metadata(&path).context("cannot inspect new world folder")?;
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "new world folder is not an ordinary directory"
        );
        let path = path
            .canonicalize()
            .context("cannot resolve new world folder")?;
        ensure!(
            path.parent() == Some(root.as_path()),
            "new world folder moved outside saves"
        );
        let session = crate::save::WorldSessionGuard::acquire(&path)
            .context("cannot lock the newly created world folder")?;
        Ok(ReservedWorldFolder {
            path,
            name: selected,
            session,
        })
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

    #[test]
    #[cfg(windows)]
    fn reservation_rechecks_stale_proposals_and_preserves_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let saves = dir.path().join("saves");
        assert_eq!(
            propose_world_folder(&saves, "World", "New World").unwrap(),
            "World"
        );
        fs::create_dir(&saves).unwrap();
        fs::create_dir(saves.join("World")).unwrap();
        fs::write(saves.join("World").join("level.dat"), b"original").unwrap();
        fs::write(saves.join("World (1)"), b"existing file").unwrap();
        let reserved = reserve_world_folder(&saves, "World", "New World").unwrap();
        assert_eq!(reserved.name, "World (2)");
        assert_eq!(
            reserved.path,
            saves.canonicalize().unwrap().join("World (2)")
        );
        assert!(reserved.path.is_dir());
        assert_eq!(
            fs::read(saves.join("World").join("level.dat")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(saves.join("World (1)")).unwrap(), b"existing file");
    }

    #[test]
    #[cfg(windows)]
    fn concurrent_reservations_choose_distinct_directories() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let saves = Arc::new(dir.path().join("saves"));
        let barrier = Arc::new(Barrier::new(12));
        let mut threads = Vec::new();
        for _ in 0..12 {
            let saves = Arc::clone(&saves);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                reserve_world_folder(&saves, "World", "New World").unwrap()
            }));
        }
        let reserved: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let names: std::collections::BTreeSet<_> =
            reserved.iter().map(|folder| &folder.name).collect();
        assert_eq!(names.len(), 12);
        assert!(reserved.iter().all(|folder| folder.path.is_dir()));
        assert_eq!(fs::read_dir(&*saves).unwrap().count(), 12);
    }

    #[test]
    #[cfg(windows)]
    fn reservation_rejects_invalid_saves_path_without_altering_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("saves");
        fs::write(&file, b"keep").unwrap();
        assert!(reserve_world_folder(&file, "World", "New World").is_err());
        assert_eq!(fs::read(file).unwrap(), b"keep");
        let missing_parent = dir.path().join("missing").join("saves");
        assert!(reserve_world_folder(&missing_parent, "World", "New World").is_err());
        assert!(!missing_parent.exists());
    }

    #[test]
    #[cfg(windows)]
    fn reservation_holds_the_vanilla_session_lock() {
        let dir = tempfile::tempdir().unwrap();
        let folder = reserve_world_folder(&dir.path().join("saves"), "World", "New World").unwrap();
        let path = folder.path.clone();
        let shared = folder.session().clone();
        let error = crate::save::WorldSessionGuard::acquire(&path)
            .err()
            .unwrap();
        assert!(
            error
                .chain()
                .any(|error| error.is::<crate::save::WorldInUse>())
        );
        drop(folder);
        assert!(crate::save::WorldSessionGuard::acquire(&path).is_err());
        drop(shared);
        assert!(crate::save::WorldSessionGuard::acquire(&path).is_ok());
    }
}
