//! Optional block defaults from a locally generated vanilla block report.
//!
//! This is data preparation for the read-only viewer, not a game runtime or a
//! replacement for implementing the complete block registry in Rust.

use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::{Manifest, TARGET_VERSION, open_verified_client, packs::ResourceId};

pub type DefaultProperties = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Deserialize)]
struct Metadata {
    report_format: u32,
    game_version: String,
    data_version: u32,
    client_sha1: String,
    report_sha1: String,
}

#[derive(Deserialize)]
struct ReportBlock {
    #[serde(default)]
    properties: BTreeMap<String, Vec<String>>,
    states: Vec<ReportState>,
}

#[derive(Deserialize)]
struct ReportState {
    #[serde(default)]
    default: bool,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "{} is too large",
        path.display()
    );
    Ok(bytes)
}

fn parse_report(bytes: &[u8]) -> Result<DefaultProperties> {
    let blocks: BTreeMap<String, ReportBlock> =
        serde_json::from_slice(bytes).context("invalid block report JSON")?;
    ensure!(!blocks.is_empty(), "block report is empty");
    let mut defaults = BTreeMap::new();
    for (id, block) in blocks {
        ResourceId::parse(&id).with_context(|| format!("invalid block id {id}"))?;
        let mut default = block.states.into_iter().filter(|state| state.default);
        let state = default
            .next()
            .with_context(|| format!("block {id} has no default state"))?;
        ensure!(
            default.next().is_none(),
            "block {id} has multiple default states"
        );
        ensure!(
            state.properties.len() == block.properties.len(),
            "block {id} has incomplete default properties"
        );
        for (name, value) in &state.properties {
            let allowed = block
                .properties
                .get(name)
                .with_context(|| format!("unknown default property {name} for {id}"))?;
            ensure!(
                allowed.contains(value),
                "invalid default value {name}={value} for {id}"
            );
        }
        defaults.insert(id, state.properties);
    }
    Ok(defaults)
}

fn load_for_hash(directory: &Path, client_sha1: &str) -> Result<DefaultProperties> {
    let metadata: Metadata =
        serde_json::from_slice(&read_limited(&directory.join("metadata.json"), 16 * 1024)?)
            .context("invalid block report metadata")?;
    ensure!(
        metadata.report_format == 1
            && metadata.game_version == TARGET_VERSION
            && metadata.data_version == 5023,
        "block report targets a different Minecraft version or report format"
    );
    ensure!(
        metadata.client_sha1.eq_ignore_ascii_case(client_sha1),
        "block report was generated for a different client JAR"
    );
    let report = read_limited(&directory.join("blocks.json"), 32 * 1024 * 1024)?;
    let report_hash = format!("{:x}", Sha1::digest(&report));
    if !report_hash.eq_ignore_ascii_case(&metadata.report_sha1) {
        bail!("block report checksum differs from its metadata");
    }
    parse_report(&report)
}

/// Read a cache generated from this verified local 26.3 client.
pub fn load(minecraft_dir: &Path, directory: &Path) -> Result<DefaultProperties> {
    let _client = open_verified_client(minecraft_dir)?;
    let manifest_path = minecraft_dir.join("versions/26.3/26.3.json");
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    load_for_hash(directory, &manifest.downloads.client.sha1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = r#"{
      "minecraft:stone":{"states":[{"id":1,"default":true}]},
      "minecraft:grass_block":{"properties":{"snowy":["true","false"]},
        "states":[{"id":2,"properties":{"snowy":"true"}},{"id":3,"default":true,"properties":{"snowy":"false"}}]}
    }"#;

    #[test]
    fn report_requires_a_single_complete_valid_default() {
        let defaults = parse_report(REPORT.as_bytes()).unwrap();
        assert!(defaults["minecraft:stone"].is_empty());
        assert_eq!(defaults["minecraft:grass_block"]["snowy"], "false");
        for invalid in [
            r#"{"minecraft:x":{"states":[{"id":1}]}}"#,
            r#"{"minecraft:x":{"states":[{"default":true},{"default":true}]}}"#,
            r#"{"minecraft:x":{"properties":{"p":["a"]},"states":[{"default":true}]}}"#,
            r#"{"minecraft:x":{"properties":{"p":["a"]},"states":[{"default":true,"properties":{"p":"b"}}]}}"#,
        ] {
            assert!(parse_report(invalid.as_bytes()).is_err(), "{invalid}");
        }
    }

    #[test]
    fn cache_checks_version_client_and_report_hash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blocks.json"), REPORT).unwrap();
        let report_sha1 = format!("{:x}", Sha1::digest(REPORT.as_bytes()));
        let metadata = format!(
            r#"{{"report_format":1,"game_version":"26.3","data_version":5023,"client_sha1":"abc","report_sha1":"{report_sha1}"}}"#
        );
        std::fs::write(dir.path().join("metadata.json"), metadata).unwrap();
        assert!(load_for_hash(dir.path(), "abc").is_ok());
        assert!(load_for_hash(dir.path(), "different").is_err());
        std::fs::write(dir.path().join("blocks.json"), b"{}").unwrap();
        assert!(load_for_hash(dir.path(), "abc").is_err());
    }
}
