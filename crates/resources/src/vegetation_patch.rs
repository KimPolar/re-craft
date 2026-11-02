//! Java 26.3 vegetation patches, including waterlogged patches.
use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::{ResourceId, ResourceProvider},
    placed_feature::IntProvider,
    tree_leaf_update::hash_position_order,
};
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub trait VegetationPatchWorld: DiskWorld {
    fn same_block(&mut self, a: &Self::State, b: &Self::State) -> Result<bool>;
    fn patch_face_sturdy(
        &mut self,
        state: &Self::State,
        pos: [i32; 3],
        direction: [i32; 3],
    ) -> Result<bool>;
    /// Apply WorldGenLevel.setBlock with flags 2, or fail if effects are unknown.
    fn set_patch_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
    /// Java's trySetValue(WATERLOGGED, true); unchanged when absent.
    fn patch_waterlogged(&mut self, state: Self::State) -> Result<Option<Self::State>>;
}

enum Replaceable {
    Tag(ResourceId),
    Blocks(Vec<ResourceId>),
}

pub struct VegetationPatch<S> {
    replaceable: Replaceable,
    ground: Provider<S>,
    water: Option<S>,
    direction: i32,
    depth: IntProvider,
    bottom_chance: f32,
    vertical_range: i32,
    vegetation_chance: f32,
    radius: IntProvider,
    edge_chance: f32,
    vegetation: Value,
}

impl<S: Clone + Eq> VegetationPatch<S> {
    pub fn decode(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing vegetation patch type")?,
        )?;
        let waterlogged = match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "vegetation_patch") => false,
            ("minecraft", "waterlogged_vegetation_patch") => true,
            _ => anyhow::bail!("unsupported vegetation patch"),
        };
        let raw = value
            .get("replaceable")
            .context("missing patch replaceable blocks")?;
        let replaceable = if let Some(tag) = raw.as_str().and_then(|v| v.strip_prefix('#')) {
            Replaceable::Tag(ResourceId::parse(tag)?)
        } else {
            let entries: Vec<&str> = if let Some(s) = raw.as_str() {
                vec![s]
            } else {
                raw.as_array()
                    .context("invalid patch replaceable blocks")?
                    .iter()
                    .map(|v| v.as_str().context("invalid patch block id"))
                    .collect::<Result<_>>()?
            };
            Replaceable::Blocks(
                entries
                    .into_iter()
                    .map(ResourceId::parse)
                    .collect::<Result<_>>()?,
            )
        };
        let direction = match value.get("surface").and_then(Value::as_str) {
            Some("floor") => -1,
            Some("ceiling") => 1,
            _ => anyhow::bail!("invalid patch surface"),
        };
        let depth = IntProvider::parse(value.get("depth").context("missing patch depth")?, 0)?;
        let (lo, hi) = depth.bounds();
        ensure!(lo >= 1 && hi <= 128, "invalid patch depth");
        let radius =
            IntProvider::parse(value.get("xz_radius").context("missing patch radius")?, 0)?;
        let (lo, hi) = radius.bounds();
        ensure!(lo >= -1 && hi <= 64, "unsupported patch radius");
        let vertical_range = i32::try_from(
            value
                .get("vertical_range")
                .and_then(Value::as_i64)
                .context("invalid patch vertical range")?,
        )?;
        ensure!(
            (1..=256).contains(&vertical_range),
            "invalid patch vertical range"
        );
        let chance = |name: &str| -> Result<f32> {
            let v = value
                .get(name)
                .and_then(Value::as_f64)
                .with_context(|| format!("invalid patch {name}"))? as f32;
            ensure!(
                v.is_finite() && (0.0..=1.0).contains(&v),
                "invalid patch {name}"
            );
            Ok(v)
        };
        Ok(Self {
            replaceable,
            ground: Provider::load(
                resources,
                value
                    .get("ground_state")
                    .context("missing patch ground state")?,
                resolve,
            )?,
            water: waterlogged
                .then(|| resolve(&json!({"id":"minecraft:water"})))
                .transpose()?,
            direction,
            depth,
            bottom_chance: chance("extra_bottom_block_chance")?,
            vertical_range,
            vegetation_chance: chance("vegetation_chance")?,
            radius,
            edge_chance: chance("extra_edge_column_chance")?,
            vegetation: value
                .get("vegetation_feature")
                .context("missing patch vegetation")?
                .clone(),
        })
    }
    pub fn vegetation(&self) -> &Value {
        &self.vegetation
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::new();
        if let Replaceable::Tag(tag) = &self.replaceable {
            tags.insert(tag.clone());
        }
        self.ground.tags(&mut tags);
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        if let Replaceable::Blocks(ids) = &self.replaceable {
            blocks.extend(ids.iter().cloned());
        }
        self.ground.blocks(&mut blocks);
        if self.water.is_some() {
            blocks.insert(ResourceId::parse("minecraft:water").unwrap());
        }
        blocks
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.ground.needs_survival(name)
    }

    pub fn place<W: VegetationPatchWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
        mut place_vegetation: impl FnMut(&mut W, &mut DecorationRandom, [i32; 3]) -> Result<bool>,
    ) -> Result<bool> {
        let radius_x = self.radius.sample(random) + 1;
        let radius_z = self.radius.sample(random) + 1;
        let mut ground = Vec::new();
        for x in -radius_x..=radius_x {
            for z in -radius_z..=radius_z {
                let xe = x == -radius_x || x == radius_x;
                let ze = z == -radius_z || z == radius_z;
                if xe && ze {
                    continue;
                }
                if (xe || ze) && (self.edge_chance == 0.0 || random.next_float() > self.edge_chance)
                {
                    continue;
                }
                let mut p = offset(origin, x, 0, z);
                let mut steps = 0;
                loop {
                    let state = world.block(p)?;
                    if !world.is_air(&state)? || steps >= self.vertical_range {
                        break;
                    }
                    p = offset(p, 0, self.direction, 0);
                    steps += 1;
                }
                steps = 0;
                loop {
                    let state = world.block(p)?;
                    if world.is_air(&state)? || steps >= self.vertical_range {
                        break;
                    }
                    p = offset(p, 0, -self.direction, 0);
                    steps += 1;
                }
                let ground_pos = offset(p, 0, self.direction, 0);
                let surface = world.block(ground_pos)?;
                let air = world.block(p)?;
                if !world.is_air(&air)?
                    || !world.patch_face_sturdy(&surface, ground_pos, [0, -self.direction, 0])?
                {
                    continue;
                }
                let depth = self.depth.sample(random)
                    + i32::from(
                        self.bottom_chance > 0.0 && random.next_float() < self.bottom_chance,
                    );
                let mut cursor = ground_pos;
                let mut success = true;
                for i in 0..depth {
                    let replacement = self
                        .ground
                        .state(world, random, cursor)?
                        .context("patch ground provider returned no state")?;
                    let old = world.block(cursor)?;
                    if world.same_block(&replacement, &old)? {
                        continue;
                    }
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
                    if !replace {
                        success = i != 0;
                        break;
                    }
                    world.set_patch_block(cursor, &replacement)?;
                    cursor = offset(cursor, 0, self.direction, 0);
                }
                if success {
                    ground.push(ground_pos);
                }
            }
        }
        let ground = hash_position_order(ground)?;
        let targets = if let Some(water) = &self.water {
            let mut enclosed = Vec::new();
            'each: for pos in ground {
                for direction in [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0], [0, -1, 0]] {
                    let neighbor = offset(pos, direction[0], direction[1], direction[2]);
                    let state = world.block(neighbor)?;
                    if !world.patch_face_sturdy(
                        &state,
                        neighbor,
                        [-direction[0], -direction[1], -direction[2]],
                    )? {
                        continue 'each;
                    }
                }
                enclosed.push(pos);
            }
            let selected = hash_position_order(enclosed)?;
            for &pos in &selected {
                world.set_patch_block(pos, water)?;
            }
            selected
        } else {
            ground
        };
        for &pos in &targets {
            if self.vegetation_chance > 0.0 && random.next_float() < self.vegetation_chance {
                let base = if self.water.is_some() {
                    offset(pos, 0, -1, 0)
                } else {
                    pos
                };
                let planted = place_vegetation(world, random, offset(base, 0, -self.direction, 0))?;
                if planted && self.water.is_some() {
                    let old = world.block(pos)?;
                    if let Some(state) = world.patch_waterlogged(old)? {
                        world.set_patch_block(pos, &state)?;
                    }
                }
            }
        }
        Ok(!targets.is_empty())
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
    use std::collections::BTreeMap;
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct State {
        name: String,
        waterlogged: Option<bool>,
    }
    fn state(name: &str) -> State {
        State {
            name: format!("minecraft:{name}"),
            waterlogged: (name == "oak_leaves").then_some(false),
        }
    }
    struct World {
        ceiling: bool,
        blocks: BTreeMap<[i32; 3], State>,
        events: Vec<String>,
    }
    impl OreWorld for World {
        type State = State;
        fn block(&mut self, p: [i32; 3]) -> Result<State> {
            if let Some(s) = self.blocks.get(&p) {
                return Ok(s.clone());
            }
            let solid = if self.ceiling { p[1] >= 7 } else { p[1] < 4 };
            if !solid {
                return Ok(state("air"));
            }
            if p[1] == if self.ceiling { 7 } else { 3 } && (p[0] + p[2]).rem_euclid(5) == 0 {
                return Ok(state("moss_block"));
            }
            if (p[0] * 3 + p[2]).rem_euclid(11) == 0 {
                return Ok(state("bedrock"));
            }
            Ok(state("stone"))
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
    impl VegetationPatchWorld for World {
        fn same_block(&mut self, a: &State, b: &State) -> Result<bool> {
            Ok(a.name == b.name)
        }
        fn patch_face_sturdy(&mut self, s: &State, _: [i32; 3], _: [i32; 3]) -> Result<bool> {
            Ok(matches!(
                s.name.as_str(),
                "minecraft:stone" | "minecraft:bedrock" | "minecraft:clay" | "minecraft:moss_block"
            ))
        }
        fn set_patch_block(&mut self, p: [i32; 3], s: &State) -> Result<()> {
            self.blocks.insert(p, s.clone());
            let extra = s.waterlogged.map(|v| format!(":{v}")).unwrap_or_default();
            self.events
                .push(format!("W{},{},{},{}{}", p[0], p[1], p[2], s.name, extra));
            Ok(())
        }
        fn patch_waterlogged(&mut self, mut s: State) -> Result<Option<State>> {
            if s.waterlogged == Some(false) {
                s.waterlogged = Some(true);
                Ok(Some(s))
            } else {
                Ok(None)
            }
        }
    }
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    fn definition(wet: bool, ceiling: bool) -> Value {
        json!({
            "type":if wet{"minecraft:waterlogged_vegetation_patch"}else{"minecraft:vegetation_patch"},
            "replaceable":["minecraft:stone","minecraft:clay"],"ground_state":{"type":"minecraft:weighted","entries":[{"data":{"id":"minecraft:moss_block"},"weight":2},{"data":{"id":"minecraft:clay"},"weight":1}]},
            "vegetation_feature":{"feature":"test:vegetation","placement":[]},"surface":if ceiling{"ceiling"}else{"floor"},
            "depth":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2},"extra_bottom_block_chance":0.4,"vertical_range":3,"vegetation_chance":0.6,
            "xz_radius":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":2},"extra_edge_column_chance":0.35
        })
    }
    fn resolve(v: &Value) -> Result<State> {
        let raw = v
            .as_str()
            .or_else(|| v.get("id").and_then(Value::as_str))
            .context("missing state")?;
        Ok(state(
            raw.strip_prefix("minecraft:").context("unknown state")?,
        ))
    }
    #[test]
    fn rejects_invalid_patch_configuration() {
        let mut value = definition(false, false);
        value["depth"] = json!(0);
        assert!(VegetationPatch::decode(&mut Data, &value, &mut resolve).is_err());
        let mut value = definition(false, false);
        value["vegetation_chance"] = json!(1.1);
        assert!(VegetationPatch::decode(&mut Data, &value, &mut resolve).is_err());
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn patch_placement_and_random_state_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
        let source = temp.path().join("VegetationPatchOracle.java");
        std::fs::write(
            &source,
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
            .arg(&source));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("VegetationPatchOracle"));
        let mut expected = Vec::new();
        for wet in [false, true] {
            for ceiling in [false, true] {
                for seed in [0, 1, -1, 42, 987654321] {
                    let patch =
                        VegetationPatch::decode(&mut Data, &definition(wet, ceiling), &mut resolve)
                            .unwrap();
                    let mut world = World {
                        ceiling,
                        blocks: BTreeMap::new(),
                        events: Vec::new(),
                    };
                    let mut random = DecorationRandom::new(seed);
                    let result = patch
                        .place(&mut world, &mut random, [4, 5, -3], |world, random, p| {
                            let n = random.next_int_bound(3).unwrap();
                            world
                                .events
                                .push(format!("P{},{},{},{}", p[0], p[1], p[2], n));
                            if n == 0 {
                                return Ok(false);
                            }
                            world.set_patch_block(p, &state("oak_leaves"))?;
                            Ok(true)
                        })
                        .unwrap();
                    expected.push(format!(
                        "V {wet} {ceiling} {seed} {result} {} R {}",
                        world.events.join(";"),
                        random.next_long()
                    ));
                }
            }
        }
        let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("V ")).collect();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(*actual, expected);
        }
    }
}
