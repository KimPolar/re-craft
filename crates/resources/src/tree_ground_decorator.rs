//! Java 26.3 tree decorator that places state-provider blocks above ground.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
    tree_leaf_update::decorator_positions,
};

pub trait GroundDecorationWorld: DiskWorld {
    /// The cached block-state `isSolidRender`, which differs from block solidity.
    fn ground_solid_render(&mut self, state: &Self::State) -> Result<bool>;
    /// Current MOTION_BLOCKING_NO_LEAVES heightmap value.
    fn ground_heightmap_y(&mut self, x: i32, z: i32) -> Result<i32>;
    /// TreeDecorator.Context.setBlock, including the tree's decoration set.
    fn place_ground_decoration(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
}

pub struct PlaceOnGroundDecorator<S> {
    tries: i32,
    radius: i32,
    height: i32,
    provider: Provider<S>,
}

impl<S: Clone + Eq> PlaceOnGroundDecorator<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let integer = |name: &str, default: i32| -> Result<i32> {
            match value.get(name) {
                None => Ok(default),
                Some(v) => Ok(i32::try_from(
                    v.as_i64()
                        .with_context(|| format!("invalid decorator {name}"))?,
                )?),
            }
        };
        let tries = integer("tries", 128)?;
        let radius = integer("radius", 2)?;
        let height = integer("height", 1)?;
        ensure!(
            tries > 0 && radius >= 0 && height >= 0,
            "invalid ground decorator dimensions"
        );
        Ok(Self {
            tries,
            radius,
            height,
            provider: Provider::load(
                resources,
                value
                    .get("block_state_provider")
                    .context("missing ground decorator provider")?,
                resolve,
            )?,
        })
    }

    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::new();
        self.provider.tags(&mut tags);
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.provider.blocks(&mut blocks);
        blocks
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.provider.needs_survival(name)
    }

    pub fn place<W: GroundDecorationWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        logs: &[[i32; 3]],
        roots: &[[i32; 3]],
    ) -> Result<()> {
        let mut logs = decorator_positions(logs)?;
        let roots = decorator_positions(roots)?;
        // TreeFeature.getLowestTrunkOrRootOfTree selects roots whenever their
        // first Y differs from the logs, even if those roots are higher.
        let mut positions = if roots.is_empty() {
            logs
        } else if !logs.is_empty() && roots[0][1] == logs[0][1] {
            logs.extend(roots);
            logs
        } else {
            roots
        };
        let Some(first) = positions.first().copied() else {
            return Ok(());
        };
        let lowest = first[1];
        positions.retain(|p| p[1] == lowest);
        let (mut min_x, mut max_x, mut min_z, mut max_z) = (first[0], first[0], first[2], first[2]);
        for pos in positions {
            min_x = min_x.min(pos[0]);
            max_x = max_x.max(pos[0]);
            min_z = min_z.min(pos[2]);
            max_z = max_z.max(pos[2]);
        }
        let min = [
            min_x.wrapping_sub(self.radius),
            lowest.wrapping_sub(self.height),
            min_z.wrapping_sub(self.radius),
        ];
        let max = [
            max_x.wrapping_add(self.radius),
            lowest.wrapping_add(self.height),
            max_z.wrapping_add(self.radius),
        ];
        let vine = ResourceId::parse("minecraft:vine")?;
        for _ in 0..self.tries {
            let pos = [
                sample(random, min[0], max[0])?,
                sample(random, min[1], max[1])?,
                sample(random, min[2], max[2])?,
            ];
            let above = [pos[0], pos[1].wrapping_add(1), pos[2]];
            let above_state = world.block(above)?;
            if !world.is_air(&above_state)? && !world.block_is(&above_state, &vine)? {
                continue;
            }
            let ground = world.block(pos)?;
            if !world.ground_solid_render(&ground)?
                || world.ground_heightmap_y(pos[0], pos[2])? > above[1]
            {
                continue;
            }
            let state = self
                .provider
                .state(world, random, above)?
                .context("ground decorator provider returned no state")?;
            world.place_ground_decoration(above, &state)?;
        }
        Ok(())
    }
}

fn sample(random: &mut DecorationRandom, min: i32, max: i32) -> Result<i32> {
    let bound = max.wrapping_sub(min).wrapping_add(1);
    Ok(random
        .next_int_bound(bound)
        .context("invalid decorator coordinate range")?
        .wrapping_add(min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ore_feature::OreWorld;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct State {
        name: String,
        facing: String,
        amount: i32,
    }
    fn state(name: &str) -> State {
        State {
            name: name.into(),
            facing: "north".into(),
            amount: 1,
        }
    }
    #[derive(Default)]
    struct World {
        blocks: BTreeMap<[i32; 3], State>,
        writes: Vec<String>,
    }
    impl OreWorld for World {
        type State = State;
        fn block(&mut self, p: [i32; 3]) -> Result<State> {
            if let Some(s) = self.blocks.get(&p) {
                return Ok(s.clone());
            }
            Ok(if p[1] == 2 {
                state("minecraft:stone")
            } else if p[1] == 3 && (p[0] - p[2]).rem_euclid(5) == 0 {
                state("minecraft:vine")
            } else {
                state("minecraft:air")
            })
        }
        fn block_is(&mut self, s: &State, id: &ResourceId) -> Result<bool> {
            Ok(s.name == format!("{}:{}", id.namespace, id.path))
        }
        fn in_tag(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn is_air(&mut self, s: &State) -> Result<bool> {
            Ok(s.name == "minecraft:air")
        }
        fn set_block(&mut self, _: [i32; 3], _: &State, _: i32) -> Result<bool> {
            unreachable!()
        }
    }
    impl DiskWorld for World {
        fn is_solid(&mut self, _: &State) -> Result<bool> {
            unreachable!()
        }
        fn fluid_is(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn set_disk_block(&mut self, _: [i32; 3], _: &State) -> Result<bool> {
            unreachable!()
        }
        fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
    }
    impl GroundDecorationWorld for World {
        fn ground_solid_render(&mut self, s: &State) -> Result<bool> {
            Ok(s.name == "minecraft:stone")
        }
        fn ground_heightmap_y(&mut self, x: i32, z: i32) -> Result<i32> {
            Ok(if (x * 31 + z).rem_euclid(7) == 0 {
                6
            } else {
                3
            })
        }
        fn place_ground_decoration(&mut self, p: [i32; 3], s: &State) -> Result<()> {
            self.blocks.insert(p, s.clone());
            self.writes.push(format!(
                "{},{},{},{},{}",
                p[0], p[1], p[2], s.facing, s.amount
            ));
            Ok(())
        }
    }
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    fn definition() -> Value {
        json!({"tries":30,"radius":2,"height":1,"block_state_provider":{"type":"minecraft:weighted","entries":[
            {"data":{"id":"minecraft:leaf_litter"},"weight":1},
            {"data":{"id":"minecraft:leaf_litter","properties":{"facing":"east","segment_amount":"3"}},"weight":2}
        ]}})
    }
    fn feature(v: &Value) -> Result<PlaceOnGroundDecorator<State>> {
        PlaceOnGroundDecorator::decode(&mut Data, v, &mut |v| {
            let mut s = state(v["id"].as_str().context("missing state")?);
            if let Some(facing) = v["properties"]["facing"].as_str() {
                s.facing = facing.into();
            }
            if let Some(amount) = v["properties"]["segment_amount"].as_str() {
                s.amount = amount.parse()?;
            }
            Ok(s)
        })
    }
    #[test]
    fn validates_bounds_and_skips_empty_tree_without_random_draws() {
        for (field, value) in [("tries", 0), ("radius", -1), ("height", -1)] {
            let mut config = definition();
            config[field] = json!(value);
            assert!(feature(&config).is_err());
        }
        let mut a = DecorationRandom::new(42);
        let mut b = a.clone();
        feature(&definition())
            .unwrap()
            .place(&mut World::default(), &mut a, &[], &[])
            .unwrap();
        assert_eq!(a.next_long(), b.next_long());
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn placements_and_random_state_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let source = temp.path().join("TreeGroundOracle.java");
        std::fs::write(&source, include_str!("../tests/java/TreeGroundOracle.java")).unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let oracle = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreeGroundOracle")
            .output()
            .unwrap();
        assert!(
            oracle.status.success(),
            "{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let actual = String::from_utf8(oracle.stdout).unwrap();
        let mut expected = Vec::new();
        for seed in [0, 1, -1, 987654321] {
            for mode in 0..5 {
                let mut logs = Vec::new();
                let mut roots = Vec::new();
                if mode != 3 {
                    logs.push([3, 3, -4]);
                }
                if mode == 1 {
                    logs.push([4, 3, -4]);
                    roots.push([3, 3, -3]);
                }
                if mode == 2 {
                    roots.push([5, 4, -4]);
                }
                if mode == 4 {
                    roots.push([5, 2, -4]);
                }
                let mut world = World::default();
                let mut random = DecorationRandom::new(seed);
                feature(&definition())
                    .unwrap()
                    .place(&mut world, &mut random, &logs, &roots)
                    .unwrap();
                expected.push(format!(
                    "G {seed} {mode} {} R {}",
                    world.writes.join(";"),
                    random.next_long()
                ));
            }
        }
        let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("G ")).collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(*actual, expected);
        }
    }
}
