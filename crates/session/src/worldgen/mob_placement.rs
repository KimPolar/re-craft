//! Block-backed placement queries for the supported generated block states.
use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_resources::mob_spawn_placement::{MobPlacementWorld, MobSpawnHeightmap};
use native_resources::worldgen_mob_spawner::{
    AttemptRejection, ChunkCreatureSpawner, CreatureAttempt, CreatureSpawnProgress,
};
use native_world::{
    loaded::{BlockState, ChunkPos, World},
    mob_spawn_blocks::{MobSpawnBlockFacts, common_mob_spawn_block_facts},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::FreshTerrainBatch;
use crate::respawn::RespawnBorder;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedCreatureProbeProgress {
    Pending,
    Rejected,
    /// Placement passed; entity-size collision and mob rules remain to check.
    NeedsCollision(IVec3),
}

pub struct GeneratedCreatureProbe {
    attempt: CreatureAttempt,
    top_search: native_resources::mob_spawn_placement::TopMobSpawnPosition,
    top: Option<IVec3>,
    can_summon: bool,
    rejected: bool,
}

impl GeneratedCreatureProbe {
    pub fn new(attempt: CreatureAttempt, min_y: i32, has_ceiling: bool, can_summon: bool) -> Self {
        Self {
            top_search: native_resources::mob_spawn_placement::TopMobSpawnPosition::new(
                attempt.x,
                attempt.z,
                min_y,
                has_ceiling,
            ),
            attempt,
            top: None,
            can_summon,
            rejected: false,
        }
    }

    /// Resolve one pending planner attempt against generated blocks. Missing
    /// heightmap inputs or unsupported block states return an error without
    /// advancing the planner. The caller still performs entity collision.
    pub fn poll(
        &mut self,
        planner: &mut ChunkCreatureSpawner,
        world: &mut GeneratedMobPlacementView<'_>,
        budget: usize,
    ) -> Result<GeneratedCreatureProbeProgress> {
        ensure!(!self.rejected, "creature probe was already rejected");
        ensure!(
            planner.next(0)? == CreatureSpawnProgress::Attempt(self.attempt.clone()),
            "creature planner attempt changed"
        );
        if self.top.is_none() {
            self.top = self.top_search.poll(world, budget)?;
            if self.top.is_none() {
                return Ok(GeneratedCreatureProbeProgress::Pending);
            }
        } else if budget == 0 {
            return Ok(GeneratedCreatureProbeProgress::Pending);
        }
        let top = self.top.expect("top position was resolved");
        if !self.can_summon
            || !native_resources::mob_spawn_placement::on_ground_spawn_position_ok(world, top)?
        {
            planner.reject(AttemptRejection::Placement)?;
            self.rejected = true;
            return Ok(GeneratedCreatureProbeProgress::Rejected);
        }
        Ok(GeneratedCreatureProbeProgress::NeedsCollision(top))
    }
}

pub struct GeneratedMobPlacementView<'a> {
    batch: &'a FreshTerrainBatch,
    world: Arc<World>,
    heightmap: MobSpawnHeightmap,
    border: RespawnBorder,
    prevent_inside: &'a BTreeSet<String>,
    height_tags: &'a native_resources::heightmap_tags::HeightmapTags,
    min_y: i32,
    max_y: i32,
    void_air: BlockState,
    last_facts: Option<(IVec3, MobSpawnBlockFacts)>,
    computed_heights: BTreeMap<(i32, i32), i32>,
}

impl<'a> GeneratedMobPlacementView<'a> {
    /// Tags must come from the same unchanged pack stack as the batch.
    pub fn new(
        batch: &'a FreshTerrainBatch,
        heightmap: MobSpawnHeightmap,
        border: RespawnBorder,
        prevent_inside: &'a BTreeSet<String>,
        height_tags: &'a native_resources::heightmap_tags::HeightmapTags,
    ) -> Result<Self> {
        ensure!(
            batch.is_complete(),
            "finish generation before mob placement"
        );
        ensure!(border.is_valid(), "invalid mob spawn border");
        let world = batch.snapshot().context("generated terrain is missing")?;
        let (min_y, max_y) = batch.build_bounds()?;
        Ok(Self {
            batch,
            world,
            heightmap,
            border,
            prevent_inside,
            height_tags,
            min_y,
            max_y,
            void_air: BlockState {
                name: "minecraft:void_air".into(),
                properties: Default::default(),
            },
            last_facts: None,
            computed_heights: BTreeMap::new(),
        })
    }
    fn block(&self, pos: IVec3) -> Result<&BlockState> {
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        ensure!(
            self.world.chunks.contains_key(&chunk),
            "mob placement chunk is missing"
        );
        if pos.y < self.min_y || pos.y >= self.max_y {
            return Ok(&self.void_air);
        }
        self.world
            .block(pos.x, pos.y, pos.z)
            .context("mob placement block is missing")
    }
    fn facts(&mut self, pos: IVec3) -> Result<MobSpawnBlockFacts> {
        if let Some((cached_pos, facts)) = self.last_facts {
            if cached_pos == pos {
                return Ok(facts);
            }
        }
        let state = self.block(pos)?;
        let facts = common_mob_spawn_block_facts(state)
            .with_context(|| format!("unsupported mob spawn block state {state:?}"))?;
        self.last_facts = Some((pos, facts));
        Ok(facts)
    }
}

impl MobPlacementWorld for GeneratedMobPlacementView<'_> {
    fn height(&mut self, x: i32, z: i32) -> Result<i32> {
        if let Some(height) = self.batch.primed_mob_spawn_height(x, z, self.heightmap)? {
            return Ok(height);
        }
        if let Some(&height) = self.computed_heights.get(&(x, z)) {
            return Ok(height);
        }
        // LIGHT-complete chunks are frozen against generation writes. A
        // requested column can therefore be scanned lazily without changing
        // the externally visible heightmap or recomputing an existing one.
        let mut height = self.min_y;
        for y in (self.min_y..self.max_y).rev() {
            let state = self.block(IVec3::new(x, y, z))?;
            if unprimed_heightmap_matches(state, self.heightmap, self.height_tags)? {
                height = y + 1;
                break;
            }
        }
        self.computed_heights.insert((x, z), height);
        Ok(height)
    }
    fn is_air(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.air)
    }
    fn pathfindable_land(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.pathfindable_land)
    }
    fn within_border(&mut self, pos: IVec3) -> Result<bool> {
        Ok(f64::from(pos.x) >= self.border.min_x
            && f64::from(pos.x) < self.border.max_x
            && f64::from(pos.z) >= self.border.min_z
            && f64::from(pos.z) < self.border.max_z)
    }
    fn valid_spawn_support(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.valid_support)
    }
    fn full_collision(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.full_collision)
    }
    fn signal_source(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.signal_source)
    }
    fn fluid_empty(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.fluid_empty)
    }
    fn prevents_mob_spawning(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.prevent_inside.contains(&self.block(pos)?.name))
    }
    fn dangerous_to_entity(&mut self, pos: IVec3) -> Result<bool> {
        Ok(self.facts(pos)?.dangerous)
    }
}

fn unprimed_heightmap_matches(
    state: &BlockState,
    kind: MobSpawnHeightmap,
    tags: &native_resources::heightmap_tags::HeightmapTags,
) -> Result<bool> {
    let air = matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    );
    if air {
        ensure!(state.properties.is_empty(), "malformed air block state");
        return Ok(false);
    }
    if kind == MobSpawnHeightmap::WorldSurface {
        return Ok(true);
    }
    let motion = tags.for_block(&state.name)?;
    let blocks_motion = match kind {
        MobSpawnHeightmap::OceanFloor | MobSpawnHeightmap::MotionBlocking => motion.blocks_motion,
        MobSpawnHeightmap::MotionBlockingNoLeaves => motion.blocks_motion_no_leaves,
        MobSpawnHeightmap::WorldSurface => unreachable!(),
    };
    if blocks_motion || kind == MobSpawnHeightmap::OceanFloor {
        return Ok(blocks_motion);
    }
    let facts = common_mob_spawn_block_facts(state)
        .with_context(|| format!("unsupported fluid state in mob heightmap: {state:?}"))?;
    Ok(!facts.fluid_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json" => {
                    Some(br#"{"values":["minecraft:stone","minecraft:oak_leaves"]}"#.to_vec())
                }
                "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                    Some(br#"{"values":["minecraft:stone"]}"#.to_vec())
                }
                _ => None,
            })
        }
    }
    fn state(name: &str, property: Option<(&str, &str)>) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: property
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
    #[test]
    fn missing_heightmap_predicates_follow_active_tags_and_fluid_state() {
        let registry = ["air", "stone", "water", "short_grass", "oak_leaves"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let tags = HeightmapTags::load(&mut Data, &registry).unwrap();
        let kinds = [
            MobSpawnHeightmap::WorldSurface,
            MobSpawnHeightmap::OceanFloor,
            MobSpawnHeightmap::MotionBlocking,
            MobSpawnHeightmap::MotionBlockingNoLeaves,
        ];
        for (state, expected) in [
            (state("air", None), [false, false, false, false]),
            (state("stone", None), [true, true, true, true]),
            (
                state("water", Some(("level", "0"))),
                [true, false, true, true],
            ),
            (state("short_grass", None), [true, false, false, false]),
        ] {
            for (kind, expected) in kinds.into_iter().zip(expected) {
                assert_eq!(
                    unprimed_heightmap_matches(&state, kind, &tags).unwrap(),
                    expected,
                    "{state:?} {kind:?}"
                );
            }
        }
        let leaves = state("oak_leaves", None);
        assert!(
            unprimed_heightmap_matches(&leaves, MobSpawnHeightmap::MotionBlocking, &tags).unwrap()
        );
        assert!(
            unprimed_heightmap_matches(&leaves, MobSpawnHeightmap::MotionBlockingNoLeaves, &tags)
                .is_err()
        );
        assert!(
            unprimed_heightmap_matches(
                &state("unknown", None),
                MobSpawnHeightmap::OceanFloor,
                &tags
            )
            .is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn unprimed_predicates_match_java_heightmap_types() {
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
        let registry = [
            "air",
            "stone",
            "water",
            "short_grass",
            "oak_leaves",
            "grass_block",
            "redstone_ore",
            "seagrass",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let tags = HeightmapTags::load(&mut Data, &registry).unwrap();
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("MobUnprimedHeightOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/MobUnprimedHeightOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let output = temp.path().join("output.json");
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("MobUnprimedHeightOracle")
            .arg(&output));
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let kinds = [
            MobSpawnHeightmap::WorldSurface,
            MobSpawnHeightmap::OceanFloor,
            MobSpawnHeightmap::MotionBlocking,
            MobSpawnHeightmap::MotionBlockingNoLeaves,
        ];
        let mut checked = 0;
        for row in rows {
            let state = BlockState {
                name: row["name"].as_str().unwrap().into(),
                properties: serde_json::from_value(row["properties"].clone()).unwrap(),
            };
            for (index, kind) in kinds.into_iter().enumerate() {
                match unprimed_heightmap_matches(&state, kind, &tags) {
                    Ok(actual) => {
                        assert_eq!(
                            actual,
                            row["matches"][index].as_bool().unwrap(),
                            "{state:?} {kind:?}"
                        );
                        checked += 1;
                    }
                    Err(_) => assert_eq!(state.name, "minecraft:oak_leaves"),
                }
            }
        }
        assert!(checked > 150, "only {checked} predicates checked");
    }
}
