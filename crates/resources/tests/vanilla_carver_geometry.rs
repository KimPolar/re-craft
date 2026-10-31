//! Compare ellipsoid masks, visit order and reach decisions with Java 26.3.
use native_resources::carver::{CarvingMask, can_reach, carve_ellipsoid};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java carver oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn ellipsoids_masks_and_reach_match_vanilla() {
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
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("CarverGeometryOracle.java");
    std::fs::write(&source, include_str!("java/CarverGeometryOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut state = 3_892_731_993u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 32) as u32
    };
    let mut cases = Vec::new();
    for i in 0..300 {
        let cx = (next() % 101) as i32 - 50;
        let cz = (next() % 101) as i32 - 50;
        let x = f64::from(cx * 16) + (next() % 960) as f64 / 16.0 - 22.0;
        let z = f64::from(cz * 16) + (next() % 960) as f64 / 16.0 - 22.0;
        let y = (next() % 3000) as f64 / 20.0 - 65.0;
        cases.push(json!({"chunk_x":cx,"chunk_z":cz,"center":[x,y,z],
            "horizontal":(next()%240+1) as f64/16.0,"vertical":(next()%300+1) as f64/16.0,
            "floor":(next()%20) as f64/20.0-1.0,"mode":i%3,
            "current":(next()%112) as i32,"total":112,"thickness":(next()%90) as f32/10.0}));
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CarverGeometryOracle")
        .arg(&input));
    let mut groups = Vec::<(bool, Vec<(u8, u8, i32, i32)>)>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => groups.push((p[1] == "true", Vec::new())),
            Some("P") => groups.last_mut().unwrap().1.push((
                p[1].parse().unwrap(),
                p[2].parse().unwrap(),
                p[3].parse().unwrap(),
                p[4].parse().unwrap(),
            )),
            _ => {}
        }
    }
    assert_eq!(groups.len(), cases.len());
    let mut populated = 0;
    for (i, v) in cases.iter().enumerate() {
        let chunk = [
            v["chunk_x"].as_i64().unwrap() as i32,
            v["chunk_z"].as_i64().unwrap() as i32,
        ];
        let center = [0, 1, 2].map(|j| v["center"][j].as_f64().unwrap());
        let mut mask = CarvingMask::new(-63, 88).unwrap();
        let floor = v["floor"].as_f64().unwrap();
        carve_ellipsoid(
            &mut mask,
            chunk,
            center,
            v["horizontal"].as_f64().unwrap(),
            v["vertical"].as_f64().unwrap(),
            |xd, yd, zd, y| match v["mode"].as_i64().unwrap() {
                0 => yd <= floor || xd * xd + yd * yd + zd * zd >= 1.0,
                1 => (xd * xd + zd * zd) * (1.0 + 0.125 * f64::from(y & 3)) + yd * yd / 6.0 >= 1.0,
                _ => false,
            },
        )
        .unwrap();
        assert_eq!(mask.segments(), groups[i].1, "mask case {i}");
        assert_eq!(
            can_reach(
                chunk,
                center[0],
                center[2],
                v["current"].as_i64().unwrap() as i32,
                v["total"].as_i64().unwrap() as i32,
                v["thickness"].as_f64().unwrap() as f32
            ),
            groups[i].0,
            "reach case {i}"
        );
        populated += usize::from(!mask.is_empty());
    }
    assert!(populated > 50, "fixtures need intersecting ellipsoids");
}
