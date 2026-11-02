//! Java 26.3 huge mushroom feature; the world owns block update effects.
use std::{collections::BTreeSet, sync::OnceLock};

use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    disk_feature::{BlockPredicate, DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
};

pub trait MushroomWorld: DiskWorld {
    fn mushroom_min_y(&self) -> i32;
    /// Java's inclusive maximum build Y.
    fn mushroom_max_y(&self) -> i32;
    /// Change the cap faces only if the block has all requested properties.
    fn mushroom_cap_faces(
        &mut self,
        state: Self::State,
        faces: &[(&str, bool)],
    ) -> Result<Self::State>;
    /// Apply Feature.setBlock / WorldGenLevel.setBlockAndUpdate, including side effects.
    fn write_mushroom_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
}

pub struct HugeMushroomFeature<S> {
    red: bool,
    cap: Provider<S>,
    stem: Provider<S>,
    radius: i32,
    can_place_on: BlockPredicate,
}

impl<S: Clone + Eq> HugeMushroomFeature<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing mushroom feature type")?,
        )?;
        let red = match (kind.namespace.as_str(), kind.path.as_str()) {
            ("minecraft", "huge_brown_mushroom") => false,
            ("minecraft", "huge_red_mushroom") => true,
            _ => anyhow::bail!("unsupported mushroom feature"),
        };
        let radius = match value.get("foliage_radius") {
            None => 2,
            Some(v) => i32::try_from(v.as_i64().context("invalid mushroom radius")?)?,
        };
        ensure!((0..=32).contains(&radius), "unsupported mushroom radius");
        Ok(Self {
            red,
            cap: Provider::load(
                resources,
                value
                    .get("cap_provider")
                    .context("missing mushroom cap provider")?,
                resolve,
            )?,
            stem: Provider::load(
                resources,
                value
                    .get("stem_provider")
                    .context("missing mushroom stem provider")?,
                resolve,
            )?,
            radius,
            can_place_on: BlockPredicate::decode(
                value
                    .get("can_place_on")
                    .context("missing mushroom ground predicate")?,
            )?,
        })
    }

    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = self.can_place_on.required_tags();
        tags.insert(ResourceId::parse("minecraft:leaves").unwrap());
        tags.insert(ResourceId::parse("minecraft:replaceable_by_mushrooms").unwrap());
        self.cap.tags(&mut tags);
        self.stem.tags(&mut tags);
        tags
    }

    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = self.can_place_on.required_blocks();
        self.cap.blocks(&mut blocks);
        self.stem.blocks(&mut blocks);
        blocks
    }

    pub fn needs_survival(&self, name: &str) -> bool {
        self.can_place_on.needs_survival(name)
            || self.cap.needs_survival(name)
            || self.stem.needs_survival(name)
    }

    pub fn place<W: MushroomWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let mut height = random
            .next_int_bound(3)
            .context("invalid mushroom height")?
            + 4;
        if random
            .next_int_bound(12)
            .context("invalid mushroom height")?
            == 0
        {
            height *= 2;
        }
        if origin[1] < world.mushroom_min_y().wrapping_add(1)
            || origin[1].wrapping_add(height).wrapping_add(1) > world.mushroom_max_y()
            || !self.can_place_on.test(world, offset(origin, 0, -1, 0))?
        {
            return Ok(false);
        }
        let leaves = ResourceId::parse("minecraft:leaves")?;
        for y in 0..=height {
            // Java calls getTreeRadiusForHeight(-1, -1, radius, y).
            let radius = if self.red || y <= 3 { 0 } else { self.radius };
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let current = world.block(offset(origin, x, y, z))?;
                    if !world.is_air(&current)? && !world.in_tag(&current, &leaves)? {
                        return Ok(false);
                    }
                }
            }
        }
        if self.red {
            for y in height - 3..=height {
                let radius = if y < height {
                    self.radius
                } else {
                    self.radius - 1
                };
                for x in -radius..=radius {
                    for z in -radius..=radius {
                        let x_edge = x == -radius || x == radius;
                        let z_edge = z == -radius || z == radius;
                        if y < height && x_edge == z_edge {
                            continue;
                        }
                        let inner = self.radius - 2;
                        let state = self
                            .cap
                            .state(world, random, origin)?
                            .context("empty mushroom cap provider")?;
                        let state = world.mushroom_cap_faces(
                            state,
                            &[
                                ("up", y >= height - 1),
                                ("west", x < -inner),
                                ("east", x > inner),
                                ("north", z < -inner),
                                ("south", z > inner),
                            ],
                        )?;
                        self.write(world, offset(origin, x, y, z), &state)?;
                    }
                }
            }
        } else {
            for x in -self.radius..=self.radius {
                for z in -self.radius..=self.radius {
                    let west = x == -self.radius;
                    let east = x == self.radius;
                    let north = z == -self.radius;
                    let south = z == self.radius;
                    let x_edge = west || east;
                    let z_edge = north || south;
                    if x_edge && z_edge {
                        continue;
                    }
                    let state = self
                        .cap
                        .state(world, random, origin)?
                        .context("empty mushroom cap provider")?;
                    let state = world.mushroom_cap_faces(
                        state,
                        &[
                            ("west", west || z_edge && x == 1 - self.radius),
                            ("east", east || z_edge && x == self.radius - 1),
                            ("north", north || x_edge && z == 1 - self.radius),
                            ("south", south || x_edge && z == self.radius - 1),
                        ],
                    )?;
                    self.write(world, offset(origin, x, height, z), &state)?;
                }
            }
        }
        for y in 0..height {
            let state = self
                .stem
                .state(world, random, origin)?
                .context("empty mushroom stem provider")?;
            self.write(world, offset(origin, 0, y, 0), &state)?;
        }
        Ok(true)
    }

    fn write<W: MushroomWorld<State = S>>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        state: &S,
    ) -> Result<()> {
        let old = world.block(pos)?;
        static REPLACEABLE: OnceLock<ResourceId> = OnceLock::new();
        let replaceable = REPLACEABLE
            .get_or_init(|| ResourceId::parse("minecraft:replaceable_by_mushrooms").unwrap());
        if world.is_air(&old)? || world.in_tag(&old, replaceable)? {
            world.write_mushroom_block(pos, state)?;
        }
        Ok(())
    }
}

fn offset(pos: [i32; 3], x: i32, y: i32, z: i32) -> [i32; 3] {
    [
        pos[0].wrapping_add(x),
        pos[1].wrapping_add(y),
        pos[2].wrapping_add(z),
    ]
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
        faces: BTreeMap<String, String>,
    }
    fn state(name: &str) -> State {
        State {
            name: name.into(),
            faces: if name == "cap" {
                ["up", "down", "west", "east", "north", "south"]
                    .into_iter()
                    .map(|v| (v.into(), "true".into()))
                    .collect()
            } else {
                BTreeMap::new()
            },
        }
    }
    #[derive(Default)]
    struct World {
        blocks: BTreeMap<[i32; 3], State>,
        writes: Vec<([i32; 3], State)>,
    }
    impl OreWorld for World {
        type State = State;
        fn block(&mut self, pos: [i32; 3]) -> Result<State> {
            Ok(self
                .blocks
                .get(&pos)
                .cloned()
                .unwrap_or_else(|| state("air")))
        }
        fn block_is(&mut self, block: &State, id: &ResourceId) -> Result<bool> {
            Ok(block.name == id.path)
        }
        fn in_tag(&mut self, block: &State, tag: &ResourceId) -> Result<bool> {
            Ok(match tag.path.as_str() {
                "leaves" => block.name == "leaves",
                "replaceable_by_mushrooms" => block.name == "leaves",
                _ => false,
            })
        }
        fn is_air(&mut self, block: &State) -> Result<bool> {
            Ok(block.name == "air")
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
    impl MushroomWorld for World {
        fn mushroom_min_y(&self) -> i32 {
            0
        }
        fn mushroom_max_y(&self) -> i32 {
            63
        }
        fn mushroom_cap_faces(
            &mut self,
            mut state: State,
            faces: &[(&str, bool)],
        ) -> Result<State> {
            if faces.iter().all(|(key, _)| state.faces.contains_key(*key)) {
                for (key, value) in faces {
                    state.faces.insert((*key).into(), value.to_string());
                }
            }
            Ok(state)
        }
        fn write_mushroom_block(&mut self, pos: [i32; 3], state: &State) -> Result<()> {
            self.blocks.insert(pos, state.clone());
            self.writes.push((pos, state.clone()));
            Ok(())
        }
    }
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    fn feature(red: bool) -> HugeMushroomFeature<State> {
        HugeMushroomFeature::decode(&mut Data, &json!({
            "type":if red {"minecraft:huge_red_mushroom"} else {"minecraft:huge_brown_mushroom"},
            "foliage_radius":3,
            "cap_provider":{"id":"cap"}, "stem_provider":{"id":"stem"},
            "can_place_on":{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt"]}
        }), &mut |v| Ok(state(v["id"].as_str().unwrap()))).unwrap()
    }
    #[test]
    fn brown_cap_then_trunk_and_java_random_consumption() {
        let mut world = World::default();
        world.blocks.insert([0, 4, 0], state("dirt"));
        let mut random = DecorationRandom::new(42);
        let mut expected = DecorationRandom::new(42);
        let mut height = expected.next_int_bound(3).unwrap() + 4;
        if expected.next_int_bound(12).unwrap() == 0 {
            height *= 2;
        }
        assert!(
            feature(false)
                .place(&mut world, &mut random, [0, 5, 0])
                .unwrap()
        );
        assert_eq!(world.writes.len(), (49 - 4 + height) as usize);
        assert_eq!(world.writes[0].0, [-3, 5 + height, -2]);
        assert_eq!(world.writes.last().unwrap().0, [0, 4 + height, 0]);
        assert_eq!(world.blocks[&[-3, 5 + height, -2]].faces["west"], "true");
        assert_eq!(world.blocks[&[-3, 5 + height, -2]].faces["north"], "true");
        assert_eq!(random.next_int(), expected.next_int());
    }
    #[test]
    fn unsuitable_ground_and_clearance_leave_world_unchanged() {
        let mut world = World::default();
        assert!(
            !feature(false)
                .place(&mut world, &mut DecorationRandom::new(42), [0, 5, 0])
                .unwrap()
        );
        world.blocks.insert([0, 4, 0], state("dirt"));
        world.blocks.insert([0, 6, 0], state("stone"));
        assert!(
            !feature(true)
                .place(&mut world, &mut DecorationRandom::new(42), [0, 5, 0])
                .unwrap()
        );
        assert!(world.writes.is_empty());
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn placements_and_random_state_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
        let source = temp.path().join("HugeMushroomOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/HugeMushroomOracle.java"),
        )
        .unwrap();
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
            .arg("HugeMushroomOracle")
            .output()
            .unwrap();
        assert!(
            oracle.status.success(),
            "{}",
            String::from_utf8_lossy(&oracle.stderr)
        );
        let actual = String::from_utf8(oracle.stdout).unwrap();
        let mut expected = Vec::new();
        for red in [false, true] {
            for seed in [0, 1, -1, 987654321] {
                for mode in 0..5 {
                    let mut world = World::default();
                    if mode != 1 {
                        world.blocks.insert([3, 2, -4], state("dirt"));
                    }
                    if mode == 2 {
                        world.blocks.insert([3, 4, -4], state("stone"));
                    }
                    if mode == 3 {
                        world.blocks.insert([4, 7, -4], state("leaves"));
                    }
                    if mode == 4 {
                        world.blocks.insert([4, 7, -4], state("stone"));
                    }
                    let mut random = DecorationRandom::new(seed);
                    let placed = feature(red)
                        .place(&mut world, &mut random, [3, 3, -4])
                        .unwrap();
                    let writes = world
                        .writes
                        .iter()
                        .map(|(pos, state)| {
                            let faces = if state.name == "stem" {
                                "-".into()
                            } else {
                                ["west", "east", "north", "south", "up", "down"]
                                    .iter()
                                    .map(|name| state.faces[*name].as_str())
                                    .collect::<Vec<_>>()
                                    .join(",")
                            };
                            format!("{},{},{},{},{}", pos[0], pos[1], pos[2], state.name, faces)
                        })
                        .collect::<Vec<_>>()
                        .join(";");
                    expected.push(format!(
                        "M {red} {seed} {mode} {placed} {writes} R {}",
                        random.next_long()
                    ));
                }
            }
        }
        let actual: Vec<_> = actual
            .lines()
            .filter(|line| line.starts_with("M "))
            .collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(*actual, expected);
        }
    }
}
