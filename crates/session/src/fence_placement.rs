//! Data-pack-aware fence placement state, before any world mutation.
use anyhow::{Result, ensure};
use native_resources::ore_feature::OreBlockTags;
use native_world::{
    fluid::{FluidKind, FluidState},
    loaded::BlockState,
};

/// Compute all four connections before placing a fence. Callers must reject
/// missing neighbors and apply the returned state through normal block updates.
pub fn placement(
    name: &str,
    fluid: Option<FluidState>,
    tags: &OreBlockTags,
    mut neighbor: impl FnMut([i32; 3]) -> Result<BlockState>,
) -> Result<BlockState> {
    let mut block = BlockState {
        name: name.to_owned(),
        properties: ["north", "east", "south", "west", "waterlogged"]
            .into_iter()
            .map(|key| (key.to_owned(), "false".to_owned()))
            .collect(),
    };
    ensure!(
        native_world::collision::fence_waterlogged(&block).is_some(),
        "unsupported fence item"
    );
    for (name, direction) in [
        ("north", [0, 0, -1]),
        ("east", [1, 0, 0]),
        ("south", [0, 0, 1]),
        ("west", [-1, 0, 0]),
    ] {
        let other = neighbor(direction)?;
        let connected = crate::worldgen::fence_connection(tags, &block, &other, direction)?;
        block.properties.insert(name.into(), connected.to_string());
    }
    let wet = fluid.is_some_and(|fluid| fluid.kind == FluidKind::Water && fluid.source);
    block
        .properties
        .insert("waterlogged".into(), wet.to_string());
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, ensure};
    use native_resources::{
        ore_feature::OreBlockTags,
        packs::{ResourceId, ResourceStack},
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
        process::Command,
    };

    #[test]
    fn placement_uses_active_tags_and_rejects_missing_neighbors() -> Result<()> {
        use native_resources::packs::ResourceProvider;
        struct Tags;
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let values = match path {
                    "data/minecraft/tags/block/fences.json"
                    | "data/minecraft/tags/block/wooden_fences.json" => {
                        vec!["minecraft:oak_fence", "minecraft:air"]
                    }
                    "data/minecraft/tags/block/shulker_boxes.json" => vec![],
                    _ => return Ok(None),
                };
                Ok(Some(serde_json::to_vec(
                    &serde_json::json!({"values": values}),
                )?))
            }
        }
        let registry =
            BTreeSet::from(["minecraft:air".to_owned(), "minecraft:oak_fence".to_owned()]);
        let names = [
            "minecraft:fences",
            "minecraft:wooden_fences",
            "minecraft:shulker_boxes",
        ]
        .into_iter()
        .map(ResourceId::parse)
        .collect::<Result<BTreeSet<_>>>()?;
        let tags = OreBlockTags::load(&mut Tags, &registry, &names)?;
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        let water = Some(FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        });
        let block = placement("minecraft:oak_fence", water, &tags, |_| Ok(air.clone()))?;
        assert!(block.properties.values().all(|value| value == "true"));
        assert!(
            placement("minecraft:oak_fence", None, &tags, |_| anyhow::bail!(
                "neighbor missing"
            ))
            .is_err()
        );
        assert!(placement("minecraft:iron_fence", None, &tags, |_| Ok(air.clone())).is_err());
        Ok(())
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn fence_placement_states_match_java() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let tag_names = [
            "minecraft:fences",
            "minecraft:wooden_fences",
            "minecraft:shulker_boxes",
        ]
        .into_iter()
        .map(ResourceId::parse)
        .collect::<Result<BTreeSet<_>>>()?;
        let tags = OreBlockTags::load(&mut resources, &ids, &tag_names)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FencePlacementOracle.java");
        let output = temp.path().join("fences.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FencePlacementOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let result = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("FencePlacementOracle")
            .arg(&output)
            .output()?;
        ensure!(
            result.status.success(),
            "Java fence placement: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(output)?)?;
        ensure!(rows.len() >= 1_000, "missing Java cases");
        let make = |name: &str| -> Result<BlockState> {
            let (name, properties) = registry.resolve(&serde_json::json!(name))?;
            Ok(BlockState { name, properties })
        };
        let air = make("minecraft:air")?;
        for row in rows {
            let direction = match row["direction"].as_str().context("missing direction")? {
                "north" => [0, 0, -1],
                "east" => [1, 0, 0],
                "south" => [0, 0, 1],
                "west" => [-1, 0, 0],
                _ => anyhow::bail!("invalid direction"),
            };
            let other = make(&format!(
                "minecraft:{}",
                row["neighbor"].as_str().context("missing neighbor")?
            ))?;
            let fluid = match row["fluid"].as_str().context("missing fluid")? {
                "air" => None,
                "water" => Some(FluidState {
                    kind: FluidKind::Water,
                    amount: 8,
                    source: true,
                    falling: false,
                }),
                "flowing" => Some(FluidState {
                    kind: FluidKind::Water,
                    amount: 7,
                    source: false,
                    falling: false,
                }),
                _ => anyhow::bail!("invalid fluid"),
            };
            let actual = placement(
                row["fence"].as_str().context("missing fence")?,
                fluid,
                &tags,
                |d| {
                    Ok(if d == direction {
                        other.clone()
                    } else {
                        air.clone()
                    })
                },
            )?;
            let expected = row["properties"]
                .as_object()
                .context("missing properties")?
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        value.as_str().context("invalid property")?.to_owned(),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            assert_eq!(actual.properties, expected, "{row}");
        }
        Ok(())
    }
}
