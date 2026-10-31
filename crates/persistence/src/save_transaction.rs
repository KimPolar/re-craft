//! Recoverable replacement of already validated world files.
//!
//! The caller owns the vanilla world lock and validates gameplay data. A durable
//! pending record is installed only after every original has a verified backup.
//! An interrupted transaction is rolled back rather than exposing mixed files.

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::save::hash_file;

const PENDING: &str = ".minecraft-native-pending";
const MANIFEST: &str = "transaction.json";
const COMPLETED: &str = "transaction-complete";
const MAX_MANIFEST: u64 = 4 * 1024 * 1024;

pub(crate) struct Replacement {
    pub target: PathBuf,
    pub backup: Option<PathBuf>,
    pub stage: NamedTempFile,
    pub original_hash: Option<[u8; 32]>,
}

pub(crate) enum Dependency {
    Present { target: PathBuf, backup: PathBuf },
    Absent { target: PathBuf },
    AbsentDirectory { target: PathBuf },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    files: Vec<FileRecord>,
    dependencies: Vec<DependencyRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    absent_dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    absent_directories: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRecord {
    target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original: Option<String>,
    replacement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyRecord {
    target: String,
    backup: String,
    original: String,
}

struct Prepared {
    backup_dir: PathBuf,
    manifest: Manifest,
    files: Vec<Replacement>,
}

pub(crate) fn ensure_no_pending(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root.join(PENDING)) {
        Ok(_) => Err(crate::save::InterruptedWorldSave.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("cannot inspect the world save recovery record"),
    }
}

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

/// Reject traversal, reparse points and non-regular files before opening them.
fn checked_path(
    base: &Path,
    relative: &str,
    target: bool,
    allow_absent: bool,
) -> Result<(PathBuf, bool)> {
    ensure!(
        !relative.is_empty() && relative.len() <= 4096,
        "invalid transaction path length"
    );
    let mut path = base.to_path_buf();
    let mut components = Path::new(relative).components().peekable();
    let mut first = true;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            bail!("invalid transaction path component")
        };
        if target && first {
            let name = name.to_string_lossy().to_ascii_lowercase();
            ensure!(
                name != "session.lock" && !name.starts_with(".minecraft-native-"),
                "protected transaction target"
            );
        }
        first = false;
        path.push(name);
        let metadata = match fs::symlink_metadata(&path) {
            Err(error)
                if allow_absent
                    && components.peek().is_none()
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                let parent = path
                    .parent()
                    .context("transaction target has no parent")?
                    .canonicalize()?;
                ensure!(
                    parent.starts_with(base),
                    "transaction path escapes its directory"
                );
                return Ok((path, false));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
            }
            Ok(metadata) => metadata,
        };
        ensure!(
            ordinary(&metadata),
            "transaction path contains a link or reparse point"
        );
        if components.peek().is_some() {
            ensure!(metadata.is_dir(), "transaction parent is not a directory");
        } else {
            ensure!(
                metadata.is_file(),
                "transaction file is not an ordinary file"
            );
        }
    }
    ensure!(!first, "empty transaction path");
    ensure!(
        path.canonicalize()?.starts_with(base),
        "transaction path escapes its directory"
    );
    Ok((path, true))
}

fn checked_file(base: &Path, relative: &str, target: bool) -> Result<PathBuf> {
    Ok(checked_path(base, relative, target, false)?.0)
}

fn checked_target(base: &Path, relative: &str) -> Result<(PathBuf, bool)> {
    checked_path(base, relative, true, true)
}

/// Inspect a directory without following links; only its final component may
/// be absent. Callers must still use create_dir rather than create_dir_all.
pub(crate) fn checked_directory(base: &Path, path: &Path) -> Result<(PathBuf, bool)> {
    let relative = relative(base, path)?;
    relative_path_key(&relative)?;
    let mut current = base.to_path_buf();
    let mut parts = Path::new(&relative).components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            bail!("invalid directory path")
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Err(error)
                if parts.peek().is_none() && error.kind() == std::io::ErrorKind::NotFound =>
            {
                ensure!(
                    current
                        .parent()
                        .context("directory has no parent")?
                        .canonicalize()?
                        .starts_with(base),
                    "directory escapes the world"
                );
                return Ok((current, false));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", current.display()));
            }
            Ok(metadata) => ensure!(
                metadata.is_dir() && ordinary(&metadata),
                "directory path contains a link or non-directory"
            ),
        }
    }
    ensure!(
        current.canonicalize()?.starts_with(base),
        "directory escapes the world"
    );
    Ok((current, true))
}

fn relative(base: &Path, path: &Path) -> Result<String> {
    let path = path
        .strip_prefix(base)
        .context("transaction path is outside its directory")?;
    let text = path
        .to_str()
        .context("transaction path is not valid Unicode")?;
    ensure!(!text.is_empty(), "empty transaction path");
    Ok(text.to_owned())
}

fn digest(path: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        sha2::digest::Output::<sha2::Sha256>::from(hash_file(path)?)
    ))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn relative_path_key(value: &str) -> Result<String> {
    let components: Option<Vec<_>> = Path::new(value)
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let components = components.context("invalid transaction path")?;
    ensure!(!components.is_empty(), "empty transaction path");
    let normalized = components.join(std::path::MAIN_SEPARATOR_STR);
    ensure!(
        normalized == value.replace('/', std::path::MAIN_SEPARATOR_STR),
        "non-normalized transaction path"
    );
    Ok(normalized.to_lowercase())
}

/// A corrupt recovery record must not turn rollback into deletion of an
/// arbitrary world file. Only explicitly supported region paths are allowed.
fn valid_created_region(value: &str, kind: &str) -> bool {
    let components: Option<Vec<_>> = Path::new(value)
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    let Some(parts) = components else {
        return false;
    };
    if parts.join(std::path::MAIN_SEPARATOR_STR)
        != value.replace('/', std::path::MAIN_SEPARATOR_STR)
    {
        return false;
    }
    if parts.len() < 5 || parts[0] != "dimensions" || parts[parts.len() - 2] != kind {
        return false;
    }
    let namespace = parts[1];
    let dimension_path = parts[2..parts.len() - 2].join("/");
    if crate::world::DimensionId::parse(&format!("{namespace}:{dimension_path}")).is_err() {
        return false;
    }
    let name = parts[parts.len() - 1];
    let pieces: Vec<_> = name.split('.').collect();
    if pieces.len() != 4 || pieces[0] != "r" || pieces[3] != "mca" {
        return false;
    }
    let (Ok(x), Ok(z)) = (pieces[1].parse::<i32>(), pieces[2].parse::<i32>()) else {
        return false;
    };
    name == format!("r.{x}.{z}.mca")
}

fn valid_created_entity_region(value: &str) -> bool {
    valid_created_region(value, "entities")
}

fn valid_created_poi_region(value: &str) -> bool {
    valid_created_region(value, "poi")
}

fn valid_entity_directory(value: &str) -> bool {
    valid_created_entity_region(&format!("{value}{}r.0.0.mca", std::path::MAIN_SEPARATOR))
}

fn valid_poi_directory(value: &str) -> bool {
    valid_created_poi_region(&format!("{value}{}r.0.0.mca", std::path::MAIN_SEPARATOR))
}

fn valid_created_target(value: &str, version: u32) -> bool {
    valid_created_entity_region(value)
        || (version >= 4 && valid_created_poi_region(value))
        || (version >= 5 && valid_created_weather(value))
}

fn valid_created_weather(value: &str) -> bool {
    let components: Vec<_> = Path::new(value).components().collect();
    components
        == [
            Component::Normal("data".as_ref()),
            Component::Normal("minecraft".as_ref()),
            Component::Normal("weather.dat".as_ref()),
        ]
}

fn valid_absent_directory(value: &str, version: u32) -> bool {
    valid_entity_directory(value) || (version >= 4 && valid_poi_directory(value))
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    ensure!(
        matches!(manifest.version, 1 | 2 | 3 | 4 | 5),
        "unsupported transaction version"
    );
    ensure!(
        (!manifest.files.is_empty()
            || !manifest.absent_dependencies.is_empty()
            || !manifest.absent_directories.is_empty())
            && manifest.files.len()
                + manifest.dependencies.len()
                + manifest.absent_dependencies.len()
                + manifest.absent_directories.len()
                <= 4096,
        "invalid transaction file count"
    );
    let mut targets = BTreeSet::new();
    let mut backups = BTreeSet::new();
    for record in &manifest.files {
        ensure!(
            targets.insert(relative_path_key(&record.target)?),
            "duplicate transaction target"
        );
        ensure!(
            valid_digest(&record.replacement),
            "invalid transaction hash"
        );
        match (&record.backup, &record.original) {
            (Some(backup), Some(original)) => {
                ensure!(
                    backups.insert(relative_path_key(backup)?),
                    "duplicate transaction backup"
                );
                ensure!(valid_digest(original), "invalid transaction hash");
            }
            (None, None) if manifest.version >= 2 => ensure!(
                valid_created_target(&record.target, manifest.version),
                "invalid created file path"
            ),
            _ => bail!("invalid transaction original"),
        }
    }
    for record in &manifest.dependencies {
        ensure!(
            targets.insert(relative_path_key(&record.target)?),
            "duplicate transaction target"
        );
        ensure!(
            backups.insert(relative_path_key(&record.backup)?),
            "duplicate transaction backup"
        );
        ensure!(valid_digest(&record.original), "invalid transaction hash");
    }
    ensure!(
        manifest.version >= 2 || manifest.absent_dependencies.is_empty(),
        "invalid transaction version for absent dependencies"
    );
    for target in &manifest.absent_dependencies {
        ensure!(
            valid_created_target(target, manifest.version),
            "invalid absent region path"
        );
        ensure!(
            targets.insert(relative_path_key(target)?),
            "duplicate transaction target"
        );
    }
    ensure!(
        manifest.version >= 3 || manifest.absent_directories.is_empty(),
        "invalid transaction version for absent directories"
    );
    for directory in &manifest.absent_directories {
        ensure!(
            valid_absent_directory(directory, manifest.version),
            "invalid absent region directory path"
        );
        let key = relative_path_key(directory)?;
        ensure!(targets.insert(key.clone()), "duplicate transaction target");
        let prefix = format!("{key}{}", std::path::MAIN_SEPARATOR);
        ensure!(
            !targets.iter().any(|target| target.starts_with(&prefix)),
            "absent directory overlaps a transaction target"
        );
    }
    let mut stages = BTreeSet::new();
    for record in &manifest.files {
        let Some(stage) = &record.stage else { continue };
        let stage_path = Path::new(stage);
        let target_path = Path::new(&record.target);
        ensure!(
            stage_path.parent() == target_path.parent(),
            "staged file is outside the target directory"
        );
        let name = stage_path
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid staged filename")?;
        ensure!(
            name.starts_with(".native-region-") || name.starts_with(".native-stage-"),
            "invalid staged filename"
        );
        ensure!(
            stages.insert(relative_path_key(stage)?)
                && !targets.contains(&relative_path_key(stage)?),
            "duplicate staged file"
        );
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("transaction file has no parent")?;
    let mut stage = tempfile::Builder::new()
        .prefix(".native-record-")
        .tempfile_in(parent)?;
    stage.write_all(bytes)?;
    stage.as_file().sync_all()?;
    stage.persist_noclobber(path).map_err(|error| error.error)?;
    Ok(())
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut result)?;
    ensure!(
        result.len() as u64 <= limit,
        "transaction record is too large"
    );
    Ok(result)
}

fn prepare(
    root: &Path,
    backup_dir: &Path,
    files: Vec<Replacement>,
    dependencies: Vec<Dependency>,
) -> Result<Prepared> {
    ensure_no_pending(root)?;
    let backups = root.join(".minecraft-native-backups");
    let backup_root = backups
        .canonicalize()
        .context("backup directory is missing")?;
    ensure!(
        backup_root.parent() == Some(root),
        "backup directory is outside the world"
    );
    let meta = fs::symlink_metadata(&backups)?;
    ensure!(
        meta.is_dir() && ordinary(&meta),
        "backup directory is not ordinary"
    );
    let backup_meta = fs::symlink_metadata(backup_dir)?;
    ensure!(
        backup_meta.is_dir() && ordinary(&backup_meta),
        "transaction backup is not an ordinary directory"
    );
    let backup_dir = backup_dir
        .canonicalize()
        .context("transaction backup is missing")?;
    ensure!(
        backup_dir.parent() == Some(backup_root.as_path()),
        "transaction backup is outside the backup directory"
    );
    let mut records = Vec::with_capacity(files.len());
    for file in &files {
        let target = relative(root, &file.target)?;
        let (path, exists) = checked_target(root, &target)?;
        let (backup, original) = match (&file.backup, file.original_hash) {
            (Some(backup), Some(hash)) => {
                let backup = relative(&backup_dir, backup)?;
                let original = format!("{:x}", sha2::digest::Output::<sha2::Sha256>::from(hash));
                ensure!(
                    exists && digest(&path)? == original,
                    "world file changed during staging"
                );
                ensure!(
                    digest(&checked_file(&backup_dir, &backup, false)?)? == original,
                    "world backup does not match the original"
                );
                (Some(backup), Some(original))
            }
            (None, None) => {
                ensure!(!exists, "world file appeared during staging");
                (None, None)
            }
            _ => bail!("transaction original and backup must both be present or absent"),
        };
        ensure!(
            file.stage.path().parent() == file.target.parent(),
            "staged file is not in the target directory"
        );
        file.stage.as_file().sync_all()?;
        records.push(FileRecord {
            target,
            backup,
            original,
            replacement: digest(file.stage.path())?,
            stage: Some(relative(root, file.stage.path())?),
        });
    }
    let mut dependency_records = Vec::with_capacity(dependencies.len());
    let mut absent_dependencies = Vec::new();
    let mut absent_directories = Vec::new();
    for dependency in &dependencies {
        match dependency {
            Dependency::Present { target, backup } => {
                let target = relative(root, target)?;
                let backup = relative(&backup_dir, backup)?;
                let original = digest(&checked_file(&backup_dir, &backup, false)?)?;
                ensure!(
                    digest(&checked_file(root, &target, true)?)? == original,
                    "world dependency changed during staging"
                );
                dependency_records.push(DependencyRecord {
                    target,
                    backup,
                    original,
                });
            }
            Dependency::Absent { target } => {
                let target = relative(root, target)?;
                ensure!(
                    valid_created_target(&target, 4),
                    "invalid absent region path"
                );
                ensure!(
                    !checked_target(root, &target)?.1,
                    "absent world dependency appeared during staging"
                );
                absent_dependencies.push(target);
            }
            Dependency::AbsentDirectory { target } => {
                let target_relative = relative(root, target)?;
                ensure!(
                    valid_absent_directory(&target_relative, 4),
                    "invalid absent region directory path"
                );
                ensure!(
                    !checked_directory(root, target)?.1,
                    "absent entity directory appeared during staging"
                );
                absent_directories.push(target_relative);
            }
        }
    }
    let manifest = Manifest {
        version: if records
            .iter()
            .any(|record| record.original.is_none() && valid_created_weather(&record.target))
        {
            5
        } else if records
            .iter()
            .any(|record| record.original.is_none() && valid_created_poi_region(&record.target))
            || absent_dependencies
                .iter()
                .any(|target| valid_created_poi_region(target))
            || absent_directories
                .iter()
                .any(|target| valid_poi_directory(target))
        {
            4
        } else if !absent_directories.is_empty() {
            3
        } else if !absent_dependencies.is_empty()
            || records.iter().any(|record| record.original.is_none())
        {
            2
        } else {
            1
        },
        files: records,
        dependencies: dependency_records,
        absent_dependencies,
        absent_directories,
    };
    validate_manifest(&manifest)?;
    let bytes = serde_json::to_vec(&manifest)?;
    ensure!(
        bytes.len() as u64 <= MAX_MANIFEST,
        "transaction record is too large"
    );
    write_new(&backup_dir.join(MANIFEST), &bytes)?;
    let name = backup_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("invalid backup directory name")?;
    ensure!(valid_backup_name(name), "invalid transaction backup name");
    write_new(&root.join(PENDING), name.as_bytes())?;
    Ok(Prepared {
        backup_dir,
        manifest,
        files,
    })
}

fn valid_backup_name(name: &str) -> bool {
    (name.starts_with("region-") || name.starts_with("transaction-"))
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn check_dependencies(root: &Path, manifest: &Manifest) -> Result<()> {
    for dependency in &manifest.dependencies {
        let path = checked_file(root, &dependency.target, true)?;
        ensure!(
            digest(&path)? == dependency.original,
            "world dependency changed: {}",
            path.display()
        );
    }
    for relative in &manifest.absent_dependencies {
        let (path, exists) = checked_target(root, relative)?;
        ensure!(
            !exists,
            "absent world dependency appeared: {}",
            path.display()
        );
    }
    for relative in &manifest.absent_directories {
        let (path, exists) = checked_directory(root, &root.join(relative))?;
        ensure!(
            !exists,
            "absent entity directory appeared: {}",
            path.display()
        );
    }
    Ok(())
}

fn replace_one(root: &Path, record: &FileRecord, file: Replacement) -> Result<()> {
    let (target, exists) = checked_target(root, &record.target)?;
    match &record.original {
        Some(original) => ensure!(
            exists && digest(&target)? == *original,
            "world file changed before replacement: {}",
            target.display()
        ),
        None => ensure!(
            !exists,
            "world file appeared before creation: {}",
            target.display()
        ),
    }
    ensure!(
        digest(file.stage.path())? == record.replacement,
        "staged world file changed"
    );
    if record.original.is_some() {
        file.stage.persist(&target).map_err(|error| error.error)
    } else {
        file.stage
            .persist_noclobber(&target)
            .map_err(|error| error.error)
    }
    .with_context(|| format!("cannot atomically install {}", target.display()))?;
    Ok(())
}

pub(crate) fn commit(
    root: &Path,
    backup_dir: &Path,
    files: Vec<Replacement>,
    dependencies: Vec<Dependency>,
) -> Result<()> {
    let prepared = prepare(root, backup_dir, files, dependencies)?;
    let result = (|| -> Result<()> {
        check_dependencies(root, &prepared.manifest)?;
        for (record, file) in prepared.manifest.files.iter().zip(prepared.files) {
            replace_one(root, record, file)?;
        }
        for record in &prepared.manifest.files {
            let target = checked_file(root, &record.target, true)?;
            ensure!(
                digest(&target)? == record.replacement,
                "replaced world file changed"
            );
        }
        check_dependencies(root, &prepared.manifest)?;
        write_new(&prepared.backup_dir.join(COMPLETED), b"committed\n")?;
        fs::remove_file(root.join(PENDING)).context("cannot clear completed transaction record")?;
        Ok(())
    })();
    if let Err(error) = result {
        let committed = fs::read(prepared.backup_dir.join(COMPLETED))
            .is_ok_and(|content| content == b"committed\n");
        return match recover(root) {
            Ok(_) if committed => Ok(()),
            Ok(_) => Err(error),
            Err(recovery) => {
                let status = if committed {
                    "the transaction was committed; cleanup stopped"
                } else {
                    "automatic recovery stopped"
                };
                Err(error.context(format!(
                    "{status}: {recovery:#}; backup retained at {}",
                    prepared.backup_dir.display()
                )))
            }
        };
    }
    Ok(())
}

/// Idempotently restore every original, provided no target was changed by an
/// outside writer. A partial restore is safe to retry after another interruption.
pub(crate) fn recover(root: &Path) -> Result<bool> {
    let marker = root.join(PENDING);
    match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(meta) => ensure!(
            meta.is_file() && ordinary(&meta),
            "invalid pending transaction record"
        ),
    }
    let name = String::from_utf8(read_limited(&marker, 128)?)?;
    ensure!(
        valid_backup_name(&name),
        "invalid pending backup directory name"
    );
    let backups = root.join(".minecraft-native-backups");
    let backup_meta =
        fs::symlink_metadata(&backups).context("recovery backup directory is unavailable")?;
    ensure!(
        backup_meta.is_dir() && ordinary(&backup_meta),
        "backup directory is not ordinary"
    );
    let backups = backups.canonicalize()?;
    ensure!(
        backups.parent() == Some(root),
        "backup directory is outside the world"
    );
    let backup_dir = backups.join(name);
    let metadata = fs::symlink_metadata(&backup_dir).context("recovery backup is unavailable")?;
    ensure!(
        metadata.is_dir() && ordinary(&metadata),
        "transaction backup is not an ordinary directory"
    );
    let backup_dir = backup_dir.canonicalize()?;
    ensure!(
        backup_dir.parent() == Some(backups.as_path()),
        "transaction backup is outside the backup directory"
    );
    let manifest_path = checked_file(&backup_dir, MANIFEST, false)?;
    let manifest: Manifest = serde_json::from_slice(&read_limited(&manifest_path, MAX_MANIFEST)?)
        .context("invalid transaction manifest")?;
    validate_manifest(&manifest)?;
    let finished = backup_dir.join(COMPLETED);
    match fs::symlink_metadata(&finished) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && ordinary(&meta),
                "invalid completed transaction record"
            );
            let content = read_limited(&finished, 32)?;
            ensure!(
                content == b"committed\n" || content == b"rolled_back\n",
                "invalid completed transaction record"
            );
            fs::remove_file(&marker)?;
            return Ok(true);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // Check everything before replacing anything, including unchanged external
    // chunk dependencies. Never overwrite a file altered by another writer.
    check_dependencies(root, &manifest)?;
    for record in &manifest.dependencies {
        ensure!(
            digest(&checked_file(&backup_dir, &record.backup, false)?)? == record.original,
            "dependency backup changed"
        );
    }
    for record in &manifest.files {
        let (target, exists) = checked_target(root, &record.target)?;
        match (&record.original, &record.backup) {
            (Some(original), Some(backup)) => {
                ensure!(
                    exists,
                    "world file was changed outside the transaction: {}",
                    target.display()
                );
                let actual = digest(&target)?;
                ensure!(
                    actual == *original || actual == record.replacement,
                    "world file was changed outside the transaction: {}",
                    target.display()
                );
                ensure!(
                    digest(&checked_file(&backup_dir, backup, false)?)? == *original,
                    "world backup changed"
                );
            }
            (None, None) => ensure!(
                !exists || digest(&target)? == record.replacement,
                "world file was changed outside the transaction: {}",
                target.display()
            ),
            _ => bail!("invalid transaction original"),
        }
    }
    for record in &manifest.files {
        let (target, exists) = checked_target(root, &record.target)?;
        let Some(original) = &record.original else {
            if exists {
                ensure!(
                    digest(&target)? == record.replacement,
                    "world file changed during recovery"
                );
                fs::remove_file(&target).context("cannot remove created transaction file")?;
            }
            continue;
        };
        ensure!(exists, "world file changed during recovery");
        let actual = digest(&target)?;
        if actual == *original {
            continue;
        }
        ensure!(
            actual == record.replacement,
            "world file changed during recovery"
        );
        let backup = checked_file(
            &backup_dir,
            record
                .backup
                .as_deref()
                .context("missing transaction backup")?,
            false,
        )?;
        let mut stage = tempfile::Builder::new()
            .prefix(".native-restore-")
            .tempfile_in(target.parent().context("target has no parent")?)?;
        std::io::copy(&mut File::open(backup)?, stage.as_file_mut())?;
        stage.as_file().sync_all()?;
        ensure!(
            digest(stage.path())? == *original,
            "restored backup failed verification"
        );
        stage
            .persist(&target)
            .map_err(|error| error.error)
            .context("cannot restore transaction backup")?;
    }
    for record in &manifest.files {
        let Some(relative) = &record.stage else {
            continue;
        };
        let path = root.join(relative);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(meta) if !meta.is_file() || !ordinary(&meta) => continue,
            Ok(_) => {}
        }
        let stage = checked_file(root, relative, true)?;
        // A modified temporary file may belong to somebody else; keep it.
        if digest(&stage)? == record.replacement {
            fs::remove_file(stage).context("cannot remove a recovered staging file")?;
        }
    }
    check_dependencies(root, &manifest)?;
    for record in &manifest.files {
        let (target, exists) = checked_target(root, &record.target)?;
        let unchanged = match &record.original {
            Some(original) => exists && digest(&target)? == *original,
            None => !exists,
        };
        ensure!(
            unchanged,
            "world file changed during recovery: {}",
            target.display()
        );
    }
    write_new(&finished, b"rolled_back\n")?;
    fs::remove_file(&marker)?;
    Ok(true)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::{
        save::WorldWriteGuard,
        world::{DimensionId, WorldReader},
    };

    const OLD_REGION: &[u8] = b"original region data";
    const NEW_REGION: &[u8] = b"replacement region data";
    const OLD_PLAYER: &[u8] = b"original player data";
    const NEW_PLAYER: &[u8] = b"replacement player data";
    const EXTERNAL: &[u8] = b"external chunk data";

    fn fixture() -> tempfile::TempDir {
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/transaction-test-temp");
        fs::create_dir_all(&parent).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(parent)
            .unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("dimensions/minecraft/overworld/region")).unwrap();
        fs::create_dir_all(root.join("dimensions/minecraft/overworld/entities")).unwrap();
        fs::create_dir_all(root.join("players/data")).unwrap();
        fs::create_dir_all(root.join(".minecraft-native-backups/transaction-fixture")).unwrap();
        fs::write(
            root.join("dimensions/minecraft/overworld/region/r.0.0.mca"),
            OLD_REGION,
        )
        .unwrap();
        fs::write(
            root.join("dimensions/minecraft/overworld/region/c.2.0.mcc"),
            EXTERNAL,
        )
        .unwrap();
        fs::write(root.join("players/data/player.dat"), OLD_PLAYER).unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        fs::write(backup.join("region-original"), OLD_REGION).unwrap();
        fs::write(backup.join("player-original"), OLD_PLAYER).unwrap();
        fs::write(backup.join("external-original"), EXTERNAL).unwrap();
        dir
    }

    fn replacements(root: &Path) -> (Vec<Replacement>, Vec<Dependency>) {
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let values = [
            (
                "dimensions/minecraft/overworld/region/r.0.0.mca",
                "region-original",
                NEW_REGION,
            ),
            ("players/data/player.dat", "player-original", NEW_PLAYER),
        ];
        let files = values
            .into_iter()
            .map(|(target, backup_name, value)| {
                let target = root.join(target);
                let original_hash = hash_file(&target).unwrap();
                let mut stage = tempfile::Builder::new()
                    .prefix(".native-stage-")
                    .tempfile_in(target.parent().unwrap())
                    .unwrap();
                stage.write_all(value).unwrap();
                Replacement {
                    target,
                    backup: Some(backup.join(backup_name)),
                    stage,
                    original_hash: Some(original_hash),
                }
            })
            .collect();
        let deps = vec![Dependency::Present {
            target: root.join("dimensions/minecraft/overworld/region/c.2.0.mcc"),
            backup: backup.join("external-original"),
        }];
        (files, deps)
    }

    fn start(root: &Path) -> Prepared {
        let (files, dependencies) = replacements(root);
        prepare(
            root,
            &root.join(".minecraft-native-backups/transaction-fixture"),
            files,
            dependencies,
        )
        .unwrap()
    }

    fn created_region(root: &Path) -> Replacement {
        let target = root.join("dimensions/minecraft/overworld/entities/r.1.0.mca");
        let mut stage = tempfile::Builder::new()
            .prefix(".native-region-")
            .tempfile_in(target.parent().unwrap())
            .unwrap();
        stage.write_all(b"new region").unwrap();
        Replacement {
            target,
            backup: None,
            original_hash: None,
            stage,
        }
    }

    fn created_poi_region(root: &Path) -> Replacement {
        let target = root.join("dimensions/minecraft/overworld/poi/r.-1.0.mca");
        let mut stage = tempfile::Builder::new()
            .prefix(".native-region-")
            .tempfile_in(target.parent().unwrap())
            .unwrap();
        stage.write_all(b"new poi region").unwrap();
        Replacement {
            target,
            backup: None,
            original_hash: None,
            stage,
        }
    }

    #[test]
    fn new_file_commits_without_overwriting_an_existing_target() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let new = created_region(&root);
        let path = new.target.clone();
        commit(&root, &backup, vec![new], vec![]).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"new region");
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(backup.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest.version, 2);
        assert!(manifest.files[0].original.is_none());
    }

    #[test]
    fn new_file_creation_never_clobbers_an_outside_writer() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let new = created_region(&root);
        let path = new.target.clone();
        let mut prepared = prepare(&root, &backup, vec![new], vec![]).unwrap();
        fs::write(&path, b"outside writer").unwrap();
        assert!(replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).is_err());
        drop(prepared);
        assert!(recover(&root).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"outside writer");
        fs::remove_file(&path).unwrap();
        assert!(recover(&root).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn created_weather_recovery_is_versioned_and_refuses_unrelated_paths() {
        let weather = "data/minecraft/weather.dat";
        assert!(!valid_created_target(weather, 4));
        assert!(valid_created_target(weather, 5));
        for invalid in [
            "data/minecraft/game_rules.dat",
            "data/minecraft/world_clocks.dat",
            "data/example/weather.dat",
            "players/data/weather.dat",
            "data/minecraft/../weather.dat",
        ] {
            assert!(!valid_created_target(invalid, 5), "{invalid}");
        }
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let target = root.join(weather);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut stage = tempfile::Builder::new()
            .prefix(".native-stage-")
            .tempfile_in(target.parent().unwrap())
            .unwrap();
        stage.write_all(b"new weather").unwrap();
        let file = Replacement {
            target: target.clone(),
            backup: None,
            stage,
            original_hash: None,
        };
        let mut prepared = prepare(&root, &backup, vec![file], vec![]).unwrap();
        assert_eq!(prepared.manifest.version, 5);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        drop(prepared);
        fs::write(&target, b"outside writer").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"outside writer");
        fs::write(&target, b"new weather").unwrap();
        assert!(recover(&root).unwrap());
        assert!(!target.exists());
    }

    #[test]
    fn recovery_records_only_allow_new_entity_region_paths() {
        assert!(valid_created_entity_region(
            "dimensions/minecraft/overworld/entities/r.-1.0.mca"
        ));
        assert!(valid_created_entity_region(
            "dimensions/example/deep/world/entities/r.5.-9.mca"
        ));
        for invalid in [
            "players/data/player.dat",
            "dimensions/minecraft/overworld/region/r.0.0.mca",
            "dimensions/minecraft/overworld/entities/r.01.0.mca",
            "dimensions/minecraft/../entities/r.0.0.mca",
            "dimensions/minecraft/./overworld/entities/r.0.0.mca",
            "dimensions/minecraft/overworld/entities/session.lock",
        ] {
            assert!(!valid_created_entity_region(invalid), "{invalid}");
        }
        assert!(relative_path_key("dimensions/minecraft/./overworld/entities/r.0.0.mca").is_err());
        assert_eq!(
            relative_path_key("dimensions/minecraft/overworld/entities/r.0.0.mca").unwrap(),
            relative_path_key("dimensions\\minecraft\\overworld\\entities\\r.0.0.mca").unwrap()
        );
    }

    #[test]
    fn poi_creation_paths_require_new_manifest_and_stay_inside_dimension() {
        let poi = "dimensions/minecraft/overworld/poi/r.-1.0.mca";
        assert!(valid_created_poi_region(poi));
        assert!(valid_created_target(poi, 4));
        assert!(!valid_created_target(poi, 3));
        assert!(valid_poi_directory("dimensions/example/deep/world/poi"));
        assert!(!valid_absent_directory(
            "dimensions/example/deep/world/poi",
            3
        ));
        for invalid in [
            "players/data/r.0.0.mca",
            "dimensions/minecraft/overworld/region/r.0.0.mca",
            "dimensions/minecraft/overworld/poi/r.01.0.mca",
            "dimensions/minecraft/../poi/r.0.0.mca",
            "dimensions/minecraft/overworld/poi/session.lock",
        ] {
            assert!(!valid_created_target(invalid, 4), "{invalid}");
        }
        let mut manifest = Manifest {
            version: 3,
            files: vec![FileRecord {
                target: poi.into(),
                backup: None,
                original: None,
                replacement: "0".repeat(64),
                stage: None,
            }],
            dependencies: vec![],
            absent_dependencies: vec![],
            absent_directories: vec![],
        };
        assert!(validate_manifest(&manifest).is_err());
        manifest.version = 4;
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn absent_poi_directory_is_checked_before_recovery() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let (files, mut dependencies) = replacements(&root);
        let missing = root.join("dimensions/minecraft/overworld/poi");
        dependencies.push(Dependency::AbsentDirectory {
            target: missing.clone(),
        });
        let mut prepared = prepare(&root, &backup, files, dependencies).unwrap();
        assert_eq!(prepared.manifest.version, 4);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        drop(prepared);
        fs::create_dir(&missing).unwrap();
        assert!(recover(&root).is_err());
        fs::remove_dir(&missing).unwrap();
        assert!(recover(&root).unwrap());
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            OLD_REGION
        );
    }

    #[test]
    fn new_poi_region_crash_helper() {
        let Some(root) = std::env::var_os("NATIVE_POI_REGION_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let (mut files, deps) = replacements(&root);
        files.push(created_poi_region(&root));
        let mut prepared = prepare(
            &root,
            &root.join(".minecraft-native-backups/transaction-fixture"),
            files,
            deps,
        )
        .unwrap();
        assert_eq!(prepared.manifest.version, 4);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        replace_one(&root, &prepared.manifest.files[2], prepared.files.remove(1)).unwrap();
        std::process::exit(24);
    }

    #[test]
    fn crash_recovery_restores_terrain_and_removes_created_poi_region() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join("dimensions/minecraft/overworld/poi")).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_transaction::tests::new_poi_region_crash_helper",
                "--nocapture",
            ])
            .env("NATIVE_POI_REGION_CRASH_FIXTURE", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(24),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let poi = root.join("dimensions/minecraft/overworld/poi/r.-1.0.mca");
        let terrain = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        assert_eq!(fs::read(&poi).unwrap(), b"new poi region");
        assert_eq!(fs::read(&terrain).unwrap(), NEW_REGION);
        fs::write(&poi, b"outside writer").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(fs::read(&terrain).unwrap(), NEW_REGION);
        fs::write(&poi, b"new poi region").unwrap();
        assert!(recover(&root).unwrap());
        assert!(!poi.exists());
        assert_eq!(fs::read(terrain).unwrap(), OLD_REGION);
    }

    #[test]
    fn absent_region_dependency_is_checked_at_commit_and_recovery() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let missing = root.join("dimensions/minecraft/overworld/entities/r.1.0.mca");
        let (files, mut deps) = replacements(&root);
        deps.push(Dependency::Absent {
            target: missing.clone(),
        });
        let mut prepared = prepare(&root, &backup, files, deps).unwrap();
        assert_eq!(prepared.manifest.version, 2);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        drop(prepared);
        fs::write(&missing, b"created by another writer").unwrap();
        assert!(format!("{:#}", recover(&root).unwrap_err()).contains("dependency appeared"));
        assert_eq!(fs::read(&missing).unwrap(), b"created by another writer");
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        fs::remove_file(&missing).unwrap();
        assert!(recover(&root).unwrap());
        assert_eq!(fs::read(region).unwrap(), OLD_REGION);
        assert!(!missing.exists());
    }

    #[test]
    fn absent_region_dependency_is_not_allowed_to_alias_a_replacement() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        let new = created_region(&root);
        let path = new.target.clone();
        assert!(
            prepare(
                &root,
                &backup,
                vec![new],
                vec![Dependency::Absent {
                    target: path.clone()
                }]
            )
            .is_err()
        );
        assert!(!path.exists());
        assert!(ensure_no_pending(&root).is_ok());
    }

    #[test]
    fn absent_dependency_crash_helper() {
        let Some(root) = std::env::var_os("NATIVE_ABSENT_DEPENDENCY_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let (files, mut deps) = replacements(&root);
        deps.push(Dependency::Absent {
            target: root.join("dimensions/minecraft/overworld/entities/r.1.0.mca"),
        });
        let mut prepared = prepare(
            &root,
            &root.join(".minecraft-native-backups/transaction-fixture"),
            files,
            deps,
        )
        .unwrap();
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        std::process::exit(25);
    }

    #[test]
    fn interrupted_save_rechecks_absent_region_before_rollback() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_transaction::tests::absent_dependency_crash_helper",
                "--nocapture",
            ])
            .env("NATIVE_ABSENT_DEPENDENCY_CRASH_FIXTURE", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(25),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let missing = root.join("dimensions/minecraft/overworld/entities/r.1.0.mca");
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        fs::write(&missing, b"outside").unwrap();
        assert!(WorldWriteGuard::recover_interrupted(&root).is_err());
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        assert_eq!(fs::read(&missing).unwrap(), b"outside");
        fs::remove_file(missing).unwrap();
        assert!(WorldWriteGuard::recover_interrupted(&root).unwrap());
        assert_eq!(fs::read(region).unwrap(), OLD_REGION);
    }

    #[test]
    fn absent_entity_directory_crash_helper() {
        let Some(root) = std::env::var_os("NATIVE_ABSENT_DIRECTORY_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let (files, mut deps) = replacements(&root);
        deps.push(Dependency::AbsentDirectory {
            target: root.join("dimensions/minecraft/overworld/entities"),
        });
        let mut prepared = prepare(
            &root,
            &root.join(".minecraft-native-backups/transaction-fixture"),
            files,
            deps,
        )
        .unwrap();
        assert_eq!(prepared.manifest.version, 3);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        std::process::exit(26);
    }

    #[test]
    fn recovery_detects_an_entity_directory_created_after_interruption() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let absent = root.join("dimensions/minecraft/overworld/entities");
        fs::remove_dir(&absent).unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_transaction::tests::absent_entity_directory_crash_helper",
                "--nocapture",
            ])
            .env("NATIVE_ABSENT_DIRECTORY_CRASH_FIXTURE", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(26),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        fs::create_dir(&absent).unwrap();
        fs::write(absent.join("outside.dat"), b"outside").unwrap();
        assert!(WorldWriteGuard::recover_interrupted(&root).is_err());
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        assert_eq!(fs::read(absent.join("outside.dat")).unwrap(), b"outside");
        fs::remove_file(absent.join("outside.dat")).unwrap();
        fs::remove_dir(&absent).unwrap();
        assert!(WorldWriteGuard::recover_interrupted(&root).unwrap());
        assert_eq!(fs::read(region).unwrap(), OLD_REGION);
    }

    #[test]
    fn new_region_crash_helper() {
        let Some(root) = std::env::var_os("NATIVE_NEW_REGION_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let (mut files, deps) = replacements(&root);
        files.push(created_region(&root));
        let mut prepared = prepare(
            &root,
            &root.join(".minecraft-native-backups/transaction-fixture"),
            files,
            deps,
        )
        .unwrap();
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        replace_one(&root, &prepared.manifest.files[2], prepared.files.remove(1)).unwrap();
        std::process::exit(24);
    }

    #[test]
    fn crash_recovery_removes_new_region_and_restores_existing_files() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_transaction::tests::new_region_crash_helper",
                "--nocapture",
            ])
            .env("NATIVE_NEW_REGION_CRASH_FIXTURE", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(24),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let new = root.join("dimensions/minecraft/overworld/entities/r.1.0.mca");
        let existing = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        assert_eq!(fs::read(&new).unwrap(), b"new region");
        assert_eq!(fs::read(&existing).unwrap(), NEW_REGION);
        fs::write(&new, b"changed elsewhere").unwrap();
        assert!(recover(&root).is_err());
        assert_eq!(fs::read(&existing).unwrap(), NEW_REGION);
        assert_eq!(fs::read(&new).unwrap(), b"changed elsewhere");
        fs::write(&new, b"new region").unwrap();
        assert!(WorldWriteGuard::recover_interrupted(&root).unwrap());
        assert!(!new.exists());
        assert_eq!(fs::read(existing).unwrap(), OLD_REGION);
        assert_eq!(
            fs::read(root.join("players/data/player.dat")).unwrap(),
            OLD_PLAYER
        );
        assert!(!WorldWriteGuard::recover_interrupted(&root).unwrap());
    }

    #[test]
    fn commits_multiple_files_and_keeps_originals() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let (files, dependencies) = replacements(&root);
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        commit(&root, &backup, files, dependencies).unwrap();
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            NEW_REGION
        );
        assert_eq!(
            fs::read(root.join("players/data/player.dat")).unwrap(),
            NEW_PLAYER
        );
        assert_eq!(
            fs::read(backup.join("region-original")).unwrap(),
            OLD_REGION
        );
        assert_eq!(
            fs::read(backup.join("player-original")).unwrap(),
            OLD_PLAYER
        );
        assert_eq!(
            fs::read(backup.join("external-original")).unwrap(),
            EXTERNAL
        );
        assert!(
            !fs::read_dir(root.join("players/data"))
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".native-stage-"))
        );
        assert_eq!(fs::read(backup.join(COMPLETED)).unwrap(), b"committed\n");
        assert!(!recover(&root).unwrap());
    }

    #[test]
    fn crash_helper() {
        let Some(root) = std::env::var_os("NATIVE_TRANSACTION_CRASH_FIXTURE") else {
            return;
        };
        let root = PathBuf::from(root).canonicalize().unwrap();
        let mut prepared = start(&root);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        // Simulate abrupt process termination without running destructors.
        std::process::exit(23);
    }

    #[test]
    fn process_interruption_blocks_world_open_and_restores_all_originals() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save_transaction::tests::crash_helper",
                "--nocapture",
            ])
            .env("NATIVE_TRANSACTION_CRASH_FIXTURE", &root)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            NEW_REGION
        );
        assert_eq!(
            fs::read(root.join("players/data/player.dat")).unwrap(),
            OLD_PLAYER
        );
        let error = WorldReader::open(&root, DimensionId::parse("overworld").unwrap())
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("needs recovery"));
        assert!(WorldWriteGuard::acquire(&root).is_err());
        assert!(WorldWriteGuard::recover_interrupted(&root).unwrap());
        assert!(!WorldWriteGuard::recover_interrupted(&root).unwrap());
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            OLD_REGION
        );
        assert_eq!(
            fs::read(root.join("players/data/player.dat")).unwrap(),
            OLD_PLAYER
        );
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        assert_eq!(fs::read(backup.join(COMPLETED)).unwrap(), b"rolled_back\n");
        assert_eq!(
            fs::read(backup.join("external-original")).unwrap(),
            EXTERNAL
        );
        assert!(
            !fs::read_dir(root.join("players/data"))
                .unwrap()
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".native-stage-"))
        );
    }

    #[test]
    fn outside_changes_or_damaged_backups_stop_recovery_without_overwrite() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let mut prepared = start(&root);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        drop(prepared);
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        let player = root.join("players/data/player.dat");
        fs::write(&player, b"changed elsewhere").unwrap();
        assert!(format!("{:#}", recover(&root).unwrap_err()).contains("outside the transaction"));
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        assert_eq!(fs::read(&player).unwrap(), b"changed elsewhere");
        fs::write(&player, OLD_PLAYER).unwrap();
        let backup = root.join(".minecraft-native-backups/transaction-fixture/region-original");
        fs::write(&backup, b"damaged backup").unwrap();
        assert!(format!("{:#}", recover(&root).unwrap_err()).contains("backup changed"));
        assert_eq!(fs::read(&region).unwrap(), NEW_REGION);
        fs::write(&backup, OLD_REGION).unwrap();
        assert!(recover(&root).unwrap());
        assert_eq!(fs::read(&region).unwrap(), OLD_REGION);
    }

    #[test]
    fn completed_record_survives_interruption_before_marker_cleanup() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let prepared = start(&root);
        for (record, file) in prepared.manifest.files.iter().zip(prepared.files) {
            replace_one(&root, record, file).unwrap();
        }
        write_new(&prepared.backup_dir.join(COMPLETED), b"committed\n").unwrap();
        assert!(recover(&root).unwrap());
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            NEW_REGION
        );
        assert_eq!(
            fs::read(root.join("players/data/player.dat")).unwrap(),
            NEW_PLAYER
        );
    }

    #[test]
    fn interrupted_rollback_can_be_retried_after_one_file_was_restored() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let prepared = start(&root);
        for (record, file) in prepared.manifest.files.iter().zip(prepared.files) {
            replace_one(&root, record, file).unwrap();
        }
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        let player = root.join("players/data/player.dat");
        // One rollback replacement completed before the process was stopped.
        fs::write(&region, OLD_REGION).unwrap();
        assert_eq!(fs::read(&player).unwrap(), NEW_PLAYER);
        assert!(recover(&root).unwrap());
        assert_eq!(fs::read(&region).unwrap(), OLD_REGION);
        assert_eq!(fs::read(&player).unwrap(), OLD_PLAYER);
    }

    #[test]
    fn failed_second_replace_restores_first_and_keeps_backups() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        let player = root.join("players/data/player.dat");
        let blocker = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&player)
            .unwrap();
        let (files, dependencies) = replacements(&root);
        let backup = root.join(".minecraft-native-backups/transaction-fixture");
        assert!(commit(&root, &backup, files, dependencies).is_err());
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            OLD_REGION
        );
        assert_eq!(fs::read(&player).unwrap(), OLD_PLAYER);
        assert_eq!(
            fs::read(backup.join("region-original")).unwrap(),
            OLD_REGION
        );
        assert!(ensure_no_pending(&root).is_ok());
        drop(blocker);
    }

    #[test]
    fn rejects_escaped_targets_and_changed_external_dependency() {
        let dir = fixture();
        let root = dir.path().canonicalize().unwrap();
        assert!(checked_file(&root, "../escape", true).is_err());
        assert!(checked_file(&root, "session.lock", true).is_err());
        let mut prepared = start(&root);
        replace_one(&root, &prepared.manifest.files[0], prepared.files.remove(0)).unwrap();
        drop(prepared);
        fs::write(
            root.join("dimensions/minecraft/overworld/region/c.2.0.mcc"),
            b"external changed",
        )
        .unwrap();
        assert!(format!("{:#}", recover(&root).unwrap_err()).contains("dependency changed"));
        assert_eq!(
            fs::read(root.join("dimensions/minecraft/overworld/region/r.0.0.mca")).unwrap(),
            NEW_REGION
        );
    }
}
