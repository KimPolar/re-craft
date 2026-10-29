//! Read verified assets from the local Minecraft launcher index.

use crate::TARGET_VERSION;
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{fs, path::Path};

pub struct AssetIndex {
    objects: serde_json::Map<String, Value>,
}

impl AssetIndex {
    pub fn load(root: &Path) -> Result<Self> {
        let metadata_path = root
            .join("versions")
            .join(TARGET_VERSION)
            .join(format!("{TARGET_VERSION}.json"));
        let metadata: Value = serde_json::from_slice(&fs::read(&metadata_path)?)?;
        let index = metadata
            .get("assetIndex")
            .context("Minecraft 26.3 has no asset index")?;
        let id = index
            .get("id")
            .and_then(Value::as_str)
            .context("Missing asset index ID")?;
        ensure!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            "Invalid asset index ID"
        );
        let hash = index
            .get("sha1")
            .and_then(Value::as_str)
            .context("Missing asset index checksum")?;
        let path = root
            .join("assets")
            .join("indexes")
            .join(format!("{id}.json"));
        let bytes = fs::read(&path).with_context(|| {
            format!(
                "Missing asset index {}; run Minecraft 26.3 once in the official launcher",
                path.display()
            )
        })?;
        verify_hash(&bytes, hash, "asset index")?;
        let mut value: Value =
            serde_json::from_slice(&bytes).context("Invalid Minecraft asset index")?;
        let objects = value
            .get_mut("objects")
            .and_then(Value::as_object_mut)
            .context("Asset index has no objects")?;
        Ok(Self {
            objects: std::mem::take(objects),
        })
    }

    pub fn read(&self, root: &Path, name: &str) -> Result<Vec<u8>> {
        let object = self
            .objects
            .get(name)
            .with_context(|| format!("Asset index does not contain {name}"))?;
        let hash = object
            .get("hash")
            .and_then(Value::as_str)
            .context("Asset has no checksum")?;
        ensure!(
            hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "Invalid Minecraft asset checksum"
        );
        let size = object
            .get("size")
            .and_then(Value::as_u64)
            .context("Asset has no size")?;
        ensure!(size <= 16 * 1024 * 1024, "Minecraft asset is too large");
        let path = root
            .join("assets")
            .join("objects")
            .join(&hash[..2])
            .join(hash);
        let bytes = fs::read(&path).with_context(|| {
            format!(
                "Missing Minecraft asset {name}; run Minecraft 26.3 once in the official launcher"
            )
        })?;
        ensure!(
            bytes.len() as u64 == size,
            "Minecraft asset size differs: {name}"
        );
        verify_hash(&bytes, hash, name)?;
        Ok(bytes)
    }
}

fn verify_hash(bytes: &[u8], expected: &str, name: &str) -> Result<()> {
    if !format!("{:x}", Sha1::digest(bytes)).eq_ignore_ascii_case(expected) {
        bail!("Minecraft {name} checksum does not match its metadata");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_validation_rejects_corrupted_assets() {
        let bytes = b"minecraft";
        let hash = format!("{:x}", Sha1::digest(bytes));
        assert!(verify_hash(bytes, &hash, "test").is_ok());
        assert!(verify_hash(b"corrupt", &hash, "test").is_err());
    }
}
