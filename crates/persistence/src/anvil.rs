//! Read-only 26.3 world metadata and region access.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use crate::nbt::{self, NamedTag, Tag};

const SECTOR_SIZE: u64 = 4096;
const HEADER_SIZE: u64 = SECTOR_SIZE * 2;
const REGION_SIDE: i32 = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelInfo {
    pub data_version: i32,
}

pub fn read_level(world_dir: &Path) -> Result<LevelInfo> {
    Ok(read_level_document(world_dir)?.0)
}

/// Keep the complete document available to callers so unknown world data is not discarded.
pub fn read_level_document(world_dir: &Path) -> Result<(LevelInfo, NamedTag)> {
    let path = world_dir.join("level.dat");
    let file = File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    let root = nbt::read_gzip(file).with_context(|| format!("invalid {}", path.display()))?;
    let data = root
        .value
        .get("Data")
        .context("level.dat is missing Data compound")?;
    let data_version = match data.get("DataVersion") {
        Some(Tag::Int(value)) => *value,
        _ => bail!("level.dat is missing integer DataVersion"),
    };
    Ok((LevelInfo { data_version }, root))
}

pub struct RegionReader {
    file: File,
    path: PathBuf,
    region_x: i32,
    region_z: i32,
    file_length: u64,
}

impl RegionReader {
    pub fn open(path: &Path) -> Result<Self> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid region file name")?;
        let parts: Vec<_> = file_name.split('.').collect();
        ensure!(
            parts.len() == 4 && parts[0] == "r" && parts[3] == "mca",
            "expected a region file named r.X.Z.mca"
        );
        let region_x = parts[1].parse().context("invalid region X coordinate")?;
        let region_z = parts[2].parse().context("invalid region Z coordinate")?;
        Self::open_for_region(path, region_x, region_z)
    }

    /// Staged files have temporary names but retain the original region coordinates.
    pub(crate) fn open_for_region(path: &Path, region_x: i32, region_z: i32) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("cannot open region {}", path.display()))?;
        let file_length = file.metadata()?.len();
        ensure!(file_length >= HEADER_SIZE, "region header is truncated");
        Ok(Self {
            file,
            path: path.to_path_buf(),
            region_x,
            region_z,
            file_length,
        })
    }

    /// Chunk coordinates are absolute, including for negative regions.
    pub fn read_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Result<Option<NamedTag>> {
        self.read_chunk_nbt_bytes(chunk_x, chunk_z)?
            .map(|bytes| nbt::read(&bytes))
            .transpose()
    }

    pub(crate) fn read_chunk_nbt_bytes(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<Option<Vec<u8>>> {
        ensure!(
            chunk_x.div_euclid(REGION_SIDE) == self.region_x
                && chunk_z.div_euclid(REGION_SIDE) == self.region_z,
            "chunk ({chunk_x}, {chunk_z}) does not belong to this region"
        );
        let local_x = chunk_x.rem_euclid(REGION_SIDE) as u64;
        let local_z = chunk_z.rem_euclid(REGION_SIDE) as u64;
        self.file.seek(SeekFrom::Start(
            (local_x + local_z * REGION_SIDE as u64) * 4,
        ))?;
        let mut entry = [0u8; 4];
        self.file
            .read_exact(&mut entry)
            .context("cannot read region location table")?;
        let location = u32::from_be_bytes(entry);
        if location == 0 {
            return Ok(None);
        }
        let offset = u64::from(location >> 8);
        let sectors = u64::from(location & 0xff);
        ensure!(offset >= 2 && sectors > 0, "invalid chunk sector location");
        let start = offset
            .checked_mul(SECTOR_SIZE)
            .context("region offset overflow")?;
        let allocation = sectors * SECTOR_SIZE;
        ensure!(
            start
                .checked_add(allocation)
                .is_some_and(|end| end <= self.file_length),
            "chunk sectors exceed region file"
        );
        self.file.seek(SeekFrom::Start(start))?;
        let mut length_bytes = [0u8; 4];
        self.file.read_exact(&mut length_bytes)?;
        let length = u64::from(u32::from_be_bytes(length_bytes));
        ensure!(
            length >= 1 && length <= allocation - 4,
            "invalid chunk payload length"
        );
        let mut compression = [0u8; 1];
        self.file.read_exact(&mut compression)?;
        let external = compression[0] & 0x80 != 0;
        let compression = compression[0] & 0x7f;
        let payload = if external {
            ensure!(
                length == 1,
                "external chunk marker contains unexpected inline bytes"
            );
            let parent = self
                .path
                .parent()
                .context("region has no parent directory")?;
            let external_path = parent.join(format!("c.{chunk_x}.{chunk_z}.mcc"));
            let external_file = File::open(&external_path).with_context(|| {
                format!("cannot open external chunk {}", external_path.display())
            })?;
            nbt::decompress_limited(external_file)?
        } else {
            ensure!(
                length - 1 <= nbt::MAX_NBT_BYTES as u64,
                "compressed chunk exceeds size limit"
            );
            let mut bytes = vec![0u8; (length - 1) as usize];
            self.file
                .read_exact(&mut bytes)
                .context("truncated chunk payload")?;
            bytes
        };
        let bytes = nbt::decompress_payload(compression, &payload).with_context(|| {
            format!(
                "cannot decode chunk ({chunk_x}, {chunk_z}) from {}",
                self.path.display()
            )
        })?;
        Ok(Some(bytes))
    }
}

pub fn overworld_region_path(world_dir: &Path, chunk_x: i32, chunk_z: i32) -> PathBuf {
    world_dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region")
        .join(format!(
            "r.{}.{}.mca",
            chunk_x.div_euclid(REGION_SIDE),
            chunk_z.div_euclid(REGION_SIDE)
        ))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::ZlibEncoder};

    use super::*;
    use crate::nbt::{JavaString, NamedTag};

    fn chunk() -> NamedTag {
        NamedTag {
            name: JavaString::from(""),
            value: Tag::Compound(vec![
                ("xPos".into(), Tag::Int(-1)),
                ("zPos".into(), Tag::Int(-1)),
            ]),
        }
    }

    fn zlib_chunk() -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&chunk()).unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn reads_negative_region_and_absent_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.-1.-1.mca");
        let compressed = zlib_chunk();
        let mut data = vec![0u8; 3 * SECTOR_SIZE as usize];
        let table_index = 1023 * 4;
        data[table_index..table_index + 4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        let offset = HEADER_SIZE as usize;
        data[offset..offset + 4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        data[offset + 4] = 2;
        data[offset + 5..offset + 5 + compressed.len()].copy_from_slice(&compressed);
        std::fs::write(&path, data).unwrap();
        let mut reader = RegionReader::open(&path).unwrap();
        assert_eq!(reader.read_chunk(-1, -1).unwrap(), Some(chunk()));
        assert_eq!(reader.read_chunk(-2, -1).unwrap(), None);
        assert!(reader.read_chunk(0, 0).is_err());
    }

    #[test]
    fn external_chunk_and_corrupt_offsets_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.0.0.mca");
        let mut data = vec![0u8; 3 * SECTOR_SIZE as usize];
        data[0..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        let offset = HEADER_SIZE as usize;
        data[offset..offset + 4].copy_from_slice(&1u32.to_be_bytes());
        data[offset + 4] = 0x82;
        std::fs::write(&path, &data).unwrap();
        std::fs::write(dir.path().join("c.0.0.mcc"), zlib_chunk()).unwrap();
        assert_eq!(
            RegionReader::open(&path).unwrap().read_chunk(0, 0).unwrap(),
            Some(chunk())
        );
        data[0..4].copy_from_slice(&((9u32 << 8) | 1).to_be_bytes());
        std::fs::write(&path, data).unwrap();
        assert!(RegionReader::open(&path).unwrap().read_chunk(0, 0).is_err());
    }
}
