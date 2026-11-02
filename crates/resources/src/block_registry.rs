//! Read verified stateless block registrations and optional local block reports.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::{Manifest, TARGET_VERSION, open_verified_client, packs::ResourceId};

pub type DefaultProperties = BTreeMap<String, BTreeMap<String, String>>;

/// Blocks proven to inherit the empty base block state definition. Blocks
/// with state properties still require separate default-state decoding.
pub struct StatelessBlockRegistry {
    ids: BTreeSet<String>,
}

impl StatelessBlockRegistry {
    /// Read class files from a hash-verified 26.3 client JAR.
    pub fn load(mut read_class: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Self> {
        const BLOCKS: &str = "net/minecraft/world/level/block/Blocks";
        const IDS: &str = "net/minecraft/references/BlockItemIds";
        const DIRECT: &str = "(Lnet/minecraft/references/BlockItemId;Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
        const FACTORY: &str = "(Lnet/minecraft/references/BlockItemId;Ljava/util/function/Function;Lnet/minecraft/world/level/block/state/BlockBehaviour$Properties;)Lnet/minecraft/world/level/block/Block;";
        const BASE: &str = "net/minecraft/world/level/block/Block";
        let mut load = |name: &str| -> Result<crate::item_registry::Class> {
            let bytes = read_class(&format!("{name}.class"))?
                .with_context(|| format!("missing class {name}"))?;
            let class = crate::item_registry::Class::read(&bytes)?;
            ensure!(class.name == name, "class name does not match {name}");
            Ok(class)
        };
        let names = load(IDS)?.block_reference_names()?;
        let blocks = load(BLOCKS)?;
        let mut candidates = BTreeSet::new();
        let mut registration = None;
        let mut constructors = BTreeSet::new();
        let mut registered_classes = BTreeMap::new();
        for ins in blocks.instructions()? {
            if ins.op == 0xba {
                if !candidates.is_empty()
                    && let Some(class) = blocks.block_factory_constructor(
                        ins.index.context("missing invokedynamic reference")?,
                    )?
                {
                    constructors.insert(class);
                }
                continue;
            }
            if !matches!(ins.op, 0xb2 | 0xb3 | 0xb8) {
                continue;
            }
            let (owner, member, descriptor) =
                blocks.member(ins.index.context("missing class reference")?)?;
            if ins.op == 0xb2 && owner == IDS {
                candidates.insert(member.to_owned());
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && member == "register"
                && descriptor == DIRECT
            {
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(true);
            } else if ins.op == 0xb8
                && owner == BLOCKS
                && member == "register"
                && descriptor == FACTORY
            {
                ensure!(registration.is_none(), "ambiguous block registration");
                registration = Some(false);
            } else if ins.op == 0xb3 && owner == BLOCKS {
                let class = match registration {
                    Some(true) => Some(BASE.to_owned()),
                    Some(false) if constructors.len() == 1 => constructors.first().cloned(),
                    _ => None,
                };
                if let Some(class) = class
                    && candidates.len() == 1
                    && candidates.contains(member)
                {
                    let name = names
                        .get(member)
                        .with_context(|| format!("missing block reference {member}"))?;
                    let id = ResourceId::parse(name)?;
                    ensure!(
                        id.namespace == "minecraft",
                        "unexpected block reference namespace"
                    );
                    ensure!(
                        registered_classes
                            .insert(format!("{}:{}", id.namespace, id.path), class)
                            .is_none(),
                        "duplicate block registration"
                    );
                }
                candidates.clear();
                constructors.clear();
                registration = None;
            }
        }
        drop(load);
        let mut cache = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for (id, class) in registered_classes {
            if stateless_class(&class, &mut read_class, &mut cache, &mut BTreeSet::new())? {
                ids.insert(id);
            }
        }
        ensure!(
            ids.contains("minecraft:stone") && ids.contains("minecraft:dirt"),
            "stateless block registration format is unsupported"
        );
        Ok(Self { ids })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

fn stateless_class(
    name: &str,
    read_class: &mut impl FnMut(&str) -> Result<Option<Vec<u8>>>,
    cache: &mut BTreeMap<String, bool>,
    visiting: &mut BTreeSet<String>,
) -> Result<bool> {
    const BASE: &str = "net/minecraft/world/level/block/Block";
    const DESCRIPTOR: &str = "(Lnet/minecraft/world/level/block/state/StateDefinition$Builder;)V";
    if let Some(result) = cache.get(name) {
        return Ok(*result);
    }
    ensure!(
        name.starts_with("net/minecraft/world/level/block/")
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_$".contains(&c)),
        "invalid block class name"
    );
    ensure!(
        visiting.insert(name.to_owned()) && visiting.len() <= 64,
        "cyclic block inheritance"
    );
    let result = (|| {
        let bytes = read_class(&format!("{name}.class"))?
            .with_context(|| format!("missing block class {name}"))?;
        let class = crate::item_registry::Class::read(&bytes)?;
        ensure!(
            class.name == name,
            "block class name does not match its path"
        );
        if name == BASE {
            ensure!(
                class.declares_method("createBlockStateDefinition", DESCRIPTOR),
                "base block state method is missing"
            );
            Ok(true)
        } else if class.declares_method("createBlockStateDefinition", DESCRIPTOR) {
            Ok(false)
        } else {
            stateless_class(
                class
                    .superclass
                    .as_deref()
                    .context("missing block superclass")?,
                read_class,
                cache,
                visiting,
            )
        }
    })();
    visiting.remove(name);
    if let Ok(value) = result {
        cache.insert(name.to_owned(), value);
    }
    result
}

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
    fn stateless_registry_rejects_missing_and_malformed_classes() {
        assert!(StatelessBlockRegistry::load(|_| Ok(None)).is_err());
        assert!(StatelessBlockRegistry::load(|_| Ok(Some(vec![0xca, 0xfe, 0xba, 0xbe]))).is_err());
    }

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
