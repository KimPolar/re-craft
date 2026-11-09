use super::*;
use native_core::wood::{Planks, Wood};

const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

#[test]
fn wood_magma_faces_validate_states() -> Result<()> {
    let (mut terrain, tags, motion, _) = super::tests::fixture();
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable)?;
    for plank in Planks::ALL {
        let mut state = BlockState {
            name: plank.id().into(),
            properties: BTreeMap::new(),
        };
        for direction in DIRECTIONS {
            assert!(
                MagmaWorld::face_is_full(&mut region, &state, direction)?,
                "{state:?}"
            );
        }
        state.properties.insert("extra".into(), "true".into());
        assert!(MagmaWorld::face_is_full(&mut region, &state, DIRECTIONS[0]).is_err());
    }
    for wood in Wood::all() {
        let mut state = BlockState {
            name: wood.id().into(),
            properties: BTreeMap::new(),
        };
        assert!(MagmaWorld::face_is_full(&mut region, &state, DIRECTIONS[0]).is_err());
        for axis in ["x", "y", "z"] {
            state.properties.insert("axis".into(), axis.into());
            for direction in DIRECTIONS {
                assert!(
                    MagmaWorld::face_is_full(&mut region, &state, direction)?,
                    "{state:?}"
                );
            }
        }
        state.properties.insert("axis".into(), "invalid".into());
        assert!(MagmaWorld::face_is_full(&mut region, &state, DIRECTIONS[0]).is_err());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn wood_magma_faces_match_installed_java() -> Result<()> {
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
    let source = temp.path().join("MagmaWoodOcclusionOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/MagmaWoodOcclusionOracle.java"),
    )?;
    let input = temp.path().join("blocks.txt");
    let ids: Vec<_> = Planks::ALL
        .into_iter()
        .map(Planks::id)
        .chain(Wood::all().map(Wood::id))
        .collect();
    std::fs::write(&input, ids.join("\n"))?;
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
        .arg("MagmaWoodOcclusionOracle")
        .arg(input)
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
        .filter(|line| line.starts_with("WOOD "))
    {
        let words: Vec<_> = line.split_whitespace().collect();
        ensure!(
            words.len() == 5 && ids.contains(&words[1]),
            "invalid oracle row: {line}"
        );
        let direction = match words[3] {
            "down" => DIRECTIONS[0],
            "up" => DIRECTIONS[1],
            "north" => DIRECTIONS[2],
            "south" => DIRECTIONS[3],
            "west" => DIRECTIONS[4],
            "east" => DIRECTIONS[5],
            _ => bail!("invalid oracle direction: {line}"),
        };
        let mut state = BlockState {
            name: words[1].into(),
            properties: BTreeMap::new(),
        };
        if words[2] != "none" {
            state.properties.insert("axis".into(), words[2].into());
        }
        assert_eq!(
            MagmaWorld::face_is_full(&mut region, &state, direction)?,
            words[4].parse::<bool>()?,
            "{line}"
        );
        assert!(
            rows.insert((state.name, words[2].to_owned(), direction)),
            "duplicate {line}"
        );
    }
    assert_eq!(
        rows.len(),
        (Planks::ALL.len() + Wood::all().count() * 3) * 6
    );
    Ok(())
}
