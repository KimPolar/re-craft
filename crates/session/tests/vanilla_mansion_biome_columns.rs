//! Compare the early mansion biome rejection against the installed 26.3 client.
use anyhow::{Context, Result, ensure};
use native_persistence::heightmap::BlockHeightFlags;
use native_resources::{
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureSetCache, StructureSetStart},
};
use native_session::worldgen::TerrainGenerator;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct MansionSet {
    resources: ResourceStack,
    all_biomes: bool,
}

impl ResourceProvider for MansionSet {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.resources.read_asset(path)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path == "data/example/worldgen/structure_set/mansion.json" {
            return Ok(Some(serde_json::to_vec(&json!({
                "placement": {"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},
                "structures": [{"structure":"minecraft:mansion","weight":1}]
            }))?));
        }
        if self.all_biomes && path == "data/minecraft/worldgen/structure/mansion.json" {
            let mut definition: Value = serde_json::from_slice(
                &self
                    .resources
                    .read_data(path)?
                    .context("mansion definition missing")?,
            )?;
            let biomes: Vec<_> = self
                .resources
                .list_data("worldgen/biome")?
                .into_iter()
                .map(|id| format!("{}:{}", id.namespace, id.path))
                .collect();
            definition["biomes"] = json!(biomes);
            return Ok(Some(serde_json::to_vec(&definition)?));
        }
        self.resources.read_data(path)
    }

    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        self.resources.list_data(folder)
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn mansion_column_rejection_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let temp = tempfile::tempdir()?;
    let packs = temp.path().join("packs");
    std::fs::create_dir(&packs)?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("MansionBiomeColumnOracle.java");
    std::fs::write(&source, include_str!("java/MansionBiomeColumnOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let positions = [
        [9, 21],
        [20, 19],
        [0, 0],
        [-3, 0],
        [19, -12],
        [-32, 46],
        [100, -70],
        [-100, 17],
        [75, 82],
        [-1, -1],
        [44, 30],
        [-41, -29],
    ];
    let mut cases = Vec::new();
    for seed in [-1_i64, 0, 42] {
        for mode in ["vanilla", "all"] {
            for [x, z] in positions {
                cases.push(json!({"seed": seed, "mode": mode, "x": x, "z": z}));
            }
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases)?)?;
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("MansionBiomeColumnOracle")
        .arg(packs)
        .arg(input)
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::str::from_utf8(&output.stdout)?;
    let expected: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("COLUMN "))
        .collect();
    ensure!(expected.len() == cases.len(), "missing Java column results");

    let mut provider = MansionSet {
        resources: ResourceStack::open(&game)?,
        all_biomes: false,
    };
    let preset: Value = serde_json::from_slice(
        &provider
            .resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")?
            .context("normal preset missing")?,
    )?;
    let structure = ResourceId::parse("minecraft:mansion")?;
    let set = ResourceId::parse("example:mansion")?;
    let mut index = 0;
    let mut rejected = 0;
    let mut possible = 0;
    for seed in [-1_i64, 0, 42] {
        let generator = TerrainGenerator::with_structure_context(
            &mut provider,
            &preset["dimensions"]["minecraft:overworld"]["generator"],
            seed,
            -64,
            384,
        )?;
        for all in [false, true] {
            provider.all_biomes = all;
            let rules = StructureBiomes::load(&mut provider, [structure.clone()])?;
            let mut evaluator = DensityEvaluator::default();
            let mut cache = StructureSetCache::default();
            for [x, z] in positions {
                let mut fields = expected[index].split_whitespace();
                ensure!(fields.next().context("index missing")?.parse::<usize>()? == index);
                let java_possible: bool = fields.next().context("result missing")?.parse()?;
                ensure!(fields.next().is_none(), "extra column result fields");
                let result = generator.try_generate_simple_structure_set(
                    &mut provider,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &set,
                    [x, z],
                    |_| Ok(false),
                    |value| {
                        let id = value
                            .as_str()
                            .or_else(|| value.get("id").and_then(Value::as_str))
                            .context("block id missing")?;
                        Ok(BlockHeightFlags {
                            is_air: id == "minecraft:air",
                            blocks_motion: id == "minecraft:stone",
                            blocks_motion_no_leaves: id == "minecraft:stone",
                            has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                        })
                    },
                );
                if java_possible {
                    let error = result.expect_err("possible mansion must remain unsupported");
                    ensure!(
                        format!("{error:#}")
                            .contains("unsupported simple structure minecraft:woodland_mansion")
                    );
                    possible += 1;
                } else {
                    ensure!(matches!(result?, StructureSetStart::Rejected));
                    rejected += 1;
                }
                index += 1;
            }
        }
    }
    ensure!(rejected > 0 && possible > 0, "both outcomes are required");
    eprintln!(
        "Compared {index} mansion biome columns with Java ({rejected} rejected, {possible} possible)"
    );
    Ok(())
}
