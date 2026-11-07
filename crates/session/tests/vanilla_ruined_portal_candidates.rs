//! Compare the production portal candidate check against the installed game.
use anyhow::{Context, Result};
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

struct SingleSet {
    resources: ResourceStack,
    structure: String,
}
impl ResourceProvider for SingleSet {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.resources.read_asset(path)
    }
    fn read_builtin_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.resources.read_builtin_data(path)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path == "data/example/worldgen/structure_set/portal.json" {
            return Ok(Some(serde_json::to_vec(&json!({
                "placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},
                "structures":[{"structure":self.structure,"weight":1}]
            }))?));
        }
        self.resources.read_data(path)
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        self.resources.list_data(folder)
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ruined_portal_candidates_match_vanilla_biomes() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("VanillaRuinedPortalCandidatesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/VanillaRuinedPortalCandidatesOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let structures = [
        "ruined_portal",
        "ruined_portal_desert",
        "ruined_portal_jungle",
        "ruined_portal_mountain",
        "ruined_portal_nether",
        "ruined_portal_ocean",
        "ruined_portal_swamp",
    ];
    let coordinates = [
        [20, 19],
        [0, 0],
        [-3, 0],
        [19, -12],
        [-32, 46],
        [100, -70],
        [-100, 17],
        [75, 82],
    ];
    let cases: Vec<_> = structures
        .iter()
        .flat_map(|name| {
            coordinates
                .into_iter()
                .map(move |[x, z]| json!({"structure":format!("minecraft:{name}"), "x":x, "z":z}))
        })
        .collect();
    let input = temp.path().join("portal-cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases)?)?;
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("VanillaRuinedPortalCandidatesOracle")
        .arg(temp.path())
        .arg(input)
        .arg(version.join("26.3.jar"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout)?;
    let lines: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("P "))
        .collect();
    assert_eq!(lines.len(), cases.len());
    let resources = ResourceStack::open(&game)?;
    let mut provider = SingleSet {
        resources,
        structure: String::new(),
    };
    let preset: Value = serde_json::from_slice(
        &provider
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")?
            .context("normal preset missing")?,
    )?;
    let generator = TerrainGenerator::with_structure_context(
        &mut provider,
        &preset["dimensions"]["minecraft:overworld"]["generator"],
        -1,
        -64,
        384,
    )?;
    let ids: Vec<_> = structures
        .iter()
        .map(|name| ResourceId::parse(&format!("minecraft:{name}")))
        .collect::<Result<_>>()?;
    let rules = StructureBiomes::load(&mut provider, ids)?;
    let set = ResourceId::parse("example:portal")?;
    let mut evaluator = DensityEvaluator::default();
    let mut accepted = 0;
    let mut rejected = 0;
    for (case, line) in cases.iter().zip(lines) {
        let words: Vec<_> = line.split_whitespace().collect();
        assert_eq!(words[1], case["structure"].as_str().unwrap(), "{line}");
        let x = case["x"].as_i64().unwrap() as i32;
        let z = case["z"].as_i64().unwrap() as i32;
        provider.structure = words[1].into();
        let result = generator.try_generate_simple_structure_set(
            &mut provider,
            &mut evaluator,
            &mut StructureSetCache::default(),
            &rules,
            &set,
            [x, z],
            |_| Ok(false),
            |value| {
                let name = value
                    .as_str()
                    .or_else(|| value.get("id").and_then(Value::as_str))
                    .context("block ID missing")?;
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: name == "minecraft:stone",
                    blocks_motion_no_leaves: name == "minecraft:stone",
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            },
        );
        match words[4] {
            "X" => {
                assert!(
                    matches!(
                        result?,
                        StructureSetStart::Rejected | StructureSetStart::NotCandidate
                    ),
                    "{line}"
                );
                rejected += 1;
            }
            "S" => {
                let error = result.expect_err("accepted portal must remain unsupported");
                assert!(
                    format!("{error:#}")
                        .contains("ruined portal structure pieces are not yet supported"),
                    "{line}: {error:#}"
                );
                accepted += 1;
            }
            _ => anyhow::bail!("invalid Java result: {line}"),
        }
    }
    assert!(accepted > 0 && rejected > 0, "both outcomes are required");
    Ok(())
}
