//! Java 26.3 root-system feature, including nested tree placement.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{BlockPredicate, DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
};

pub trait RootSystemWorld: DiskWorld {
    /// WORLD_SURFACE height, the first position above the highest non-air block.
    fn root_world_surface(&mut self, x: i32, z: i32) -> Result<i32>;
    /// BlockState.canSurvive with the active generation world and tags.
    fn root_can_survive(&mut self, state: &Self::State, pos: [i32; 3]) -> Result<bool>;
    /// Apply WorldGenLevel.setBlock with flag 2, including generation effects.
    fn set_root_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
}

enum Replaceable {
    Tag(ResourceId),
    Blocks(Vec<ResourceId>),
}

pub struct RootSystemFeature<S> {
    tree: Value,
    space: i32,
    level_distance: i32,
    deviation: i32,
    radius: i32,
    replaceable: Replaceable,
    dirt: Provider<S>,
    attempts: i32,
    height: i32,
    hanging_radius: i32,
    hanging_span: i32,
    hanging: Provider<S>,
    hanging_attempts: i32,
    water: i32,
    allowed: BlockPredicate,
}

impl<S: Clone + Eq> RootSystemFeature<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing root system type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "root_system",
            "wrong root system type"
        );
        let integer = |name: &str, min: i32, max: i32| -> Result<i32> {
            let n = value
                .get(name)
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .with_context(|| format!("invalid root system {name}"))?;
            ensure!((min..=max).contains(&n), "invalid root system {name}");
            Ok(n)
        };
        let raw = value
            .get("root_replaceable")
            .context("missing root replaceable")?;
        let replaceable = if let Some(tag) = raw.as_str().and_then(|s| s.strip_prefix('#')) {
            Replaceable::Tag(ResourceId::parse(tag)?)
        } else {
            let values = if let Some(s) = raw.as_str() {
                vec![s]
            } else {
                raw.as_array()
                    .context("invalid root replaceable")?
                    .iter()
                    .map(|v| v.as_str().context("invalid root replaceable block"))
                    .collect::<Result<Vec<_>>>()?
            };
            ensure!(values.len() <= 4096, "too many root replaceable blocks");
            Replaceable::Blocks(
                values
                    .into_iter()
                    .map(ResourceId::parse)
                    .collect::<Result<_>>()?,
            )
        };
        Ok(Self {
            tree: value.get("feature").context("missing root tree")?.clone(),
            space: integer("required_vertical_space_for_tree", 1, 64)?,
            level_distance: integer("level_test_distance", 0, 16)?,
            deviation: integer("max_level_deviation", 0, 64)?,
            radius: integer("root_radius", 1, 64)?,
            replaceable,
            dirt: Provider::load(
                resources,
                value
                    .get("root_state_provider")
                    .context("missing root state provider")?,
                resolve,
            )?,
            attempts: integer("root_placement_attempts", 1, 256)?,
            height: integer("root_column_max_height", 1, 4096)?,
            hanging_radius: integer("hanging_root_radius", 1, 64)?,
            hanging_span: integer("hanging_roots_vertical_span", 1, 16)?,
            hanging: Provider::load(
                resources,
                value
                    .get("hanging_root_state_provider")
                    .context("missing hanging root provider")?,
                resolve,
            )?,
            hanging_attempts: integer("hanging_root_placement_attempts", 1, 256)?,
            water: integer("allowed_vertical_water_for_tree", 1, 64)?,
            allowed: BlockPredicate::decode(
                value
                    .get("allowed_tree_position")
                    .context("missing root tree predicate")?,
            )?,
        })
    }
    pub fn tree(&self) -> &Value {
        &self.tree
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = self.allowed.required_tags();
        if let Replaceable::Tag(tag) = &self.replaceable {
            tags.insert(tag.clone());
        }
        self.dirt.tags(&mut tags);
        self.hanging.tags(&mut tags);
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = self.allowed.required_blocks();
        if let Replaceable::Blocks(ids) = &self.replaceable {
            blocks.extend(ids.iter().cloned());
        }
        self.dirt.blocks(&mut blocks);
        self.hanging.blocks(&mut blocks);
        blocks
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.allowed.needs_survival(name)
            || self.dirt.needs_survival(name)
            || self.hanging.needs_survival(name)
    }
    fn space_for_tree<W: RootSystemWorld<State = S>>(
        &self,
        world: &mut W,
        pos: [i32; 3],
    ) -> Result<bool> {
        let water = ResourceId::parse("minecraft:water")?;
        for y in 1..=self.space {
            let p = offset(pos, 0, y, 0);
            let state = world.block(p)?;
            if !world.is_air(&state)?
                && !(y + 1 <= self.water && world.fluid_in_tag(&state, &water)?)
            {
                return Ok(false);
            }
        }
        if self.level_distance > 0 {
            for [dx, dz] in [[0, 1], [-1, 0], [0, -1], [1, 0]] {
                let p = offset(pos, dx * self.level_distance, 0, dz * self.level_distance);
                let below = world.block(offset(p, 0, -self.deviation, 0))?;
                let above = world.block(offset(p, 0, self.deviation, 0))?;
                if world.is_air(&below)? {
                    return Ok(false);
                }
                if !world.is_air(&above)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    pub fn place<W: RootSystemWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
        mut place_tree: impl FnMut(&mut W, &mut DecorationRandom, [i32; 3]) -> Result<bool>,
    ) -> Result<bool> {
        let first = world.block(origin)?;
        if !world.is_air(&first)? {
            return Ok(false);
        }
        let lava = ResourceId::parse("minecraft:lava")?;
        let mut found = None;
        for i in 0..self.height {
            let p = offset(origin, 0, i + 1, 0);
            if world.root_world_surface(p[0], p[2])? < p[1] {
                break;
            }
            if !self.allowed.test(world, p)? || !self.space_for_tree(world, p)? {
                continue;
            }
            let below = world.block(offset(p, 0, -1, 0))?;
            if world.fluid_in_tag(&below, &lava)? || !world.is_solid(&below)? {
                break;
            }
            if place_tree(world, random, p)? {
                found = Some(i);
                break;
            }
        }
        if let Some(height) = found {
            for y in 0..height {
                for _ in 0..self.attempts {
                    let x = random.next_int_bound(self.radius).unwrap()
                        - random.next_int_bound(self.radius).unwrap();
                    let z = random.next_int_bound(self.radius).unwrap()
                        - random.next_int_bound(self.radius).unwrap();
                    let p = offset(origin, x, y, z);
                    let old = world.block(p)?;
                    let replace = match &self.replaceable {
                        Replaceable::Tag(tag) => world.in_tag(&old, tag)?,
                        Replaceable::Blocks(ids) => {
                            let mut found = false;
                            for id in ids {
                                if world.block_is(&old, id)? {
                                    found = true;
                                    break;
                                }
                            }
                            found
                        }
                    };
                    if replace {
                        let state = self
                            .dirt
                            .state(world, random, p)?
                            .context("root state provider returned no state")?;
                        world.set_root_block(p, &state)?;
                    }
                }
            }
            for _ in 0..self.hanging_attempts {
                let x = random.next_int_bound(self.hanging_radius).unwrap()
                    - random.next_int_bound(self.hanging_radius).unwrap();
                let y = random.next_int_bound(self.hanging_span).unwrap()
                    - random.next_int_bound(self.hanging_span).unwrap();
                let z = random.next_int_bound(self.hanging_radius).unwrap()
                    - random.next_int_bound(self.hanging_radius).unwrap();
                let p = offset(origin, x, y, z);
                let old = world.block(p)?;
                if !world.is_air(&old)? {
                    continue;
                }
                let state = self
                    .hanging
                    .state(world, random, p)?
                    .context("hanging root provider returned no state")?;
                if !world.root_can_survive(&state, p)? {
                    continue;
                }
                let above = world.block(offset(p, 0, 1, 0))?;
                if world.face_sturdy(&above, p, [0, -1, 0])? {
                    world.set_root_block(p, &state)?;
                }
            }
        }
        Ok(true)
    }
}

fn offset(p: [i32; 3], x: i32, y: i32, z: i32) -> [i32; 3] {
    [
        p[0].wrapping_add(x),
        p[1].wrapping_add(y),
        p[2].wrapping_add(z),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ore_feature::OreWorld;
    use serde_json::json;
    use std::collections::BTreeMap;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    fn resolve(v: &Value) -> Result<String> {
        Ok(v.as_str()
            .or_else(|| v.get("id").and_then(Value::as_str))
            .context("missing block")?
            .into())
    }
    fn definition() -> Value {
        json!({"type":"minecraft:root_system","feature":{"feature":"test:tree","placement":[]},
            "required_vertical_space_for_tree":3,"level_test_distance":0,"max_level_deviation":0,
            "root_radius":3,"root_replaceable":["minecraft:stone"],"root_state_provider":{"id":"minecraft:rooted_dirt"},
            "root_placement_attempts":8,"root_column_max_height":10,"hanging_root_radius":3,
            "hanging_roots_vertical_span":2,"hanging_root_state_provider":{"id":"minecraft:hanging_roots"},
            "hanging_root_placement_attempts":12,"allowed_vertical_water_for_tree":2,
            "allowed_tree_position":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"}})
    }
    struct World {
        mode: i32,
        blocks: BTreeMap<[i32; 3], String>,
        events: Vec<String>,
    }
    impl OreWorld for World {
        type State = String;
        fn block(&mut self, p: [i32; 3]) -> Result<String> {
            if let Some(s) = self.blocks.get(&p) {
                return Ok(s.clone());
            }
            let name = if self.mode == 4 && p == [0, 3, 0] {
                "stone"
            } else if self.mode == 1 && p == [0, 7, 0] {
                "water"
            } else if self.mode == 2 && p == [0, 8, 0] {
                "stone"
            } else if p[1] <= 2 || p[1] == 4 || p[1] == 5 {
                "stone"
            } else {
                "air"
            };
            Ok(format!("minecraft:{name}"))
        }
        fn block_is(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
            Ok(*s == format!("{}:{}", id.namespace, id.path))
        }
        fn in_tag(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn is_air(&mut self, s: &String) -> Result<bool> {
            Ok(s == "minecraft:air")
        }
        fn set_block(&mut self, p: [i32; 3], s: &String, flags: i32) -> Result<bool> {
            assert_eq!(flags, 2);
            self.blocks.insert(p, s.clone());
            self.events
                .push(format!("W{},{},{},{}", p[0], p[1], p[2], s));
            Ok(true)
        }
    }
    impl DiskWorld for World {
        fn is_solid(&mut self, s: &String) -> Result<bool> {
            Ok(s == "minecraft:stone" || s == "minecraft:rooted_dirt")
        }
        fn fluid_is(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
            self.fluid_in_tag(s, id)
        }
        fn fluid_in_tag(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
            Ok(*s == format!("{}:{}", id.namespace, id.path))
        }
        fn set_disk_block(&mut self, _: [i32; 3], _: &String) -> Result<bool> {
            unreachable!()
        }
        fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
        fn face_sturdy(&mut self, s: &String, _: [i32; 3], _: [i32; 3]) -> Result<bool> {
            self.is_solid(s)
        }
    }
    impl RootSystemWorld for World {
        fn root_world_surface(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(6)
        }
        fn root_can_survive(&mut self, _: &String, p: [i32; 3]) -> Result<bool> {
            let above = self.block(offset(p, 0, 1, 0))?;
            self.is_solid(&above)
        }
        fn set_root_block(&mut self, p: [i32; 3], state: &String) -> Result<bool> {
            self.set_block(p, state, 2)
        }
    }
    #[test]
    fn rejects_out_of_range_configuration() {
        for (key, value) in [
            ("root_radius", 0),
            ("root_column_max_height", 4097),
            ("hanging_roots_vertical_span", 17),
        ] {
            let mut v = definition();
            v[key] = json!(value);
            assert!(RootSystemFeature::decode(&mut Data, &v, &mut resolve).is_err());
        }
    }
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn root_placement_and_random_state_match_java() {
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
        let source = temp.path().join("RootSystemOracle.java");
        let helper = temp.path().join("VegetationPatchOracle.java");
        std::fs::write(&source, include_str!("../tests/java/RootSystemOracle.java")).unwrap();
        std::fs::write(
            &helper,
            include_str!("../tests/java/VegetationPatchOracle.java"),
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
            .arg(&source)
            .arg(&helper));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("RootSystemOracle"));
        let mut expected = Vec::new();
        for mode in 0..5 {
            for seed in [0, 1, -1, 42, 987654321] {
                let feature =
                    RootSystemFeature::decode(&mut Data, &definition(), &mut resolve).unwrap();
                let mut world = World {
                    mode,
                    blocks: BTreeMap::new(),
                    events: Vec::new(),
                };
                let mut random = DecorationRandom::new(seed);
                let result = feature
                    .place(&mut world, &mut random, [0, 3, 0], |w, r, p| {
                        let n = r.next_int_bound(3).unwrap();
                        w.events.push(format!("P{},{},{},{}", p[0], p[1], p[2], n));
                        Ok(mode != 3 && n != 0)
                    })
                    .unwrap();
                expected.push(format!(
                    "R {mode} {seed} {result} {} X {}",
                    world.events.join(";"),
                    random.next_long()
                ));
            }
        }
        let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("R ")).collect();
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(&expected) {
            assert_eq!(*a, e);
        }
    }
}
