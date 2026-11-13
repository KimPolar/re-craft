use native_resources::{
    packs::ResourceStack,
    structure_transform::{Mirror, Rotation},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ocean_ruin_states_transform_like_java() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("StructureStateTransformOracle.java");
    std::fs::write(
        &source,
        include_str!("java/StructureStateTransformOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("StructureStateTransformOracle")
        .arg(version.join("26.3.jar"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut resources = ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    let mut chain_count = 0;
    let mut wall_count = 0;
    let mut sulfur_count = 0;
    let mut trapdoor_count = 0;
    let mut crop_count = 0;
    let mut barrel_count = 0;
    let mut hopper_count = 0;
    let mut button_count = 0;
    let mut dispenser_count = 0;
    let mut plate_count = 0;
    let mut tripwire_count = 0;
    let mut trial_spawner_count = 0;
    let mut dripstone_count = 0;
    for line in text.lines().filter_map(|line| line.strip_prefix("STATE\t")) {
        let row: Value = serde_json::from_str(line)?;
        let id = row["id"].as_str().unwrap();
        barrel_count += usize::from(id == "minecraft:barrel");
        hopper_count += usize::from(id == "minecraft:hopper");
        dispenser_count += usize::from(id == "minecraft:dispenser");
        tripwire_count += usize::from(matches!(
            id,
            "minecraft:tripwire" | "minecraft:tripwire_hook"
        ));
        plate_count += usize::from(matches!(
            id,
            "minecraft:oak_pressure_plate"
                | "minecraft:stone_pressure_plate"
                | "minecraft:light_weighted_pressure_plate"
                | "minecraft:heavy_weighted_pressure_plate"
        ));
        button_count += usize::from(matches!(
            id,
            "minecraft:oak_button" | "minecraft:stone_button"
        ));
        trial_spawner_count += usize::from(id == "minecraft:trial_spawner");
        dripstone_count += usize::from(id == "minecraft:pointed_dripstone");
        chain_count += usize::from(id == "minecraft:iron_chain");
        sulfur_count += usize::from(matches!(
            id,
            "minecraft:potent_sulfur" | "minecraft:sulfur_spike"
        ));
        trapdoor_count += usize::from(id == "minecraft:dark_oak_trapdoor");
        crop_count += usize::from(matches!(
            id,
            "minecraft:wheat"
                | "minecraft:carrots"
                | "minecraft:potatoes"
                | "minecraft:beetroots"
                | "minecraft:farmland"
                | "minecraft:sweet_berry_bush"
        ));
        wall_count += usize::from(matches!(
            id,
            "minecraft:stone_brick_wall"
                | "minecraft:mossy_stone_brick_wall"
                | "minecraft:cobblestone_wall"
                | "minecraft:mossy_cobblestone_wall"
                | "minecraft:blackstone_wall"
                | "minecraft:polished_blackstone_brick_wall"
        ));
        let input: BTreeMap<String, String> = serde_json::from_value(row["input"].clone())?;
        let expected: BTreeMap<String, String> = serde_json::from_value(row["result"].clone())?;
        let mirror = match row["mirror"].as_u64().unwrap() {
            0 => Mirror::None,
            1 => Mirror::LeftRight,
            2 => Mirror::FrontBack,
            _ => anyhow::bail!("invalid mirror"),
        };
        let rotation = match row["rotation"].as_u64().unwrap() {
            0 => Rotation::None,
            1 => Rotation::Clockwise90,
            2 => Rotation::Clockwise180,
            3 => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid rotation"),
        };
        let actual = registry
            .transform_template_state(id, &input, mirror, rotation)
            .map_err(|e| anyhow::anyhow!("{id} {input:?}: {e:#}"))?;
        assert_eq!(actual, expected, "{line}");
        count += 1;
    }
    assert!(count > 2000, "unexpectedly few oracle cases: {count}");
    assert_eq!(chain_count, 6 * 12, "missing iron chain state transforms");
    assert_eq!(sulfur_count, 25 * 12, "missing sulfur state transforms");
    assert_eq!(trapdoor_count, 64 * 12, "missing trapdoor state transforms");
    assert_eq!(crop_count, 40 * 12, "missing crop or farmland transforms");
    assert_eq!(barrel_count, 12 * 12, "missing barrel state transforms");
    assert_eq!(hopper_count, 10 * 12, "missing hopper state transforms");
    assert_eq!(button_count, 2 * 24 * 12, "missing button state transforms");
    assert_eq!(
        tripwire_count,
        (128 + 16) * 12,
        "missing tripwire state transforms"
    );
    assert_eq!(
        plate_count,
        (2 * 2 + 2 * 16) * 12,
        "missing pressure plate state transforms"
    );
    assert_eq!(
        dispenser_count,
        12 * 12,
        "missing dispenser state transforms"
    );
    assert_eq!(
        dripstone_count,
        20 * 12,
        "missing pointed dripstone state transforms"
    );
    assert_eq!(
        trial_spawner_count,
        12 * 12,
        "missing trial spawner state transforms"
    );
    assert_eq!(
        wall_count,
        6 * 324 * 12,
        "missing portal wall state transforms"
    );
    println!("verified {count} Java block-state transforms");
    Ok(())
}
