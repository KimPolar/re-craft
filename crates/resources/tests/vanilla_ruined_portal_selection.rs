use anyhow::{Context, Result};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use native_resources::{
    packs::{ResourceProvider, ResourceStack},
    ruined_portal::{Settings, VerticalPlacement, find_suitable_y},
    structure_start::Heightmap,
    structure_transform::{Mirror, Rotation},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

fn definition(mode: usize) -> Value {
    let setup = |probability: f32, weight: f32| {
        json!({
            "placement":"on_land_surface", "air_pocket_probability":probability,
            "mossiness":0.2, "overgrown":false, "vines":false,
            "can_be_cold":false, "replace_with_blackstone":false, "weight":weight
        })
    };
    let setups = match mode {
        0 => vec![setup(0.0, 1.0)],
        1 => vec![setup(1.0, 1.0)],
        2 => vec![setup(0.35, 1.0)],
        3 => vec![setup(0.0, 0.5), setup(0.35, 0.5)],
        _ => vec![setup(1.0, 0.1), setup(0.0, 0.3), setup(0.35, 0.6)],
    };
    json!({"type":"minecraft:ruined_portal", "setups":setups})
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ruined_portal_selection_and_height_match_java() -> Result<()> {
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
    let source = temp.path().join("RuinedPortalSelectionOracle.java");
    std::fs::write(
        &source,
        include_str!("java/RuinedPortalSelectionOracle.java"),
    )?;
    let height_source = temp.path().join("RuinedPortalHeightOracle.java");
    std::fs::write(
        &height_source,
        include_str!("java/RuinedPortalHeightOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .arg(height_source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("RuinedPortalSelectionOracle")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|line| line.starts_with("C ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 9);
        let mode: usize = fields[1].parse()?;
        let seed: i64 = fields[2].parse()?;
        let x: i32 = fields[3].parse()?;
        let z: i32 = fields[4].parse()?;
        let mut random = LegacyRandom::new(seed);
        let a = random.next_long();
        let b = random.next_long();
        let mut random =
            LegacyRandom::new(i64::from(x).wrapping_mul(a) ^ i64::from(z).wrapping_mul(b) ^ seed);
        let selection = Settings::decode(&definition(mode))?.select_initial(&mut random)?;
        assert_eq!(
            format!(
                "{}:{}",
                selection.template.namespace, selection.template.path
            ),
            fields[5],
            "{line}"
        );
        let rotation = match fields[6] {
            "0" => Rotation::None,
            "1" => Rotation::Clockwise90,
            "2" => Rotation::Clockwise180,
            "3" => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid rotation"),
        };
        let mirror = if fields[7] == "0" {
            Mirror::None
        } else {
            Mirror::FrontBack
        };
        assert_eq!(selection.rotation, rotation, "{line}");
        assert_eq!(selection.mirror, mirror, "{line}");
        assert_eq!(random.next_int(), fields[8].parse::<i32>()?, "{line}");
        count += 1;
    }
    assert_eq!(count, 5 * 7 * 33);
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("RuinedPortalHeightOracle")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java height oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|line| line.starts_with("H ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 10);
        let placement = match fields[1] {
            "ON_LAND_SURFACE" => VerticalPlacement::OnLandSurface,
            "PARTLY_BURIED" => VerticalPlacement::PartlyBuried,
            "ON_OCEAN_FLOOR" => VerticalPlacement::OnOceanFloor,
            "IN_MOUNTAIN" => VerticalPlacement::InMountain,
            "UNDERGROUND" => VerticalPlacement::Underground,
            "IN_NETHER" => VerticalPlacement::InNether,
            _ => anyhow::bail!("invalid vertical placement"),
        };
        let pocket: bool = fields[2].parse()?;
        let seed: i64 = fields[3].parse()?;
        let surface: i32 = fields[4].parse()?;
        let x: i32 = fields[5].parse()?;
        let z: i32 = fields[6].parse()?;
        let height: i32 = fields[7].parse()?;
        let bounds = BoundingBox::new([x, 0, z], [x + height + 2, height - 1, z + 9]).unwrap();
        let mut random = LegacyRandom::new(seed);
        let actual = find_suitable_y(
            &mut random,
            placement,
            pocket,
            surface,
            height,
            bounds,
            -64,
            |[x, y, z], map| {
                let floor = 60 + (x * 13 + z * 7).rem_euclid(11) - 5;
                Ok((-64..320).contains(&y)
                    && (y <= floor || (map == Heightmap::WorldSurface && y <= floor + 3)))
            },
        )?;
        assert_eq!(actual, fields[8].parse::<i32>()?, "{line}");
        assert_eq!(random.next_int(), fields[9].parse::<i32>()?, "{line}");
        count += 1;
    }
    assert_eq!(count, 6 * 2 * 7 * 4 * 7);
    let mut resources = ResourceStack::open(&game)?;
    for name in [
        "ruined_portal",
        "ruined_portal_desert",
        "ruined_portal_jungle",
        "ruined_portal_mountain",
        "ruined_portal_nether",
        "ruined_portal_ocean",
        "ruined_portal_swamp",
    ] {
        let path = format!("data/minecraft/worldgen/structure/{name}.json");
        let bytes = resources
            .read_builtin_data(&path)?
            .with_context(|| format!("missing {path}"))?;
        Settings::decode(&serde_json::from_slice(&bytes)?)?;
    }
    Ok(())
}
