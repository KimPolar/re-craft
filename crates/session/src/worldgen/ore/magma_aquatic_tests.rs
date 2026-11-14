use super::*;

const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

fn plant(name: &str, property: Option<(&str, &str)>) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: property
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
    }
}

#[test]
fn aquatic_magma_faces_validate_states() -> Result<()> {
    let (mut terrain, tags, motion, _) = super::tests::fixture();
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable)?;
    let mut valid = vec![
        plant("seagrass", None),
        plant("tall_seagrass", Some(("half", "lower"))),
        plant("tall_seagrass", Some(("half", "upper"))),
        plant("kelp_plant", None),
    ];
    for age in 0..=25 {
        valid.push(plant("kelp", Some(("age", &age.to_string()))));
    }
    for state in valid {
        for direction in DIRECTIONS {
            assert!(
                !MagmaWorld::face_is_full(&mut region, &state, direction)?,
                "{state:?}"
            );
        }
    }
    let invalid = [
        plant("seagrass", Some(("extra", "true"))),
        plant("tall_seagrass", None),
        plant("tall_seagrass", Some(("half", "invalid"))),
        plant("kelp_plant", Some(("age", "0"))),
        plant("kelp", None),
        plant("kelp", Some(("age", "26"))),
        plant("kelp", Some(("age", "01"))),
    ];
    for state in invalid {
        assert!(
            MagmaWorld::face_is_full(&mut region, &state, DIRECTIONS[0]).is_err(),
            "{state:?}"
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn aquatic_magma_faces_match_installed_java() -> Result<()> {
    use std::{path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "unexpected Minecraft version");
    let temp = tempfile::tempdir()?;
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("MagmaAquaticOcclusionOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/MagmaAquaticOcclusionOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "javac failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("MagmaAquaticOcclusionOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let (mut terrain, tags, motion, _) = super::tests::fixture();
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable)?;
    let mut rows = BTreeSet::new();
    for line in String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| line.starts_with("AQUA "))
    {
        let words: Vec<_> = line.split_whitespace().collect();
        ensure!(words.len() == 5, "invalid oracle row: {line}");
        let name = words[1]
            .strip_prefix("minecraft:")
            .context("invalid namespace")?;
        ensure!(
            ["seagrass", "tall_seagrass", "kelp", "kelp_plant"].contains(&name),
            "invalid block: {line}"
        );
        let property = if words[2] == "none" {
            None
        } else {
            Some(words[2].split_once('=').context("invalid property")?)
        };
        let state = plant(name, property);
        let direction = match words[3] {
            "down" => DIRECTIONS[0],
            "up" => DIRECTIONS[1],
            "north" => DIRECTIONS[2],
            "south" => DIRECTIONS[3],
            "west" => DIRECTIONS[4],
            "east" => DIRECTIONS[5],
            _ => bail!("invalid direction: {line}"),
        };
        assert_eq!(
            MagmaWorld::face_is_full(&mut region, &state, direction)?,
            words[4].parse::<bool>()?,
            "{line}"
        );
        assert!(
            rows.insert((state.name, words[2].to_owned(), direction)),
            "duplicate: {line}"
        );
    }
    assert_eq!(rows.len(), 30 * DIRECTIONS.len());
    Ok(())
}
