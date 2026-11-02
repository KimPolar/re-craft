//! Java 26.3 VinesFeature placement.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{multiface_growth::Face, packs::ResourceId};

pub trait VinesWorld {
    type State: Clone;
    fn vines_empty(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Match MultifaceBlock.canAttachTo: support or collision shape covers
    /// the neighbor's face opposite `direction`.
    fn vines_can_attach(&mut self, neighbor: [i32; 3], direction: Face) -> Result<bool>;
    fn vine_with_face(&mut self, state: &Self::State, direction: Face) -> Result<Self::State>;
    /// Apply WorldGenLevel.setBlock with flags 2.
    fn set_vines(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
}

pub struct VinesFeature<S> {
    default: S,
}
impl<S: Clone> VinesFeature<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing vines type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "vines",
            "wrong vines feature type"
        );
        Ok(Self {
            default: resolve(&json!("minecraft:vine"))?,
        })
    }
    pub fn place<W: VinesWorld<State = S>>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        if !world.vines_empty(pos)? {
            return Ok(false);
        }
        for direction in [Face::Up, Face::North, Face::South, Face::West, Face::East] {
            if world.vines_can_attach(direction.offset(pos), direction)? {
                let state = world.vine_with_face(&self.default, direction)?;
                world.set_vines(pos, &state)?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::DecorationRandom;
    use std::collections::BTreeMap;

    struct World {
        kind: i32,
        mask: i32,
        occupied: bool,
        writes: Vec<String>,
    }
    impl VinesWorld for World {
        type State = BTreeMap<String, String>;
        fn vines_empty(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(!self.occupied)
        }
        fn vines_can_attach(&mut self, _: [i32; 3], direction: Face) -> Result<bool> {
            let index = match direction {
                Face::Up => 0,
                Face::North => 1,
                Face::South => 2,
                Face::West => 3,
                Face::East => 4,
                _ => unreachable!(),
            };
            Ok(self.mask & (1 << index) != 0
                && match self.kind {
                    0 | 3 | 4 | 5 => true,
                    1 => direction == Face::Up,
                    2 | 6 => false,
                    _ => unreachable!(),
                })
        }
        fn vine_with_face(&mut self, state: &Self::State, direction: Face) -> Result<Self::State> {
            let mut state = state.clone();
            let face = match direction {
                Face::Up => "up",
                Face::North => "north",
                Face::South => "south",
                Face::West => "west",
                Face::East => "east",
                _ => unreachable!(),
            };
            state.insert(face.into(), "true".into());
            Ok(state)
        }
        fn set_vines(&mut self, _: [i32; 3], state: &Self::State) -> Result<bool> {
            self.writes.push(
                state
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            Ok(true)
        }
    }
    fn feature() -> VinesFeature<BTreeMap<String, String>> {
        VinesFeature::decode(&json!({"type":"minecraft:vines"}), &mut |_| {
            Ok(["up", "north", "south", "west", "east"]
                .into_iter()
                .map(|s| (s.into(), "false".into()))
                .collect())
        })
        .unwrap()
    }
    #[test]
    fn first_supported_face_is_selected() {
        let mut world = World {
            kind: 0,
            mask: 0b10110,
            occupied: false,
            writes: Vec::new(),
        };
        assert!(feature().place(&mut world, [2, 5, -3]).unwrap());
        assert_eq!(
            world.writes,
            ["east=false,north=true,south=false,up=false,west=false"]
        );
    }
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn vines_feature_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let source = temp.path().join("VinesFeatureOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/VinesFeatureOracle.java"),
        )
        .unwrap();
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
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("VinesFeatureOracle"));
        let mut expected = Vec::new();
        for kind in 0..7 {
            for mask in 0..32 {
                for occupied in [false, true] {
                    let mut world = World {
                        kind,
                        mask,
                        occupied,
                        writes: Vec::new(),
                    };
                    let mut random = DecorationRandom::new(42);
                    let result = feature().place(&mut world, [2, 5, -3]).unwrap();
                    expected.push(format!(
                        "V {kind} {mask} {occupied} {result} {} R {}",
                        world.writes.join(";"),
                        random.next_long()
                    ));
                }
            }
        }
        let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("V ")).collect();
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(&expected) {
            assert_eq!(*a, e);
        }
    }
}
