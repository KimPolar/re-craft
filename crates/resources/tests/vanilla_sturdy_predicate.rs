use anyhow::Result;
use native_resources::{
    disk_feature::{BlockPredicate, DiskWorld},
    ore_feature::OreWorld,
    packs::ResourceId,
};
use serde_json::json;
use std::{path::PathBuf, process::Command};

struct World {
    kind: &'static str,
    seen: Vec<([i32; 3], [i32; 3])>,
}
impl OreWorld for World {
    type State = &'static str;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State> {
        assert_eq!(pos, [4, 6, 5]);
        Ok(self.kind)
    }
    fn block_is(&mut self, _: &&str, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn in_tag(&mut self, _: &&str, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn is_air(&mut self, _: &&str) -> Result<bool> {
        unreachable!()
    }
    fn set_block(&mut self, _: [i32; 3], _: &&str, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl DiskWorld for World {
    fn is_solid(&mut self, _: &&str) -> Result<bool> {
        unreachable!()
    }
    fn fluid_is(&mut self, _: &&str, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &&str) -> Result<bool> {
        unreachable!()
    }
    fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
        unreachable!()
    }
    fn face_sturdy(&mut self, state: &&str, pos: [i32; 3], direction: [i32; 3]) -> Result<bool> {
        self.seen.push((pos, direction));
        Ok(match *state {
            "stone" | "double" => true,
            "bottom" => direction == [0, -1, 0],
            "top" => direction == [0, 1, 0],
            _ => false,
        })
    }
}
fn direction(name: &str) -> [i32; 3] {
    match name {
        "down" => [0, -1, 0],
        "up" => [0, 1, 0],
        "north" => [0, 0, -1],
        "south" => [0, 0, 1],
        "west" => [-1, 0, 0],
        "east" => [1, 0, 0],
        _ => panic!(),
    }
}
#[test]
fn sturdy_face_predicate_preserves_offset_and_direction() {
    let predicate = BlockPredicate::decode(
        &json!({"type":"minecraft:has_sturdy_face","offset":[1,-2,3],"direction":"up"}),
    )
    .unwrap();
    let mut world = World {
        kind: "top",
        seen: Vec::new(),
    };
    assert!(predicate.test(&mut world, [3, 8, 2]).unwrap());
    assert_eq!(world.seen, [([4, 6, 5], [0, 1, 0])]);
    for value in [
        json!({"type":"minecraft:has_sturdy_face","direction":"diagonal"}),
        json!({"type":"minecraft:has_sturdy_face","offset":[17,0,0],"direction":"up"}),
        json!({"type":"minecraft:has_sturdy_face"}),
    ] {
        assert!(BlockPredicate::decode(&value).is_err());
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn sturdy_face_predicate_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SturdyFacePredicateOracle.java");
    std::fs::write(&source, include_str!("java/SturdyFacePredicateOracle.java")).unwrap();
    let run = |c: &mut Command| {
        let o = c.output().unwrap();
        assert!(
            o.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&o.stderr),
            String::from_utf8_lossy(&o.stdout)
        );
        String::from_utf8(o.stdout).unwrap()
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SturdyFacePredicateOracle"));
    let lines: Vec<_> = output.lines().filter(|v| v.starts_with("S ")).collect();
    assert_eq!(lines.len(), 30);
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        let kind = match fields[1] {
            "air" => "air",
            "stone" => "stone",
            "bottom" => "bottom",
            "top" => "top",
            "double" => "double",
            _ => panic!(),
        };
        let dir = fields[2];
        let predicate = BlockPredicate::decode(
            &json!({"type":"minecraft:has_sturdy_face","offset":[1,-2,3],"direction":dir}),
        )
        .unwrap();
        let mut world = World {
            kind,
            seen: Vec::new(),
        };
        assert_eq!(
            predicate.test(&mut world, [3, 8, 2]).unwrap(),
            fields[3].parse::<bool>().unwrap(),
            "{line}"
        );
        assert_eq!(world.seen, [([4, 6, 5], direction(dir))]);
    }
}
