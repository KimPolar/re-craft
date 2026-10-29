//! Experimental, lossless region rewrites. This does not yet save gameplay changes.
//!
//! A whole region is copied to a same-directory staging file, checked, backed up,
//! and atomically replaced while holding the vanilla world lock. Other chunks are
//! copied byte for byte. The only exposed operation rewrites an existing chunk's
//! unchanged NBT; gameplay editing must not be enabled until the simulation and
//! dependent world data can also be preserved.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use flate2::{Compression, write::ZlibEncoder};
use sha2::{Digest, Sha256};

use crate::{
    anvil::{self, RegionReader},
    nbt::{self, Tag},
    palette,
    world::{DimensionId, TARGET_DATA_VERSION},
};

const SECTOR_SIZE: u64 = 4096;
const HEADER_SIZE: u64 = 2 * SECTOR_SIZE;
const REGION_SIDE: i32 = 32;
const MAX_OFFSET: u64 = 0x00ff_ffff;

/// The backup contains the original region and any external chunks it references.
#[derive(Debug)]
pub struct RewriteResult {
    pub region_path: PathBuf,
    pub backup_directory: PathBuf,
}

/// A recoverable native save was interrupted; opening must wait for recovery.
#[derive(Debug)]
pub struct InterruptedWorldSave;

impl std::fmt::Display for InterruptedWorldSave {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "an interrupted Minecraft Native save needs recovery before this world can be opened",
        )
    }
}

impl std::error::Error for InterruptedWorldSave {}

/// Another process or session currently holds the world's Java lock.
#[derive(Debug)]
pub struct WorldInUse;

impl std::fmt::Display for WorldInUse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("world is already in use")
    }
}

impl std::error::Error for WorldInUse {}

/// Keeps the vanilla session lock alive while a world is in use. Clones share
/// the same OS lock, including across the client and its chunk reader.
#[derive(Clone)]
pub struct WorldSessionGuard {
    shared: Arc<SessionLock>,
}

struct SessionLock {
    root: PathBuf,
    _lock: WorldLock,
    writer_active: AtomicBool,
}

impl WorldSessionGuard {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(SessionLock {
                root: root.to_path_buf(),
                _lock: WorldLock::acquire(root)?,
                writer_active: AtomicBool::new(false),
            }),
        })
    }

    /// Reuse this session's OS lock for a single writer. The caller must pause
    /// conflicting reads and coordinate the simulation before a gameplay save.
    /// Only the existing lossless rewrite operation is currently available.
    pub fn acquire_write(&self) -> Result<WorldWriteGuard> {
        ensure!(
            self.shared
                .writer_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "another world write is already in progress"
        );
        let guard = WorldWriteGuard {
            root: self.shared.root.clone(),
            session: self.clone(),
        };
        crate::save_transaction::ensure_no_pending(&guard.root)?;
        let level = anvil::read_level(&guard.root)?;
        ensure!(
            level.data_version == TARGET_DATA_VERSION,
            "unsupported world DataVersion {}; expected {TARGET_DATA_VERSION}",
            level.data_version
        );
        Ok(guard)
    }
}

/// Holds the same exclusive session.lock used by the Java client.
pub struct WorldWriteGuard {
    root: PathBuf,
    session: WorldSessionGuard,
}

impl Drop for WorldWriteGuard {
    fn drop(&mut self) {
        self.session
            .shared
            .writer_active
            .store(false, Ordering::Release);
    }
}

impl WorldWriteGuard {
    pub fn acquire(world: &Path) -> Result<Self> {
        let root = world
            .canonicalize()
            .with_context(|| format!("cannot open world {}", world.display()))?;
        ensure!(root.is_dir(), "world path is not a directory");
        WorldSessionGuard::acquire(&root)?.acquire_write()
    }

    /// Roll back a previously interrupted native transaction while holding the
    /// vanilla world lock. The backup is retained, and conflicting files stop
    /// recovery without being overwritten.
    pub fn recover_interrupted(world: &Path) -> Result<bool> {
        let root = world
            .canonicalize()
            .with_context(|| format!("cannot open world {}", world.display()))?;
        ensure!(root.is_dir(), "world path is not a directory");
        let _lock = WorldLock::acquire(&root)?;
        crate::save_transaction::recover(&root)
    }

    /// Recompress an existing full 26.3 chunk without changing its NBT.
    /// No gameplay data is edited by this operation.
    pub fn rewrite_chunk_losslessly(
        &mut self,
        dimension: &DimensionId,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<RewriteResult> {
        let region_x = chunk_x.div_euclid(REGION_SIDE);
        let region_z = chunk_z.div_euclid(REGION_SIDE);
        let region_dir = dimension
            .region_dir(&self.root)
            .canonicalize()
            .context("cannot open dimension region directory")?;
        ensure!(
            region_dir.starts_with(&self.root),
            "region directory is outside the world"
        );
        let name = format!("r.{region_x}.{region_z}.mca");
        let region_path = region_dir.join(&name);
        let metadata =
            fs::symlink_metadata(&region_path).context("cannot inspect existing region")?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "region must be an ordinary file"
        );
        let canonical = region_path.canonicalize()?;
        ensure!(
            canonical.parent() == Some(region_dir.as_path()),
            "region resolves outside its directory"
        );

        let mut stage = tempfile::Builder::new()
            .prefix(".native-region-")
            .tempfile_in(&region_dir)
            .context("cannot create region staging file")?;
        let mut original = File::open(&canonical)?;
        let original_length = original.metadata()?.len();
        ensure!(
            original_length >= HEADER_SIZE
                && original_length.is_multiple_of(SECTOR_SIZE)
                && original_length / SECTOR_SIZE <= MAX_OFFSET + 1,
            "invalid or unsupported region file length"
        );
        std::io::copy(&mut original, stage.as_file_mut())
            .context("cannot stage original region")?;
        stage.as_file_mut().flush()?;
        let baseline_hash = hash_file(stage.path())?;
        let external = audit_region(stage.as_file_mut(), &region_dir, region_x, region_z)?;

        let original_nbt = RegionReader::open_for_region(stage.path(), region_x, region_z)?
            .read_chunk_nbt_bytes(chunk_x, chunk_z)?
            .context("cannot rewrite a missing chunk")?;
        let chunk = nbt::read(&original_nbt)?;
        validate_chunk(&chunk.value, chunk_x, chunk_z)?;
        let encoded = nbt::write(&chunk)?;
        ensure!(
            encoded == original_nbt,
            "chunk NBT cannot be reserialized without changing its bytes"
        );
        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&encoded)?;
        let compressed = compressor.finish()?;
        let length = compressed
            .len()
            .checked_add(1)
            .context("chunk payload length overflow")?;
        let allocation = length
            .checked_add(4)
            .context("chunk allocation overflow")?
            .div_ceil(SECTOR_SIZE as usize);
        ensure!(
            allocation <= 255,
            "rewritten chunk needs external storage, which is not yet supported"
        );

        let index = chunk_x.rem_euclid(REGION_SIDE) as u64
            + chunk_z.rem_euclid(REGION_SIDE) as u64 * REGION_SIDE as u64;
        stage.as_file_mut().seek(SeekFrom::Start(index * 4))?;
        let mut previous_location = [0u8; 4];
        stage.as_file_mut().read_exact(&mut previous_location)?;
        let previous_location = u32::from_be_bytes(previous_location);
        let previous_offset = u64::from(previous_location >> 8);
        let previous_sectors = u64::from(previous_location & 255);
        let reuse = allocation as u64 <= previous_sectors;
        let offset = if reuse {
            previous_offset
        } else {
            stage.as_file().metadata()?.len() / SECTOR_SIZE
        };
        let sectors = if reuse {
            previous_sectors
        } else {
            allocation as u64
        };
        ensure!(
            offset <= MAX_OFFSET && offset + sectors <= MAX_OFFSET + 1,
            "region has no addressable space for the rewritten chunk"
        );
        let backup_directory =
            backup_region(&self.root, dimension, &name, stage.path(), &external)?;
        stage
            .as_file_mut()
            .seek(SeekFrom::Start(offset * SECTOR_SIZE))?;
        stage
            .as_file_mut()
            .write_all(&(length as u32).to_be_bytes())?;
        stage.as_file_mut().write_all(&[2])?;
        stage.as_file_mut().write_all(&compressed)?;
        if !reuse {
            let padding = allocation * SECTOR_SIZE as usize - length - 4;
            stage.as_file_mut().write_all(&vec![0; padding])?;
        }
        stage.as_file_mut().seek(SeekFrom::Start(index * 4))?;
        let location = ((offset as u32) << 8) | sectors as u32;
        stage.as_file_mut().write_all(&location.to_be_bytes())?;
        stage.as_file_mut().flush()?;
        stage
            .as_file()
            .sync_all()
            .context("cannot flush staged region")?;

        let verify = RegionReader::open_for_region(stage.path(), region_x, region_z)?
            .read_chunk(chunk_x, chunk_z)?
            .context("staged chunk disappeared")?;
        ensure!(
            nbt::write(&verify)? == encoded,
            "staged chunk failed the NBT round-trip check"
        );
        drop(original);
        let backup = backup_directory.join(&name);
        let dependencies = external
            .iter()
            .map(|path| {
                let name = path.file_name().context("external chunk has no filename")?;
                Ok(crate::save_transaction::Dependency {
                    target: path.clone(),
                    backup: backup_directory.join(name),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        crate::save_transaction::commit(
            &self.root,
            &backup_directory,
            vec![crate::save_transaction::Replacement {
                target: canonical.clone(),
                backup,
                stage,
                original_hash: baseline_hash,
            }],
            dependencies,
        )
        .with_context(|| {
            format!(
                "cannot atomically replace region; backup retained at {}",
                backup_directory.display()
            )
        })?;
        Ok(RewriteResult {
            region_path: canonical,
            backup_directory,
        })
    }
}

pub(crate) fn validate_chunk(chunk: &Tag, x: i32, z: i32) -> Result<()> {
    let Tag::Compound(fields) = chunk else {
        anyhow::bail!("chunk NBT root is not a compound");
    };
    for name in ["DataVersion", "xPos", "zPos", "Status", "sections"] {
        ensure!(
            fields.iter().filter(|(key, _)| key.matches(name)).count() <= 1,
            "duplicate chunk {name} tags are not supported"
        );
    }
    ensure!(
        matches!(chunk.get("DataVersion"), Some(Tag::Int(value)) if *value == TARGET_DATA_VERSION),
        "chunk is not data version {TARGET_DATA_VERSION}"
    );
    ensure!(
        matches!(chunk.get("xPos"), Some(Tag::Int(value)) if *value == x),
        "chunk X coordinate does not match its region entry"
    );
    ensure!(
        matches!(chunk.get("zPos"), Some(Tag::Int(value)) if *value == z),
        "chunk Z coordinate does not match its region entry"
    );
    ensure!(
        matches!(chunk.get("Status"), Some(Tag::String(value)) if value.matches("minecraft:full")),
        "only full chunks can be rewritten"
    );
    palette::decode_sections(chunk).context("unsupported or invalid chunk section")?;
    palette::validate_26_3_block_states(chunk).context("unsupported 26.3 block-state format")?;
    Ok(())
}

fn audit_region(
    file: &mut File,
    directory: &Path,
    region_x: i32,
    region_z: i32,
) -> Result<Vec<PathBuf>> {
    let length = file.metadata()?.len();
    ensure!(
        length >= HEADER_SIZE && length.is_multiple_of(SECTOR_SIZE),
        "invalid or incomplete region file length"
    );
    ensure!(
        length / SECTOR_SIZE <= MAX_OFFSET + 1,
        "region is larger than the location table supports"
    );
    let mut header = [0u8; HEADER_SIZE as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let mut intervals = Vec::new();
    let mut external = Vec::new();
    for index in 0..1024usize {
        let offset = index * 4;
        let entry = u32::from_be_bytes(header[offset..offset + 4].try_into()?);
        if entry == 0 {
            continue;
        }
        let start = u64::from(entry >> 8);
        let count = u64::from(entry & 255);
        ensure!(
            start >= 2 && count > 0 && start + count <= length / SECTOR_SIZE,
            "invalid chunk sector allocation at entry {index}"
        );
        intervals.push((start, start + count));
        file.seek(SeekFrom::Start(start * SECTOR_SIZE))?;
        let mut record = [0u8; 5];
        file.read_exact(&mut record)?;
        let stored_length = u64::from(u32::from_be_bytes(record[..4].try_into()?));
        ensure!(
            stored_length >= 1 && stored_length <= count * SECTOR_SIZE - 4,
            "invalid chunk length at entry {index}"
        );
        let compression = record[4] & 0x7f;
        ensure!(
            (1..=4).contains(&compression),
            "unsupported chunk compression {compression} at entry {index}"
        );
        if record[4] & 0x80 != 0 {
            ensure!(
                stored_length == 1,
                "external chunk marker has inline data at entry {index}"
            );
            let x = region_x
                .checked_mul(REGION_SIDE)
                .and_then(|value| value.checked_add((index % 32) as i32))
                .context("external chunk X overflows")?;
            let z = region_z
                .checked_mul(REGION_SIDE)
                .and_then(|value| value.checked_add((index / 32) as i32))
                .context("external chunk Z overflows")?;
            let path = directory.join(format!("c.{x}.{z}.mcc"));
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("external chunk is missing: {}", path.display()))?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "external chunk is not an ordinary file"
            );
            ensure!(
                metadata.len() <= nbt::MAX_NBT_BYTES as u64,
                "external chunk exceeds the supported size limit"
            );
            ensure!(
                path.canonicalize()?.parent() == Some(directory),
                "external chunk resolves outside its region directory"
            );
            external.push(path);
        }
    }
    intervals.sort_unstable();
    ensure!(
        intervals.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "overlapping region chunk allocations"
    );
    Ok(external)
}

fn backup_region(
    root: &Path,
    dimension: &DimensionId,
    name: &str,
    original: &Path,
    external: &[PathBuf],
) -> Result<PathBuf> {
    let backups = root.join(".minecraft-native-backups");
    match fs::symlink_metadata(&backups) {
        Ok(meta) => ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "backup directory must be an ordinary directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&backups)?,
        Err(error) => return Err(error.into()),
    }
    ensure!(
        backups.canonicalize()?.starts_with(root),
        "backup directory is outside the world"
    );
    let dir = tempfile::Builder::new()
        .prefix("region-")
        .tempdir_in(&backups)?;
    copy_sync(original, &dir.path().join(name))?;
    for path in external {
        let file_name = path.file_name().context("external chunk has no filename")?;
        copy_sync(path, &dir.path().join(file_name))?;
    }
    let mut manifest = File::create(dir.path().join("backup.txt"))?;
    writeln!(
        manifest,
        "Minecraft Native region backup\ndimension={}:{}\nregion={name}\nexternal_chunks={}",
        dimension.namespace,
        dimension.path,
        external.len()
    )?;
    manifest.sync_all()?;
    Ok(dir.keep())
}

fn copy_sync(source: &Path, destination: &Path) -> Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(windows)]
struct WorldLock(File);

#[cfg(windows)]
impl WorldLock {
    fn acquire(root: &Path) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION},
            Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
            System::IO::OVERLAPPED,
        };
        let lock_error = |error: std::io::Error| {
            if matches!(error.raw_os_error(), Some(value) if value == ERROR_LOCK_VIOLATION as i32 || value == ERROR_SHARING_VIOLATION as i32)
            {
                anyhow::Error::new(WorldInUse).context(error)
            } else {
                anyhow::Error::new(error).context("session.lock cannot be locked")
            }
        };
        let path = root.join("session.lock");
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "session.lock must be an ordinary file"
            );
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(lock_error)?;
        let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
        // Java FileChannel.tryLock() locks [0, Long.MAX_VALUE) on this file.
        let ok = unsafe {
            LockFileEx(
                file.as_raw_handle() as _,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                0x7fff_ffff,
                &mut overlap,
            )
        };
        if ok == 0 {
            return Err(lock_error(std::io::Error::last_os_error()));
        }
        Ok(Self(file))
    }
}

#[cfg(windows)]
impl Drop for WorldLock {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};
        let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
        unsafe {
            UnlockFileEx(
                self.0.as_raw_handle() as _,
                0,
                u32::MAX,
                0x7fff_ffff,
                &mut overlap,
            );
        }
    }
}

#[cfg(not(windows))]
struct WorldLock;

#[cfg(not(windows))]
impl WorldLock {
    fn acquire(_root: &Path) -> Result<Self> {
        anyhow::bail!("world writing is currently supported only on Windows")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::nbt::{NamedTag, TagType};
    use flate2::write::{GzEncoder, ZlibEncoder};

    fn test_world() -> tempfile::TempDir {
        // Windows sandboxes may forbid replacement in the system temporary folder.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
        fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(root)
            .unwrap()
    }

    fn chunk(x: i32, marker: &str) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(x)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
                (
                    "unknown".into(),
                    Tag::Compound(vec![("value".into(), Tag::String(marker.into()))]),
                ),
            ]),
        }
    }

    #[test]
    fn ambiguous_chunk_fields_cannot_be_rewritten() {
        let original = chunk(0, "preserved");
        assert!(validate_chunk(&original.value, 0, 0).is_ok());
        for name in ["DataVersion", "xPos", "zPos", "Status", "sections"] {
            let mut ambiguous = original.clone();
            let Tag::Compound(fields) = &mut ambiguous.value else {
                unreachable!()
            };
            let duplicate = fields
                .iter()
                .find(|(key, _)| key.matches(name))
                .unwrap()
                .clone();
            fields.push(duplicate);
            assert!(validate_chunk(&ambiguous.value, 0, 0).is_err(), "{name}");
        }
    }

    fn compressed(root: &NamedTag) -> Vec<u8> {
        let mut output = ZlibEncoder::new(Vec::new(), Compression::default());
        output.write_all(&nbt::write(root).unwrap()).unwrap();
        output.finish().unwrap()
    }

    fn fixture(root: &Path) -> (PathBuf, Vec<u8>, Vec<u8>) {
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
        fs::write(root.join("level.dat"), gzip.finish().unwrap()).unwrap();
        let directory = root.join("dimensions/minecraft/overworld/region");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let mut region = vec![0u8; 5 * SECTOR_SIZE as usize];
        for index in 0..3usize {
            region[index * 4..index * 4 + 4]
                .copy_from_slice(&((((index + 2) as u32) << 8) | 1).to_be_bytes());
            region[4096 + index * 4..4096 + index * 4 + 4]
                .copy_from_slice(&(1000 + index as u32).to_be_bytes());
        }
        for x in 0..2usize {
            let bytes = compressed(&chunk(x as i32, if x == 0 { "target" } else { "neighbor" }));
            let start = (x + 2) * SECTOR_SIZE as usize;
            region[start..start + 4].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
            region[start + 4] = 2;
            region[start + 5..start + 5 + bytes.len()].copy_from_slice(&bytes);
        }
        let start = 4 * SECTOR_SIZE as usize;
        region[start..start + 4].copy_from_slice(&1u32.to_be_bytes());
        region[start + 4] = 0x82;
        let outside = compressed(&chunk(2, "external"));
        fs::write(directory.join("c.2.0.mcc"), &outside).unwrap();
        fs::write(&path, &region).unwrap();
        (path, region, outside)
    }

    #[test]
    fn playable_reader_shares_lock_until_last_session_owner_is_dropped() {
        use crate::world::WorldReader;
        let world = test_world();
        let (region, original, external) = fixture(world.path());
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut reader = WorldReader::open_for_session(world.path(), dimension.clone()).unwrap();
        assert!(reader.read_chunk(0, 0).unwrap().is_some());
        let error = WorldReader::open_for_session(world.path(), dimension)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<WorldInUse>().is_some());
        assert!(
            WorldWriteGuard::acquire(world.path())
                .err()
                .unwrap()
                .downcast_ref::<WorldInUse>()
                .is_some()
        );
        let shared = reader.session_guard().unwrap();
        drop(reader);
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        drop(shared);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
        assert_eq!(fs::read(region).unwrap(), original);
        assert_eq!(
            fs::read(
                world
                    .path()
                    .join("dimensions/minecraft/overworld/region/c.2.0.mcc")
            )
            .unwrap(),
            external
        );
    }

    #[test]
    fn active_session_can_share_its_lock_with_one_writer() {
        use crate::world::WorldReader;
        let world = test_world();
        let (region, original, external) = fixture(world.path());
        let reader =
            WorldReader::open_for_session(world.path(), DimensionId::parse("overworld").unwrap())
                .unwrap();
        let session = reader.session_guard().unwrap();
        let mut writer = session.acquire_write().unwrap();
        assert!(session.acquire_write().is_err());
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        let result = writer
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
            external
        );
        assert!(region.is_file());
        drop(writer);
        assert!(session.acquire_write().is_ok());
        drop(reader);
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        drop(session);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
    }

    #[test]
    fn lossless_rewrite_keeps_untouched_records_and_backups_external_chunks() {
        let world = test_world();
        let (region_path, old_region, outside) = fixture(world.path());
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        let result = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            old_region
        );
        assert_eq!(
            fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
            outside
        );
        let new_region = fs::read(&region_path).unwrap();
        assert_eq!(
            &new_region[..HEADER_SIZE as usize],
            &old_region[..HEADER_SIZE as usize]
        );
        assert_eq!(
            &new_region[3 * SECTOR_SIZE as usize..],
            &old_region[3 * SECTOR_SIZE as usize..]
        );
        assert_eq!(new_region.len(), old_region.len());
        let mut reader = RegionReader::open(&region_path).unwrap();
        for x in 0..3 {
            let expected = chunk(x, ["target", "neighbor", "external"][x as usize]);
            assert_eq!(
                nbt::write(&reader.read_chunk(x, 0).unwrap().unwrap()).unwrap(),
                nbt::write(&expected).unwrap()
            );
        }
        drop(guard);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
    }

    #[test]
    fn unsupported_and_overlapping_records_stop_before_replacement() {
        let world = test_world();
        let (path, mut bytes, _) = fixture(world.path());
        let dim = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let original_entry = bytes[0..4].to_vec();
        bytes[4..8].copy_from_slice(&original_entry);
        fs::write(&path, &bytes).unwrap();
        assert!(
            guard
                .rewrite_chunk_losslessly(&dim, 0, 0)
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
        bytes[4..8].copy_from_slice(&((3u32 << 8) | 1).to_be_bytes());
        bytes[3 * SECTOR_SIZE as usize + 4] = 99;
        fs::write(&path, &bytes).unwrap();
        assert!(
            guard
                .rewrite_chunk_losslessly(&dim, 0, 0)
                .unwrap_err()
                .to_string()
                .contains("unsupported chunk compression")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn legacy_shaped_palette_is_not_written_as_26_3() {
        let world = test_world();
        let (path, mut bytes, _) = fixture(world.path());
        let mut target = chunk(0, "target");
        let Tag::Compound(fields) = &mut target.value else {
            unreachable!()
        };
        let sections = fields
            .iter_mut()
            .find(|(name, _)| name.matches("sections"))
            .unwrap();
        sections.1 = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![(
                        "palette".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![Tag::Compound(vec![(
                                "Name".into(),
                                Tag::String("minecraft:stone".into()),
                            )])],
                        },
                    )]),
                ),
            ])],
        };
        let compressed = compressed(&target);
        let start = 2 * SECTOR_SIZE as usize;
        bytes[start..start + SECTOR_SIZE as usize].fill(0);
        bytes[start..start + 4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        bytes[start + 4] = 2;
        bytes[start + 5..start + 5 + compressed.len()].copy_from_slice(&compressed);
        fs::write(&path, &bytes).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let error = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap_err();
        assert!(format!("{error:#}").contains("legacy block-state format"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn duplicate_biome_or_light_fields_leave_region_untouched() {
        for name in ["biomes", "BlockLight", "SkyLight", "palette", "data"] {
            let world = test_world();
            let (path, mut bytes, _) = fixture(world.path());
            let mut biome_fields = vec![
                (
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String("minecraft:plains".into())],
                    },
                ),
                ("data".into(), Tag::LongArray(vec![0])),
            ];
            if matches!(name, "palette" | "data") {
                biome_fields.push(
                    biome_fields
                        .iter()
                        .find(|(key, _): &&(crate::nbt::JavaString, Tag)| key.matches(name))
                        .unwrap()
                        .clone(),
                );
            }
            let mut section = vec![
                ("Y".into(), Tag::Byte(0)),
                ("biomes".into(), Tag::Compound(biome_fields)),
                ("BlockLight".into(), Tag::ByteArray(vec![0; 2048])),
                ("SkyLight".into(), Tag::ByteArray(vec![0; 2048])),
            ];
            if !matches!(name, "palette" | "data") {
                section.push(
                    section
                        .iter()
                        .find(|(key, _): &&(crate::nbt::JavaString, Tag)| key.matches(name))
                        .unwrap()
                        .clone(),
                );
            }
            let mut target = chunk(0, "target");
            let Tag::Compound(fields) = &mut target.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("sections"))
                .unwrap()
                .1 = Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(section)],
            };
            let payload = compressed(&target);
            let start = 2 * SECTOR_SIZE as usize;
            bytes[start..start + SECTOR_SIZE as usize].fill(0);
            bytes[start..start + 4].copy_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
            bytes[start + 4] = 2;
            bytes[start + 5..start + 5 + payload.len()].copy_from_slice(&payload);
            fs::write(&path, &bytes).unwrap();
            let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
            let error = guard
                .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
                .unwrap_err();
            assert!(
                format!("{error:#}").contains("duplicate"),
                "{name}: {error:#}"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name}");
            assert!(!world.path().join(".minecraft-native-backups").exists());
        }
    }

    #[test]
    fn missing_external_chunk_and_noncanonical_nbt_stop_without_a_save() {
        let world = test_world();
        let (path, mut bytes, outside) = fixture(world.path());
        let dim = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let external_path = path.parent().unwrap().join("c.2.0.mcc");
        fs::remove_file(&external_path).unwrap();
        assert!(
            format!(
                "{:#}",
                guard.rewrite_chunk_losslessly(&dim, 0, 0).unwrap_err()
            )
            .contains("external chunk is missing")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(external_path, outside).unwrap();

        let mut raw = nbt::write(&chunk(0, "\0")).unwrap();
        let string = raw
            .windows(4)
            .rposition(|part| part == [0, 2, 0xc0, 0x80])
            .unwrap();
        raw.splice(string..string + 4, [0, 1, 0]);
        let start = 2 * SECTOR_SIZE as usize;
        bytes[start..start + SECTOR_SIZE as usize].fill(0);
        bytes[start..start + 4].copy_from_slice(&((raw.len() + 1) as u32).to_be_bytes());
        bytes[start + 4] = 3;
        bytes[start + 5..start + 5 + raw.len()].copy_from_slice(&raw);
        fs::write(&path, &bytes).unwrap();
        assert!(
            format!(
                "{:#}",
                guard.rewrite_chunk_losslessly(&dim, 0, 0).unwrap_err()
            )
            .contains("reserialized without changing its bytes")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn rewrite_allocates_new_sectors_when_recompressed_chunk_grows() {
        let world = test_world();
        let (path, mut region, _) = fixture(world.path());
        let mut target = chunk(0, "target");
        let Tag::Compound(entries) = &mut target.value else {
            unreachable!()
        };
        entries.push(("random_data".into(), Tag::ByteArray(Vec::new())));
        let filler = 4091 - nbt::write(&target).unwrap().len();
        let Tag::Compound(entries) = &mut target.value else {
            unreachable!()
        };
        let mut seed = 0x913a12d5u32;
        let data = (0..filler)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        entries.last_mut().unwrap().1 = Tag::ByteArray(data);
        let raw = nbt::write(&target).unwrap();
        assert_eq!(raw.len(), 4091);
        assert!(compressed(&target).len() + 5 > SECTOR_SIZE as usize);
        let start = 2 * SECTOR_SIZE as usize;
        region[start..start + SECTOR_SIZE as usize].fill(0);
        region[start..start + 4].copy_from_slice(&((raw.len() + 1) as u32).to_be_bytes());
        region[start + 4] = 3;
        region[start + 5..start + 5 + raw.len()].copy_from_slice(&raw);
        fs::write(&path, &region).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            region
        );
        let updated = fs::read(&path).unwrap();
        assert_eq!(
            u32::from_be_bytes(updated[..4].try_into().unwrap()),
            (5 << 8) | 2
        );
        assert_eq!(&updated[4..region.len()], &region[4..]);
        assert_eq!(
            nbt::write(
                &RegionReader::open(&path)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            raw
        );
    }

    #[test]
    fn failed_replace_keeps_original_and_a_complete_backup() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let world = test_world();
        let (path, old_region, outside) = fixture(world.path());
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        let error = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap_err();
        assert!(format!("{error:#}").contains("cannot atomically replace"));
        assert_eq!(fs::read(&path).unwrap(), old_region);
        let dirs: Vec<_> = fs::read_dir(world.path().join(".minecraft-native-backups"))
            .unwrap()
            .collect();
        assert_eq!(dirs.len(), 1);
        let backup = dirs[0].as_ref().unwrap().path();
        assert_eq!(fs::read(backup.join("r.0.0.mca")).unwrap(), old_region);
        assert_eq!(fs::read(backup.join("c.2.0.mcc")).unwrap(), outside);
        drop(blocker);
    }
}
