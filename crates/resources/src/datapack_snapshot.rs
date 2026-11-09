//! Freeze world data packs before generation, without changing the source.
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

#[derive(Debug, Eq, PartialEq)]
enum Entry {
    Directory,
    File { len: u64, hash: [u8; 32] },
}

/// A private copy of every world data pack, including disabled packs. The
/// original may change after capture; generation reads only this snapshot.
pub struct DataPackSnapshot {
    directory: tempfile::TempDir,
    present: bool,
    manifest: BTreeMap<PathBuf, Entry>,
}

impl DataPackSnapshot {
    pub fn capture(world: &Path) -> Result<Self> {
        let world = world
            .canonicalize()
            .context("cannot open data pack source world")?;
        ensure!(world.is_dir(), "data pack source is not a directory");
        let source = world.join("datapacks");
        let present = match fs::symlink_metadata(&source) {
            Ok(metadata) => {
                ensure!(
                    ordinary(&metadata) && metadata.is_dir(),
                    "data pack directory must be an ordinary directory"
                );
                ensure!(
                    source.canonicalize()?.starts_with(&world),
                    "data pack directory is outside the source world"
                );
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).context("cannot inspect data pack directory"),
        };
        let directory = tempfile::Builder::new()
            .prefix("re-craft-packs-")
            .tempdir()?;
        let target = directory.path().join("datapacks");
        let manifest = if present {
            fs::create_dir(&target)?;
            let copied = scan(&source, Some(&target))?;
            ensure!(
                scan(&source, None)? == copied,
                "data packs changed while being copied"
            );
            ensure!(
                scan(&target, None)? == copied,
                "data pack snapshot did not round-trip"
            );
            copied
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            directory,
            present,
            manifest,
        })
    }

    /// Root used by the resource stack; callers must not modify its files.
    pub(crate) fn world_path(&self) -> &Path {
        self.directory.path()
    }

    pub fn verify(&self) -> Result<()> {
        let root = self.directory.path().join("datapacks");
        if self.present {
            let metadata = fs::symlink_metadata(&root).context("data pack snapshot is missing")?;
            ensure!(
                ordinary(&metadata) && metadata.is_dir(),
                "data pack snapshot is not an ordinary directory"
            );
            ensure!(
                scan(&root, None)? == self.manifest,
                "data pack snapshot changed during generation"
            );
        } else {
            ensure!(
                matches!(fs::symlink_metadata(&root), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "unexpected data pack snapshot directory"
            );
        }
        Ok(())
    }

    /// Verify that a world's current pack tree still matches this snapshot.
    /// This does not lock the source against a later external modification.
    pub fn verify_source(&self, world: &Path) -> Result<()> {
        let world = world
            .canonicalize()
            .context("cannot resolve data pack source world")?;
        ensure!(world.is_dir(), "data pack source is not a directory");
        let source = world.join("datapacks");
        if self.present {
            let metadata =
                fs::symlink_metadata(&source).context("source data packs are missing")?;
            ensure!(
                ordinary(&metadata) && metadata.is_dir(),
                "source data packs are not an ordinary directory"
            );
            ensure!(
                source.canonicalize()?.starts_with(&world),
                "source data packs are outside the world"
            );
            ensure!(
                scan(&source, None)? == self.manifest,
                "source data packs changed after capture"
            );
        } else {
            ensure!(
                matches!(fs::symlink_metadata(&source), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "source data packs appeared after capture"
            );
        }
        Ok(())
    }

    /// Stable fingerprint of pack names, kinds, lengths and file contents.
    /// This is suitable for a durable save-transaction dependency.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        manifest_fingerprint(self.present, &self.manifest)
    }

    /// Copy into a new staged world; on failure the caller must discard it.
    pub fn copy_into(&self, world: &Path) -> Result<()> {
        self.verify()?;
        let target = world.join("datapacks");
        ensure!(
            matches!(fs::symlink_metadata(&target), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "staged data pack directory already exists"
        );
        if self.present {
            fs::create_dir(&target)?;
            let copied = scan(&self.directory.path().join("datapacks"), Some(&target))?;
            ensure!(
                copied == self.manifest && scan(&target, None)? == self.manifest,
                "staged data packs differ from the generation snapshot"
            );
            self.verify()?;
        }
        Ok(())
    }
}

/// Fingerprint the current tree using the same format as a captured snapshot.
pub fn source_fingerprint(world: &Path) -> Result<[u8; 32]> {
    let world = world
        .canonicalize()
        .context("cannot resolve data pack source world")?;
    ensure!(world.is_dir(), "data pack source is not a directory");
    let source = world.join("datapacks");
    match fs::symlink_metadata(&source) {
        Ok(metadata) => {
            ensure!(
                ordinary(&metadata) && metadata.is_dir(),
                "source data packs are not an ordinary directory"
            );
            ensure!(
                source.canonicalize()?.parent() == Some(world.as_path()),
                "source data packs are outside the world"
            );
            manifest_fingerprint(true, &scan(&source, None)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            manifest_fingerprint(false, &BTreeMap::new())
        }
        Err(error) => Err(error).context("cannot inspect source data packs"),
    }
}

fn manifest_fingerprint(present: bool, manifest: &BTreeMap<PathBuf, Entry>) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(b"re-craft-data-pack-tree-v1\0");
    hash.update([u8::from(present)]);
    hash.update((manifest.len() as u64).to_le_bytes());
    for (path, entry) in manifest {
        #[cfg(windows)]
        let name = {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        #[cfg(unix)]
        let name = {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes().to_vec()
        };
        #[cfg(not(any(windows, unix)))]
        let name = path
            .to_str()
            .context("unsupported data pack path encoding")?
            .as_bytes()
            .to_vec();
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name);
        match entry {
            Entry::Directory => hash.update([0]),
            Entry::File { len, hash: content } => {
                hash.update([1]);
                hash.update(len.to_le_bytes());
                hash.update(content);
            }
        }
    }
    Ok(hash.finalize().into())
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

fn scan(root: &Path, target: Option<&Path>) -> Result<BTreeMap<PathBuf, Entry>> {
    let root = root.canonicalize()?;
    let mut pending = vec![PathBuf::new()];
    let mut manifest = BTreeMap::new();
    while let Some(relative) = pending.pop() {
        for item in fs::read_dir(root.join(&relative))? {
            let item = item?;
            let path = relative.join(item.file_name());
            let source = root.join(&path);
            let metadata = fs::symlink_metadata(&source)?;
            ensure!(
                ordinary(&metadata),
                "data pack contains a symbolic link or reparse point: {}",
                source.display()
            );
            ensure!(
                source.canonicalize()?.starts_with(&root),
                "data pack entry is outside its root"
            );
            let entry = if metadata.is_dir() {
                if let Some(target) = target {
                    fs::create_dir(target.join(&path))?;
                }
                pending.push(path.clone());
                Entry::Directory
            } else {
                ensure!(metadata.is_file(), "data pack contains a non-file entry");
                let mut input = File::open(&source)?;
                let mut output = target
                    .map(|target| {
                        OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(target.join(&path))
                    })
                    .transpose()?;
                let mut hash = Sha256::new();
                let mut size = 0u64;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hash.update(&buffer[..count]);
                    if let Some(output) = &mut output {
                        output.write_all(&buffer[..count])?;
                    }
                    size = size
                        .checked_add(count as u64)
                        .context("data pack size overflow")?;
                }
                if let Some(output) = &mut output {
                    output.flush()?;
                }
                Entry::File {
                    len: size,
                    hash: hash.finalize().into(),
                }
            };
            ensure!(
                manifest.insert(path, entry).is_none(),
                "duplicate data pack entry"
            );
            ensure!(manifest.len() <= 1_000_000, "too many data pack files");
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_preserves_all_packs_and_is_independent_of_later_source_edits() {
        let source = tempfile::tempdir().unwrap();
        let packs = source.path().join("datapacks");
        fs::create_dir_all(packs.join("active/data/example")).unwrap();
        fs::create_dir_all(packs.join("disabled/empty")).unwrap();
        fs::write(packs.join("active/pack.mcmeta"), b"metadata").unwrap();
        fs::write(packs.join("active/data/example/value.json"), b"original").unwrap();
        fs::write(packs.join("disabled.zip"), b"archive bytes").unwrap();
        let snapshot = DataPackSnapshot::capture(source.path()).unwrap();
        snapshot.verify_source(source.path()).unwrap();
        let fingerprint = snapshot.fingerprint().unwrap();
        assert_eq!(source_fingerprint(source.path()).unwrap(), fingerprint);
        fs::write(packs.join("active/data/example/value.json"), b"modified").unwrap();
        assert!(snapshot.verify_source(source.path()).is_err());
        assert_ne!(source_fingerprint(source.path()).unwrap(), fingerprint);
        fs::write(packs.join("active/data/example/value.json"), b"original").unwrap();
        snapshot.verify_source(source.path()).unwrap();
        fs::write(packs.join("later.zip"), b"new").unwrap();
        assert!(snapshot.verify_source(source.path()).is_err());
        assert_ne!(source_fingerprint(source.path()).unwrap(), fingerprint);
        fs::write(packs.join("active/data/example/value.json"), b"modified").unwrap();
        let stage = tempfile::tempdir().unwrap();
        snapshot.copy_into(stage.path()).unwrap();
        let staged = stage.path().join("datapacks");
        assert_eq!(
            fs::read(staged.join("active/data/example/value.json")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(staged.join("disabled.zip")).unwrap(),
            b"archive bytes"
        );
        assert!(staged.join("disabled/empty").is_dir());
        assert!(!staged.join("later.zip").exists());
        assert!(snapshot.copy_into(stage.path()).is_err());
        assert_eq!(
            fs::read(packs.join("active/data/example/value.json")).unwrap(),
            b"modified"
        );
    }

    #[test]
    fn snapshot_detects_tampering_and_missing_directories() {
        let source = tempfile::tempdir().unwrap();
        fs::create_dir(source.path().join("datapacks")).unwrap();
        fs::write(source.path().join("datapacks/pack.zip"), b"original").unwrap();
        let snapshot = DataPackSnapshot::capture(source.path()).unwrap();
        fs::write(
            snapshot.world_path().join("datapacks/pack.zip"),
            b"different",
        )
        .unwrap();
        let stage = tempfile::tempdir().unwrap();
        assert!(snapshot.verify().is_err());
        assert!(snapshot.copy_into(stage.path()).is_err());
        assert!(!stage.path().join("datapacks").exists());
        let empty_source = tempfile::tempdir().unwrap();
        let empty = DataPackSnapshot::capture(empty_source.path()).unwrap();
        empty.verify_source(empty_source.path()).unwrap();
        assert_eq!(
            source_fingerprint(empty_source.path()).unwrap(),
            empty.fingerprint().unwrap()
        );
        fs::create_dir(empty_source.path().join("datapacks")).unwrap();
        assert!(empty.verify_source(empty_source.path()).is_err());
        assert_ne!(
            source_fingerprint(empty_source.path()).unwrap(),
            empty.fingerprint().unwrap()
        );
        empty.copy_into(stage.path()).unwrap();
        assert!(!stage.path().join("datapacks").exists());
    }

    #[cfg(windows)]
    #[test]
    fn snapshot_rejects_directory_links_when_supported() {
        let source = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::create_dir(source.path().join("datapacks")).unwrap();
        let link = source.path().join("datapacks/link");
        if std::os::windows::fs::symlink_dir(other.path(), &link).is_ok() {
            assert!(DataPackSnapshot::capture(source.path()).is_err());
        }
    }
}
