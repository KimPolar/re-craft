//! Apply ordinary ore features to intermediate generated chunk sections.
//!
//! The surrounding decoration scheduler must still supply Java's feature order,
//! random seed, writable region and complete CARVERS-stage neighborhood.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use native_resources::{
    heightmap_tags::HeightmapTags,
    ore_feature::{OreBlockTags, OreVein, OreVeinWorld, OreWorld},
    packs::ResourceId,
};
use native_world::loaded::{BlockState, BlockStateId, ChunkPos, World};

use super::CarvedTerrainWorld;

/// An unpublished generation attempt. Dropping it, or any failed placement,
/// leaves the original terrain and its snapshots untouched.
pub struct OreVeinRegion<'a> {
    target: &'a mut CarvedTerrainWorld,
    world: World,
    tags: &'a OreBlockTags,
    motion: &'a HeightmapTags,
    writable: BTreeSet<ChunkPos>,
    heights: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    motion_cache: BTreeMap<BlockStateId, bool>,
    failed: bool,
}

impl CarvedTerrainWorld {
    /// Writable chunks must match the Java WorldGenRegion for this step.
    /// All reads, including neighbors and heightmaps, require loaded chunks.
    /// The tags must belong to the same registry and active data packs for the
    /// lifetime of the terrain. Other generation writes must update any primed
    /// heightmaps before this interface can be combined with those features.
    pub fn ore_vein_region<'a>(
        &'a mut self,
        tags: &'a OreBlockTags,
        motion: &'a HeightmapTags,
        writable: BTreeSet<ChunkPos>,
    ) -> Result<OreVeinRegion<'a>> {
        ensure!(
            writable.iter().all(|p| self.world.chunks.contains_key(p)),
            "writable generation chunk is missing"
        );
        let world = (*self.world).clone();
        let heights = self.ore_ocean_floor_heights.clone();
        Ok(OreVeinRegion {
            target: self,
            world,
            tags,
            motion,
            writable,
            heights,
            motion_cache: BTreeMap::new(),
            failed: false,
        })
    }
}

impl OreVeinRegion<'_> {
    /// A failed attempt must be discarded; on failure the caller's RNG is unchanged.
    /// If the region contains earlier successful placements, the caller must
    /// also restore its RNG to the beginning of the discarded region.
    pub fn place(
        &mut self,
        feature: &OreVein<BlockState>,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let result = feature.place(self, &mut next, origin)?;
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Publish this intermediate state; it is not a finished or saveable chunk.
    pub fn finish(self) -> Result<()> {
        ensure!(!self.failed, "cannot publish a failed generation attempt");
        self.target.world = Arc::new(self.world);
        self.target.ore_ocean_floor_heights = self.heights;
        Ok(())
    }

    fn read(&self, p: [i32; 3]) -> Result<BlockState> {
        self.world
            .block(p[0], p[1], p[2])
            .cloned()
            .context("generation block or section is missing")
    }

    fn is_outside(&self, y: i32) -> bool {
        y < self.target.min_y || y >= self.target.min_y + self.target.height
    }

    // Java primes the entire heightmap on first access. Direct ore section
    // writes do not update it, including for data-pack replacement states.
    fn prime_heightmap(&mut self, pos: ChunkPos) -> Result<()> {
        let base_x = pos.x.checked_mul(16).context("chunk X overflow")?;
        let base_z = pos.z.checked_mul(16).context("chunk Z overflow")?;
        ensure!(
            base_x.checked_add(15).is_some() && base_z.checked_add(15).is_some(),
            "chunk coordinate overflow"
        );
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generation heightmap chunk is missing"
        );
        let mut values = Box::new([self.target.min_y; 256]);
        for z in 0..16 {
            for x in 0..16 {
                for y in (self.target.min_y..self.target.min_y + self.target.height).rev() {
                    let id = self
                        .world
                        .block_id(base_x + x, y, base_z + z)
                        .context("generation heightmap block is missing")?;
                    let matches = if let Some(value) = self.motion_cache.get(&id) {
                        *value
                    } else {
                        let state = self
                            .world
                            .state(id)
                            .context("generation block state is missing")?;
                        let value =
                            !is_air(state) && self.motion.for_block(&state.name)?.blocks_motion;
                        self.motion_cache.insert(id, value);
                        value
                    };
                    if matches {
                        values[(x + z * 16) as usize] = y + 1;
                        break;
                    }
                }
            }
        }
        self.heights.insert(pos, Arc::from(values));
        Ok(())
    }
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
fn chunk(p: [i32; 3]) -> ChunkPos {
    ChunkPos {
        x: p[0].div_euclid(16),
        z: p[2].div_euclid(16),
    }
}

impl OreWorld for OreVeinRegion<'_> {
    type State = BlockState;
    fn block(&mut self, p: [i32; 3]) -> Result<BlockState> {
        self.read(p)
    }
    fn block_is(&mut self, state: &BlockState, id: &ResourceId) -> Result<bool> {
        Ok(state.name == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, state: &BlockState, tag: &ResourceId) -> Result<bool> {
        self.tags.contains(tag, &state.name)
    }
    fn is_air(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state))
    }
    fn set_block(&mut self, _: [i32; 3], _: &BlockState, _: i32) -> Result<bool> {
        bail!("WorldGenLevel.setBlock effects are not implemented by the ore vein region")
    }
}
impl OreVeinWorld for OreVeinRegion<'_> {
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        let pos = chunk([x, 0, z]);
        if !self.heights.contains_key(&pos) {
            self.prime_heightmap(pos)?;
        }
        Ok(self.heights[&pos][(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize])
    }
    fn outside_build_height(&self, y: i32) -> bool {
        self.is_outside(y)
    }
    fn ensure_can_write(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.writable.contains(&chunk(p)))
    }
    fn section_block(&mut self, p: [i32; 3]) -> Result<Option<BlockState>> {
        if self.is_outside(p[1]) {
            return Ok(None);
        }
        self.read(p).map(Some)
    }
    fn section_neighbor(&mut self, p: [i32; 3]) -> Result<BlockState> {
        if self.is_outside(p[1]) {
            return Ok(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
        }
        self.read(p)
    }
    fn set_section_block(&mut self, p: [i32; 3], state: &BlockState) -> Result<()> {
        ensure!(!self.is_outside(p[1]), "ore write outside build height");
        let id = self.world.intern_state(state.clone());
        let section_y = i8::try_from(p[1].div_euclid(16))?;
        let chunk = Arc::make_mut(
            self.world
                .chunks
                .get_mut(&chunk(p))
                .context("ore write chunk is missing")?,
        );
        let section = Arc::make_mut(
            chunk
                .sections
                .get_mut(&section_y)
                .context("ore write section is missing")?,
        );
        let blocks = section
            .blocks
            .as_mut()
            .context("ore write block data is missing")?;
        let index = p[0].rem_euclid(16) as usize
            + p[2].rem_euclid(16) as usize * 16
            + p[1].rem_euclid(16) as usize * 256;
        blocks[index] = id;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;
    use native_world::loaded::{BLOCK_COUNT, BlockStateId, Chunk, Section};
    use serde_json::{Value, json};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:diamond_ore"]}"#.to_vec())
                }
                "data/minecraft/tags/block/base_stone_overworld.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }
    fn state(name: &str) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        }
    }
    fn fixture() -> (
        CarvedTerrainWorld,
        OreBlockTags,
        HeightmapTags,
        OreVein<BlockState>,
    ) {
        let registry = ["minecraft:air", "minecraft:stone", "minecraft:diamond_ore"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut data = Data;
        let feature = OreVein::decode(&json!({"type":"minecraft:ore", "size": 33, "discard_chance_on_air_exposure": 0.0, "targets": [{"target":{"predicate_type":"minecraft:tag_match", "tag":"minecraft:base_stone_overworld"}, "state":"minecraft:diamond_ore"}]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        for z in -1..=1 {
            for x in -1..=1 {
                let position = ChunkPos { x, z };
                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            blocks[x + z * 16 + y * 256] = if y < 8 { stone } else { air };
                        }
                    }
                }
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        position,
                        status: Some("minecraft:carvers".into()),
                        block_light_correct: false,
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let result = CarvedTerrainWorld {
            world: Arc::new(world),
            seed: 0,
            min_y: 0,
            height: 16,
            ore_ocean_floor_heights: BTreeMap::new(),
            postprocessing: BTreeMap::new(),
            noise_postprocessing: BTreeMap::new(),
            surface_postprocessing: BTreeMap::new(),
            carver_postprocessing: BTreeMap::new(),
        };
        (result, tags, motion, feature)
    }

    #[test]
    fn writes_real_sections_and_preserves_snapshots_without_session_edits() {
        let (mut terrain, tags, motion, feature) = fixture();
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        assert!(region.place(&feature, &mut random, [15, 4, 7]).unwrap());
        region.finish().unwrap();
        let current = terrain.snapshot();
        let count = (0..32)
            .flat_map(|x| (0..16).flat_map(move |z| (0..8).map(move |y| (x, y, z))))
            .filter(|&(x, y, z)| current.block(x, y, z).unwrap().name == "minecraft:diamond_ore")
            .count();
        assert!(count > 0);
        assert_eq!(previous.block(15, 4, 7).unwrap().name, "minecraft:stone");
        assert_eq!(current.block_edits().count(), 0);
        let untouched = ChunkPos { x: -1, z: -1 };
        assert!(Arc::ptr_eq(
            &previous.chunks[&untouched],
            &current.chunks[&untouched]
        ));
    }

    #[test]
    fn primes_whole_heightmap_and_keeps_direct_section_writes_out_of_it() {
        let (mut terrain, tags, motion, _) = fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        assert_eq!(region.ocean_floor_height(0, 0).unwrap(), 8);
        region
            .set_section_block([1, 12, 1], &state("stone"))
            .unwrap();
        assert_eq!(region.ocean_floor_height(1, 1).unwrap(), 8);
        assert_eq!(region.section_neighbor([1, -1, 1]).unwrap(), state("air"));
        region.finish().unwrap();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut next = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        assert_eq!(next.ocean_floor_height(1, 1).unwrap(), 8);
    }

    #[test]
    fn missing_neighbors_reject_and_discard_attempt_without_consuming_rng() {
        let (mut terrain, tags, motion, feature) = fixture();
        Arc::make_mut(&mut terrain.world)
            .chunks
            .remove(&ChunkPos { x: 0, z: -1 });
        let previous = terrain.snapshot();
        let writable = previous.chunks.keys().copied().collect();
        let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(123);
        let mut expected = random.clone();
        assert!(region.place(&feature, &mut random, [15, 4, 7]).is_err());
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&previous, &terrain.snapshot()));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn generated_chunk_veins_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
        let source = temp.path().join("OreVeinOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/OreVeinOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("OreVeinOracle"));
        let mut expected =
            BTreeMap::<(i32, String, i64), (BTreeMap<[i32; 3], String>, bool, i64)>::new();
        let mut key = None;
        let mut writes = BTreeMap::new();
        for line in output.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["C", size, chance, seed] => {
                    key = Some((
                        size.parse().unwrap(),
                        (*chance).to_owned(),
                        seed.parse().unwrap(),
                    ));
                    writes.clear();
                }
                ["W", x, y, z, name] if key.is_some() => {
                    writes.insert(
                        [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                        (*name).to_owned(),
                    );
                }
                ["R", result, next] if key.is_some() => {
                    expected.insert(
                        key.take().unwrap(),
                        (
                            std::mem::take(&mut writes),
                            result.parse().unwrap(),
                            next.parse().unwrap(),
                        ),
                    );
                }
                _ => {}
            }
        }
        struct OracleData;
        impl ResourceProvider for OracleData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/tags/block/base_stone_overworld.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(br#"{"values":["minecraft:stone","minecraft:deepslate"]}"#.to_vec())
                    }
                    _ => None,
                })
            }
        }
        let registry = ["air", "stone", "deepslate", "diamond_ore", "gold_ore"]
            .into_iter()
            .map(|x| format!("minecraft:{x}"))
            .collect();
        let mut data = OracleData;
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        let mut checked = 0;
        for size in [3, 17, 64] {
            for chance in [0.0f32, 0.5, 1.0] {
                let feature = OreVein::decode(&json!({"type":"minecraft:ore","size":size,"discard_chance_on_air_exposure":chance,"targets":[
                    {"state":"minecraft:diamond_ore","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
                    {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
                ]}), &mut |v: &Value| Ok(state(v.as_str().unwrap().strip_prefix("minecraft:").unwrap()))).unwrap();
                let tags =
                    OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
                for seed in [1i64, 2, 5, 7] {
                    let (mut terrain, _, _, _) = fixture();
                    let world = Arc::make_mut(&mut terrain.world);
                    world.chunks.clear();
                    let ids = [
                        world.intern_state(state("air")),
                        world.intern_state(state("stone")),
                        world.intern_state(state("deepslate")),
                    ];
                    for cz in 1..=3i32 {
                        for cx in -3..=0i32 {
                            let pos = ChunkPos { x: cx, z: cz };
                            let mut sections = BTreeMap::new();
                            for sy in -1..=0i32 {
                                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                                for y in 0..16i32 {
                                    for z in 0..16i32 {
                                        for x in 0..16i32 {
                                            let (wx, wy, wz) =
                                                (cx * 16 + x, sy * 16 + y, cz * 16 + z);
                                            let v = wx * 31 + wy * 17 + wz * 13;
                                            blocks[(x + z * 16 + y * 256) as usize] =
                                                ids[if v & 7 == 0 {
                                                    0
                                                } else if wy < 0 {
                                                    2
                                                } else {
                                                    1
                                                }];
                                        }
                                    }
                                }
                                sections.insert(
                                    sy as i8,
                                    Arc::new(Section {
                                        blocks: Some(blocks),
                                        biomes: None,
                                        block_light: None,
                                        sky_light: None,
                                    }),
                                );
                            }
                            world.chunks.insert(
                                pos,
                                Arc::new(Chunk {
                                    position: pos,
                                    status: Some("minecraft:carvers".into()),
                                    block_light_correct: false,
                                    sections,
                                }),
                            );
                        }
                    }
                    terrain.min_y = -16;
                    terrain.height = 32;
                    let previous = terrain.snapshot();
                    let writable = previous.chunks.keys().copied().collect();
                    let mut region = terrain.ore_vein_region(&tags, &motion, writable).unwrap();
                    let mut random = DecorationRandom::new(0);
                    let deco = random.set_decoration_seed(seed, -32, 48);
                    random.set_feature_seed(deco, 7, 3);
                    let result = region
                        .place(
                            &feature,
                            &mut random,
                            [-16, if seed % 2 == 0 { -1 } else { 3 }, 32],
                        )
                        .unwrap();
                    region.finish().unwrap();
                    let world = terrain.snapshot();
                    let mut actual = BTreeMap::new();
                    for z in 16..64 {
                        for x in -48..16 {
                            for y in -16..16 {
                                let after = world.block(x, y, z).unwrap();
                                if previous.block(x, y, z).unwrap() != after {
                                    actual.insert([x, y, z], after.name.clone());
                                }
                            }
                        }
                    }
                    let key = (size, format!("{chance:.1}"), seed);
                    let (java_writes, java_result, java_next) = &expected[&key];
                    assert_eq!(&actual, java_writes, "block writes {key:?}");
                    assert_eq!(result, *java_result, "feature result {key:?}");
                    assert_eq!(random.next_long(), *java_next, "random stream {key:?}");
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 36);
    }
}
