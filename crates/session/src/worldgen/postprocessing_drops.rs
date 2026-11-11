//! Transactional non-entity block drops during generated chunk postprocessing.
//!
//! These records are intermediate: callers must merge them into complete
//! entity storage and continue both random streams before publishing a world.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_persistence::{nbt::Tag, random_sequences::RandomSequences};
use native_resources::loot::BlockLootTables;
use native_world::loaded::{BlockState, ChunkPos};

/// A forkable checkpoint of one level's loot, item and level random streams.
/// The caller supplies the same initial random streams and rules used by the
/// level; no random state is inferred from the terrain seed.
#[derive(Clone)]
pub struct GeneratedBlockDrops {
    loot: Arc<BlockLootTables>,
    world_seed: i64,
    sequences: Arc<RandomSequences>,
    level_random: LegacyRandom,
    entity_seeder: XoroshiroRandom,
    enabled: bool,
    records: Arc<BTreeMap<ChunkPos, Arc<Vec<Tag>>>>,
    uuids: Arc<BTreeSet<[i32; 4]>>,
}

impl GeneratedBlockDrops {
    pub fn new(
        loot: Arc<BlockLootTables>,
        world_seed: i64,
        sequences: RandomSequences,
        level_random: LegacyRandom,
        entity_seeder: XoroshiroRandom,
        enabled: bool,
    ) -> Self {
        Self {
            loot,
            world_seed,
            sequences: Arc::new(sequences),
            level_random,
            entity_seeder,
            enabled,
            records: Arc::new(BTreeMap::new()),
            uuids: Arc::new(BTreeSet::new()),
        }
    }

    /// Evaluate the original block state, then prepare the resulting item NBT.
    /// Errors do not change either random stream or any previously saved item.
    pub fn drop_block(&mut self, pos: IVec3, block: &BlockState) -> Result<()> {
        let mut level = self.level_random.clone();
        let mut seeder = self.entity_seeder.clone();
        let sequence = self.loot.random_sequence_for_block(&block.name)?.cloned();
        let (drops, changed_sequence) = if let Some(id) = sequence {
            let name = format!("{}:{}", id.namespace, id.path);
            let mut random = self.sequences.get(&name, self.world_seed)?;
            let drops = self.loot.drops_without_entity_at(
                &block.name,
                Some(&block.properties),
                Some(&mut random),
            )?;
            (drops, Some((name, random)))
        } else {
            (
                self.loot.drops_without_entity_at(
                    &block.name,
                    Some(&block.properties),
                    Some(&mut level),
                )?,
                None,
            )
        };
        let records = crate::items::prepare_generated_block_drops(
            pos,
            drops,
            self.enabled,
            &mut level,
            &mut seeder,
        )?;
        let mut ids = BTreeSet::new();
        for record in &records {
            let uuid = crate::saved_item::optional_uuid(record, "UUID")?
                .context("generated item UUID is missing")?;
            ensure!(
                ids.insert(uuid) && !self.uuids.contains(&uuid),
                "duplicate generated item UUID"
            );
        }
        if let Some((name, random)) = changed_sequence {
            Arc::make_mut(&mut self.sequences).set(&name, &random)?;
        }
        if !records.is_empty() {
            let chunk = ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            };
            Arc::make_mut(Arc::make_mut(&mut self.records).entry(chunk).or_default())
                .extend(records);
            Arc::make_mut(&mut self.uuids).extend(ids);
        }
        self.level_random = level;
        self.entity_seeder = seeder;
        Ok(())
    }

    pub fn records(&self, chunk: ChunkPos) -> &[Tag] {
        self.records
            .get(&chunk)
            .map_or(&[], |records| records.as_slice())
    }

    pub fn item_chunks(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.records.keys().copied()
    }

    pub fn level_random(&self) -> &LegacyRandom {
        &self.level_random
    }
    pub fn random_sequences(&self) -> &RandomSequences {
        &self.sequences
    }
    pub fn entity_seeder(&self) -> &XoroshiroRandom {
        &self.entity_seeder
    }
    pub fn world_seed(&self) -> i64 {
        self.world_seed
    }
    pub fn block_drops_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::packs::ResourceProvider;

    struct Loot;
    impl ResourceProvider for Loot {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/loot_table/blocks/ladder.json" => Some(br#"{"type":"minecraft:block","random_sequence":"example:ladder","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:ladder","condition":{"type":"minecraft:random_chance","chance":1.0}}]}]}"#.to_vec()),
                "data/minecraft/loot_table/blocks/snow.json" => Some(br#"{"type":"minecraft:block","pools":[{"rolls":1,"condition":{"type":"example:unsupported"},"entries":[{"type":"minecraft:item","name":"minecraft:stick"}]}]}"#.to_vec()),
                "data/minecraft/loot_table/blocks/stone.json" => Some(br#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","condition":{"type":"minecraft:random_chance","chance":1.0}}]}]}"#.to_vec()),
                _ => None,
            })
        }
    }

    fn fixture(enabled: bool) -> GeneratedBlockDrops {
        GeneratedBlockDrops::new(
            Arc::new(BlockLootTables::load(&mut Loot).unwrap()),
            42,
            RandomSequences::read(None).unwrap(),
            LegacyRandom::new(9182),
            XoroshiroRandom::new(77),
            enabled,
        )
    }

    fn ladder() -> BlockState {
        BlockState {
            name: "minecraft:ladder".into(),
            properties: BTreeMap::from([
                ("facing".into(), "east".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        }
    }

    #[test]
    fn drops_capture_item_records_and_advance_the_correct_streams() {
        let pos = IVec3::new(-1, 70, -17);
        let chunk = ChunkPos { x: -1, z: -2 };
        let mut enabled = fixture(true);
        let mut disabled = fixture(false);
        let original_sequence = enabled.sequences.get("example:ladder", 42).unwrap().state();
        let original_entity = enabled.entity_seeder.state();
        let mut expected_level = enabled.level_random.clone();
        let expected_position = [
            f64::from(pos.x) + 0.25 + expected_level.next_double() * 0.5,
            f64::from(pos.y) + 0.125 + expected_level.next_double() * 0.5,
            f64::from(pos.z) + 0.25 + expected_level.next_double() * 0.5,
        ];
        enabled.drop_block(pos, &ladder()).unwrap();
        disabled.drop_block(pos, &ladder()).unwrap();
        assert_eq!(enabled.records(chunk).len(), 1);
        assert!(disabled.records(chunk).is_empty());
        let saved = crate::saved_item::read(&enabled.records(chunk)[0])
            .unwrap()
            .unwrap();
        assert_eq!(saved.position.to_array(), expected_position);
        assert_eq!(saved.stack.id(), "minecraft:ladder");
        assert_eq!(
            enabled.level_random.clone().next_int(),
            expected_level.clone().next_int()
        );
        assert_eq!(
            disabled.level_random.clone().next_int(),
            expected_level.next_int()
        );
        let after = enabled.sequences.get("example:ladder", 42).unwrap().state();
        assert_ne!(after, original_sequence);
        assert_eq!(
            after,
            disabled
                .sequences
                .get("example:ladder", 42)
                .unwrap()
                .state()
        );
        assert_ne!(enabled.entity_seeder.state(), original_entity);
        assert_eq!(disabled.entity_seeder.state(), original_entity);
    }

    #[test]
    fn failed_drops_and_forks_leave_previous_records_and_randomness_unchanged() {
        let mut original = fixture(true);
        let mut fork = original.clone();
        let chunk = ChunkPos { x: 0, z: 0 };
        fork.drop_block(IVec3::new(1, 3, 1), &ladder()).unwrap();
        assert_eq!(fork.records(chunk).len(), 1);
        assert!(original.records(chunk).is_empty());
        let original_level = original.level_random.clone().next_int();
        let original_sequence = original
            .sequences
            .get("example:ladder", 42)
            .unwrap()
            .state();
        let original_entity = original.entity_seeder.state();
        let bad = BlockState {
            name: "minecraft:snow".into(),
            properties: BTreeMap::from([("layers".into(), "1".into())]),
        };
        assert!(original.drop_block(IVec3::new(1, 3, 1), &bad).is_err());
        assert!(
            original
                .drop_block(IVec3::new(40_000_000, 3, 1), &ladder())
                .is_err()
        );
        assert!(original.records(chunk).is_empty());
        assert_eq!(original.level_random.clone().next_int(), original_level);
        assert_eq!(
            original
                .sequences
                .get("example:ladder", 42)
                .unwrap()
                .state(),
            original_sequence
        );
        assert_eq!(original.entity_seeder.state(), original_entity);
        original.drop_block(IVec3::new(1, 3, 1), &ladder()).unwrap();
        assert_eq!(original.records(chunk), fork.records(chunk));
    }

    #[test]
    fn unnamed_loot_uses_the_level_stream_before_item_position_samples() {
        let mut generated = fixture(true);
        let mut expected = generated.level_random.clone();
        expected.next_float();
        for _ in 0..3 {
            expected.next_double();
        }
        generated
            .drop_block(
                IVec3::new(1, 3, 1),
                &BlockState {
                    name: "minecraft:stone".into(),
                    properties: BTreeMap::new(),
                },
            )
            .unwrap();
        assert_eq!(
            generated.level_random.clone().next_int(),
            expected.next_int()
        );
        assert!(!generated.sequences.is_dirty());
        assert_eq!(generated.records(ChunkPos { x: 0, z: 0 }).len(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn generated_ladder_loot_and_random_streams_match_java() {
        use native_resources::packs::ResourceStack;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("LadderDropLootOracle.java");
        let input = temp.path().join("ladder.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../../tests/java/LadderDropLootOracle.java"),
        )
        .unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        std::fs::write(
            &input,
            resources
                .read_builtin_data("data/minecraft/loot_table/blocks/ladder.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let result = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("LadderDropLootOracle")
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 40);
        let tables = Arc::new(BlockLootTables::load(&mut resources).unwrap());
        for group in rows.chunks_exact(8) {
            let seed = group[0]["seed"].as_i64().unwrap();
            for enabled in [false, true] {
                let mut generated = GeneratedBlockDrops::new(
                    Arc::clone(&tables),
                    seed,
                    RandomSequences::read(None).unwrap(),
                    LegacyRandom::new(9182),
                    XoroshiroRandom::new(77),
                    enabled,
                );
                let chunk = ChunkPos { x: 0, z: 0 };
                for (step, row) in group.iter().enumerate() {
                    let mut block = ladder();
                    block
                        .properties
                        .insert("waterlogged".into(), (step % 2 == 0).to_string());
                    let previous = generated.records(chunk).len();
                    generated.drop_block(IVec3::ZERO, &block).unwrap();
                    let actual: Vec<_> = generated.records(chunk)[previous..]
                        .iter()
                        .map(|entity| {
                            let item = crate::saved_item::read(entity).unwrap().unwrap();
                            serde_json::json!([item.stack.id(), item.stack.count()])
                        })
                        .collect();
                    let expected = if enabled {
                        row["drops"].as_array().unwrap().clone()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(actual, expected, "seed {seed} step {step} drops {enabled}");
                    assert_eq!(
                        generated.level_random.clone().next_double().to_bits(),
                        row["levelNext"].as_f64().unwrap().to_bits()
                    );
                    let mut random = generated
                        .sequences
                        .get("minecraft:blocks/ladder", seed)
                        .unwrap();
                    assert_eq!(random.next_long(), row["next"].as_i64().unwrap());
                }
            }
        }
    }
}
