//! Java 26.3 block-pile placement in a caller-owned generation draft.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
};

pub trait BlockPileWorld: DiskWorld {
    fn min_y(&self) -> i32;
    fn is_dirt_path(&mut self, state: &Self::State) -> Result<bool>;
    fn pile_sturdy_up(&mut self, state: &Self::State, pos: [i32; 3]) -> Result<bool> {
        self.face_sturdy(state, pos, [0, 1, 0])
    }
    /// Apply the feature's flag-260 setBlock, including generation effects.
    fn set_pile_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
}

pub struct BlockPileFeature<S> {
    provider: Provider<S>,
}

impl<S: Clone + Eq> BlockPileFeature<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("block pile type missing")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "block_pile",
            "wrong block pile feature type"
        );
        Ok(Self {
            provider: Provider::load(
                resources,
                value
                    .get("state_provider")
                    .context("block pile state provider missing")?,
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

    pub fn place<W: BlockPileWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        if origin[1] < world.min_y().wrapping_add(5) {
            return Ok(false);
        }
        let radius_x = 2 + random.next_int_bound(2).expect("positive bound");
        let radius_z = 2 + random.next_int_bound(2).expect("positive bound");
        // BlockPos.betweenClosed visits X first, then Y, then Z.
        for dz in -radius_z..=radius_z {
            for dy in 0..=1 {
                for dx in -radius_x..=radius_x {
                    let distance = (dx * dx + dz * dz) as f32;
                    let threshold = random.next_float() * 10.0 - random.next_float() * 6.0;
                    if distance > threshold && f64::from(random.next_float()) >= 0.031 {
                        continue;
                    }
                    let pos = [
                        origin[0].wrapping_add(dx),
                        origin[1].wrapping_add(dy),
                        origin[2].wrapping_add(dz),
                    ];
                    let current = world.block(pos)?;
                    if !world.is_air(&current)? {
                        continue;
                    }
                    let below = [pos[0], pos[1].wrapping_sub(1), pos[2]];
                    let support = world.block(below)?;
                    let sturdy = if world.is_dirt_path(&support)? {
                        random.next_bool()
                    } else {
                        world.pile_sturdy_up(&support, below)?
                    };
                    if !sturdy {
                        continue;
                    }
                    let state = self
                        .provider
                        .state(world, random, pos)?
                        .context("block pile provider returned no state")?;
                    world.set_pile_block(pos, &state)?;
                }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ore_feature::OreWorld, packs::ResourceProvider};
    use std::collections::BTreeMap;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    struct World {
        mode: i32,
        blocks: BTreeMap<[i32; 3], String>,
        writes: Vec<([i32; 3], String)>,
    }
    impl OreWorld for World {
        type State = String;
        fn block(&mut self, pos: [i32; 3]) -> Result<String> {
            if let Some(state) = self.blocks.get(&pos) {
                return Ok(state.clone());
            }
            let [x, y, z] = pos;
            if y < 8 {
                if self.mode == 2 && y == 7 && (x + z).rem_euclid(4) == 0 {
                    return Ok("air".into());
                }
                if matches!(self.mode, 1 | 3) && y == 7 && (x * 3 + z).rem_euclid(3) == 0 {
                    return Ok("dirt_path".into());
                }
                return Ok("stone".into());
            }
            if self.mode == 3 && y == 8 && (x + z * 2).rem_euclid(5) == 0 {
                return Ok("stone".into());
            }
            Ok("air".into())
        }
        fn block_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
            anyhow::bail!("unexpected block comparison")
        }
        fn in_tag(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
            anyhow::bail!("unexpected tag lookup")
        }
        fn is_air(&mut self, state: &String) -> Result<bool> {
            Ok(state == "air")
        }
        fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<bool> {
            anyhow::bail!("unexpected set_block")
        }
    }
    impl DiskWorld for World {
        fn is_solid(&mut self, _: &String) -> Result<bool> {
            anyhow::bail!("unexpected solidity")
        }
        fn fluid_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
            anyhow::bail!("unexpected fluid")
        }
        fn set_disk_block(&mut self, _: [i32; 3], _: &String) -> Result<bool> {
            anyhow::bail!("unexpected disk write")
        }
        fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
            anyhow::bail!("unexpected postprocessing")
        }
        fn face_sturdy(&mut self, state: &String, _: [i32; 3], _: [i32; 3]) -> Result<bool> {
            Ok(state == "stone" || state.starts_with("hay:"))
        }
        fn with_provider_direction(&mut self, state: String, dir: [i32; 3]) -> Result<String> {
            ensure!(state == "hay:y", "unexpected provider state");
            let axis = if dir[0] != 0 {
                "x"
            } else if dir[1] != 0 {
                "y"
            } else {
                "z"
            };
            Ok(format!("hay:{axis}"))
        }
    }
    impl BlockPileWorld for World {
        fn min_y(&self) -> i32 {
            -16
        }
        fn is_dirt_path(&mut self, state: &String) -> Result<bool> {
            Ok(state == "dirt_path")
        }
        fn set_pile_block(&mut self, pos: [i32; 3], state: &String) -> Result<()> {
            self.blocks.insert(pos, state.clone());
            self.writes.push((pos, state.clone()));
            Ok(())
        }
    }
    fn feature() -> Result<BlockPileFeature<String>> {
        BlockPileFeature::decode(
            &mut Data,
            &serde_json::json!({"type":"minecraft:block_pile","state_provider":{"type":"minecraft:rotated","state":{"id":"minecraft:hay_block","properties":{"axis":"y"}}}}),
            &mut |_| Ok("hay:y".into()),
        )
    }

    #[test]
    fn pile_below_minimum_height_preserves_random() -> Result<()> {
        let mut world = World {
            mode: 0,
            blocks: BTreeMap::new(),
            writes: vec![],
        };
        let mut random = DecorationRandom::new(42);
        let mut original = random.clone();
        assert!(!feature()?.place(&mut world, &mut random, [8, -12, 8])?);
        assert!(world.writes.is_empty());
        assert_eq!(random.next_long(), original.next_long());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
    fn pile_placement_and_random_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home missing")?
            .join("bin");
        let source = temp.path().join("BlockPileOracle.java");
        std::fs::write(
            &source,
            include_str!("../../session/tests/java/BlockPileOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("BlockPileOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let mut cases = Vec::<(i32, i64, i64, usize, Vec<([i32; 3], String)>)>::new();
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["CASE", mode, seed, placed, next, count] => {
                    assert_eq!(*placed, "true");
                    cases.push((
                        mode.parse()?,
                        seed.parse()?,
                        next.parse()?,
                        count.parse()?,
                        Vec::new(),
                    ));
                }
                ["WRITE", x, y, z, axis, flags] => {
                    assert_eq!(*flags, "260");
                    cases
                        .last_mut()
                        .context("write without case")?
                        .4
                        .push(([x.parse()?, y.parse()?, z.parse()?], format!("hay:{axis}")));
                }
                _ => {}
            }
        }
        assert_eq!(cases.len(), 20);
        for (mode, seed, next, count, expected) in cases {
            assert_eq!(expected.len(), count);
            let mut world = World {
                mode,
                blocks: BTreeMap::new(),
                writes: vec![],
            };
            let mut random = DecorationRandom::new(seed);
            assert!(feature()?.place(&mut world, &mut random, [8, 8, 8])?);
            assert_eq!(world.writes, expected, "mode {mode} seed {seed}");
            assert_eq!(random.next_long(), next, "mode {mode} seed {seed}");
        }
        Ok(())
    }
}
