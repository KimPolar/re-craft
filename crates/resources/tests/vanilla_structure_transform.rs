use native_resources::structure_transform::{self, Mirror, Rotation};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn structure_transforms_match_java() -> anyhow::Result<()> {
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
    let source = temp.path().join("StructureTransformOracle.java");
    std::fs::write(&source, include_str!("java/StructureTransformOracle.java"))?;
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
        .arg("StructureTransformOracle")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|line| line.starts_with("T ")) {
        let row: Vec<_> = line.split_whitespace().collect();
        let rotation = match row[1] {
            "0" => Rotation::None,
            "1" => Rotation::Clockwise90,
            "2" => Rotation::Clockwise180,
            "3" => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid rotation"),
        };
        let mirror = match row[2] {
            "0" => Mirror::None,
            "1" => Mirror::LeftRight,
            "2" => Mirror::FrontBack,
            _ => anyhow::bail!("invalid mirror"),
        };
        let i: i32 = row[3].parse()?;
        let pivot = [i % 11 - 5, 19, i % 13 - 6];
        let block = structure_transform::transform_block(
            [i * 7 - 500, i - 70, 300 - i * 11],
            mirror,
            rotation,
            pivot,
        );
        assert_eq!(
            block,
            [
                row[4].parse::<i32>()?,
                row[5].parse::<i32>()?,
                row[6].parse::<i32>()?
            ],
            "{line}"
        );
        let entity = structure_transform::transform_entity(
            [
                f64::from(i) * 0.25 - 17.125,
                f64::from(i) * 0.125 - 4.5,
                21.375 - f64::from(i) * 0.5,
            ],
            mirror,
            rotation,
            pivot,
        );
        assert_eq!(
            entity.map(|n| n.to_bits() as i64),
            [
                row[7].parse::<i64>()?,
                row[8].parse::<i64>()?,
                row[9].parse::<i64>()?
            ],
            "{line}"
        );
        let bounds = structure_transform::bounding_box(
            [i % 17, i % 19, i % 23],
            [i * 16 - 900, -64 + i % 9, 700 - i * 8],
            mirror,
            rotation,
            pivot,
        )?;
        assert_eq!(
            bounds.min,
            [
                row[10].parse::<i32>()?,
                row[11].parse::<i32>()?,
                row[12].parse::<i32>()?
            ],
            "{line}"
        );
        assert_eq!(
            bounds.max,
            [
                row[13].parse::<i32>()?,
                row[14].parse::<i32>()?,
                row[15].parse::<i32>()?
            ],
            "{line}"
        );
        count += 1;
    }
    assert_eq!(count, 4 * 3 * 128);
    Ok(())
}
