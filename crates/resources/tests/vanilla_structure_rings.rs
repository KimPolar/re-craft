//! Compare concentric-ring candidates and biome reservoir searches with Java.
use native_resources::structure_placement::{ConcentricRingsPlacement, find_ring_biome};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java ring oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2500)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn ring_positions_and_biome_reservoir_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
      .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("StructureRingsOracle.java");
    std::fs::write(&source, include_str!("java/StructureRingsOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let cases = [
        (32, 3, 128, 0i64, 0),
        (32, 3, 128, 2345, 1),
        (32, 3, 128, -1, 2),
        (0, 0, 17, i64::MIN, 2),
        (1023, 1, 64, i64::MAX, 0),
        (17, 0, 10, 42, 1),
    ];
    let mut cmd = Command::new(java.join("java.exe"));
    cmd.arg("-classpath").arg(&cp).arg("StructureRingsOracle");
    for (distance, spread, count, seed, mode) in cases {
        for value in [
            distance.to_string(),
            spread.to_string(),
            count.to_string(),
            seed.to_string(),
            mode.to_string(),
        ] {
            cmd.arg(value);
        }
    }
    let output = run(&mut cmd);
    let mut groups = Vec::<Vec<[i32; 2]>>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => groups.push(Vec::new()),
            Some("P") => groups
                .last_mut()
                .unwrap()
                .push([p[1].parse().unwrap(), p[2].parse().unwrap()]),
            _ => {}
        }
    }
    assert_eq!(groups.len(), cases.len());
    for (i, (distance, spread, count, seed, mode)) in cases.into_iter().enumerate() {
        let placement=ConcentricRingsPlacement::decode(&json!({"type":"minecraft:concentric_rings","distance":distance,"spread":spread,"count":count,"salt":0,"preferred_biomes":"minecraft:plains"})).unwrap();
        let result = placement
            .positions_with_search(seed, |initial, random| {
                find_ring_biome(initial, random, |[x, _, z]| {
                    Ok(match mode {
                        0 => false,
                        1 => true,
                        _ => (x.wrapping_mul(31).wrapping_add(z.wrapping_mul(17)) & 7) == 0,
                    })
                })
            })
            .unwrap();
        assert_eq!(result, groups[i], "ring fixture {i}");
    }
}
