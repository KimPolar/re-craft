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
use native_world::loaded::{BlockState, ChunkPos, World};

/// A successful Level.destroyBlock game event, in callback completion order.
#[derive(Clone, Debug)]
pub struct GeneratedBlockDestroyEvent {
    pub position: [i32; 3],
    pub state: BlockState,
    world: Arc<World>,
    registered_chunks: Arc<[ChunkPos]>,
    block_entities: Arc<BTreeMap<ChunkPos, Vec<Tag>>>,
    entities: Arc<BTreeMap<ChunkPos, Option<Arc<Vec<Tag>>>>>,
    generated_items: Arc<BTreeMap<ChunkPos, Arc<Vec<Tag>>>>,
}

impl GeneratedBlockDestroyEvent {
    /// Block geometry at emission time; listener registration is separate.
    pub fn world(&self) -> &Arc<World> {
        &self.world
    }

    /// Converted tick containers registered at emission, in their original order.
    /// This does not establish which listeners were active.
    pub fn registered_chunks(&self) -> &[ChunkPos] {
        &self.registered_chunks
    }

    /// Materialized block entities in registered chunks and sections within
    /// the event radius. This is not an active-listener registry.
    pub fn block_entities(&self) -> &BTreeMap<ChunkPos, Vec<Tag>> {
        &self.block_entities
    }

    /// Recorded protochunk entities at conversion. `None` means the source
    /// was unavailable; `Some` does not by itself prove complete coverage.
    pub fn entities(&self) -> &BTreeMap<ChunkPos, Option<Arc<Vec<Tag>>>> {
        &self.entities
    }

    /// Items created by postprocessing before this event was emitted.
    pub fn generated_items(&self) -> &BTreeMap<ChunkPos, Arc<Vec<Tag>>> {
        &self.generated_items
    }
}

impl PartialEq for GeneratedBlockDestroyEvent {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position
            && self.state == other.state
            && self.registered_chunks == other.registered_chunks
            && self.block_entities == other.block_entities
            && self.entities == other.entities
            && self.generated_items == other.generated_items
            && Arc::ptr_eq(&self.world, &other.world)
    }
}
impl Eq for GeneratedBlockDestroyEvent {}

/// Proof tied to one immutable event sequence. Construction is restricted to
/// a validated generation batch with complete entity coverage.
pub struct InertGeneratedDestroyEvents {
    events: Arc<Vec<GeneratedBlockDestroyEvent>>,
}

/// Only accept materialized block entities whose matching vanilla block and
/// block entity types have been checked for listener registration.
fn block_entities_without_listeners(world: &World, chunk: ChunkPos, records: &[Tag]) -> bool {
    records.iter().all(|record| {
        let Tag::Compound(fields) = record else {
            return false;
        };
        let unique = |name: &str| {
            let mut values = fields.iter().filter(|(key, _)| key.matches(name));
            let value = values.next().map(|(_, value)| value);
            if values.next().is_none() { value } else { None }
        };
        let (Some(Tag::String(id)), Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))) =
            (unique("id"), unique("x"), unique("y"), unique("z"))
        else {
            return false;
        };
        if x.div_euclid(16) != chunk.x || z.div_euclid(16) != chunk.z {
            return false;
        }
        let Some(state) = world.block(*x, *y, *z) else {
            return false;
        };
        (id.matches("minecraft:chest") && state.name == "minecraft:chest")
            || (id.matches("minecraft:mob_spawner") && state.name == "minecraft:spawner")
    })
}

/// These vanilla entity classes inherit Entity's no-op dynamic listener
/// registration in 26.3. Unknown IDs and passenger trees remain unverified.
pub(super) fn entities_without_listeners(records: &[Tag]) -> bool {
    records.iter().all(|record| {
        let Tag::Compound(fields) = record else {
            return false;
        };
        let mut ids = fields.iter().filter(|(name, _)| name.matches("id"));
        let Some((_, Tag::String(id))) = ids.next() else {
            return false;
        };
        if ids.next().is_some()
            || ![
                "minecraft:chicken",
                "minecraft:cow",
                "minecraft:pig",
                "minecraft:sheep",
                "minecraft:item",
            ]
            .iter()
            .any(|name| id.matches(name))
        {
            return false;
        }
        let mut passengers = fields.iter().filter(|(name, _)| name.matches("Passengers"));
        match passengers.next() {
            None => true,
            Some((
                _,
                Tag::List {
                    element_type,
                    values,
                },
            )) => {
                passengers.next().is_none()
                    && values.is_empty()
                    && matches!(
                        element_type,
                        native_persistence::nbt::TagType::End
                            | native_persistence::nbt::TagType::Compound
                    )
            }
            _ => false,
        }
    })
}

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
    items: Arc<BTreeMap<ChunkPos, Arc<Vec<crate::items::GeneratedItem>>>>,
    item_order: Arc<Vec<(ChunkPos, usize)>>,
    uuids: Arc<BTreeSet<[i32; 4]>>,
    destroy_events: Arc<Vec<GeneratedBlockDestroyEvent>>,
    operations: u64,
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
            items: Arc::new(BTreeMap::new()),
            item_order: Arc::new(Vec::new()),
            uuids: Arc::new(BTreeSet::new()),
            destroy_events: Arc::new(Vec::new()),
            operations: 0,
        }
    }

    /// Evaluate the original block state, then prepare the resulting item NBT.
    /// Errors do not change either random stream or any previously saved item.
    pub fn drop_block(&mut self, pos: IVec3, block: &BlockState) -> Result<()> {
        let operations = self
            .operations
            .checked_add(1)
            .context("generated drop count overflow")?;
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
        let (records, items) = crate::items::prepare_generated_block_drops(
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
        ensure!(
            records.len() == items.len()
                && records.iter().zip(&items).all(|(record, item)| {
                    crate::saved_item::optional_uuid(record, "UUID")
                        .ok()
                        .flatten()
                        == Some(item.uuid())
                }),
            "generated item records and runtime identities differ"
        );
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        let start = self.items.get(&chunk).map_or(0, |items| items.len());
        let end = start
            .checked_add(items.len())
            .context("generated item count overflow")?;
        if let Some((name, random)) = changed_sequence {
            Arc::make_mut(&mut self.sequences).set(&name, &random)?;
        }
        if !records.is_empty() {
            Arc::make_mut(&mut self.item_order).extend((start..end).map(|index| (chunk, index)));
            Arc::make_mut(Arc::make_mut(&mut self.records).entry(chunk).or_default())
                .extend(records);
            Arc::make_mut(Arc::make_mut(&mut self.items).entry(chunk).or_default()).extend(items);
            Arc::make_mut(&mut self.uuids).extend(ids);
        }
        self.level_random = level;
        self.entity_seeder = seeder;
        self.operations = operations;
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

    /// Runtime item state corresponding one-for-one to `records` in this chunk.
    pub fn items(&self, chunk: ChunkPos) -> &[crate::items::GeneratedItem] {
        self.items.get(&chunk).map_or(&[], |items| items.as_slice())
    }

    /// Items across all chunks in the order they were created.
    pub fn items_in_spawn_order(
        &self,
    ) -> impl Iterator<Item = (ChunkPos, &crate::items::GeneratedItem)> + '_ {
        self.item_order
            .iter()
            .map(|&(chunk, index)| (chunk, &self.items[&chunk][index]))
    }

    pub fn level_random(&self) -> &LegacyRandom {
        &self.level_random
    }
    pub(super) fn set_level_random(&mut self, random: LegacyRandom) {
        self.level_random = random;
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
    pub fn operations(&self) -> u64 {
        self.operations
    }

    /// Events have not yet been delivered to game-event listeners.
    pub fn destroy_events(&self) -> &[GeneratedBlockDestroyEvent] {
        &self.destroy_events
    }

    pub(crate) fn verify_inert_proof(&self, proof: &InertGeneratedDestroyEvents) -> bool {
        Arc::ptr_eq(&self.destroy_events, &proof.events)
            && u64::try_from(self.destroy_events.len()).ok() == Some(self.operations)
    }

    pub(crate) fn certify_inert_events(
        &self,
        mut complete_inert_source: impl FnMut(ChunkPos) -> Result<bool>,
    ) -> Result<InertGeneratedDestroyEvents> {
        ensure!(
            !self.destroy_events.is_empty(),
            "no generated destroy events"
        );
        ensure!(
            u64::try_from(self.destroy_events.len()).ok() == Some(self.operations),
            "generated drop operations differ from destroy events"
        );
        let mut checked = BTreeSet::new();
        for event in self.destroy_events.iter() {
            let x = i64::from(event.position[0]);
            let z = i64::from(event.position[2]);
            for cx in (x - 16).div_euclid(16)..=(x + 16).div_euclid(16) {
                for cz in (z - 16).div_euclid(16)..=(z + 16).div_euclid(16) {
                    let chunk = ChunkPos {
                        x: i32::try_from(cx)?,
                        z: i32::try_from(cz)?,
                    };
                    ensure!(
                        event.registered_chunks.contains(&chunk)
                            && event.world.chunks.contains_key(&chunk),
                        "event listener chunk is unavailable"
                    );
                    ensure!(
                        event.block_entities.get(&chunk).is_some_and(|records| {
                            block_entities_without_listeners(&event.world, chunk, records)
                        }),
                        "event block listeners are not known to be absent"
                    );
                    ensure!(
                        event
                            .entities
                            .get(&chunk)
                            .and_then(Option::as_ref)
                            .is_some_and(|records| entities_without_listeners(records)),
                        "event entity listeners are not known to be absent"
                    );
                    if checked.insert(chunk) {
                        ensure!(
                            complete_inert_source(chunk)?,
                            "event entity coverage is incomplete or may contain listeners"
                        );
                    }
                }
            }
        }
        Ok(InertGeneratedDestroyEvents {
            events: Arc::clone(&self.destroy_events),
        })
    }

    pub(crate) fn record_destroy_event(
        &mut self,
        position: [i32; 3],
        state: &BlockState,
        world: Arc<World>,
        registered_chunks: &[ChunkPos],
        block_entities: BTreeMap<ChunkPos, Vec<Tag>>,
        entities: BTreeMap<ChunkPos, Option<Arc<Vec<Tag>>>>,
    ) {
        let generated_items: BTreeMap<_, _> = self
            .records
            .iter()
            .filter(|(chunk, _)| block_entities.contains_key(chunk))
            .map(|(&chunk, records)| (chunk, Arc::clone(records)))
            .collect();
        let registered_chunks = self
            .destroy_events
            .last()
            .filter(|event| event.registered_chunks.as_ref() == registered_chunks)
            .map(|event| Arc::clone(&event.registered_chunks))
            .unwrap_or_else(|| registered_chunks.into());
        let block_entities = self
            .destroy_events
            .last()
            .filter(|event| event.block_entities.as_ref() == &block_entities)
            .map(|event| Arc::clone(&event.block_entities))
            .unwrap_or_else(|| Arc::new(block_entities));
        let entities = self
            .destroy_events
            .last()
            .filter(|event| event.entities.as_ref() == &entities)
            .map(|event| Arc::clone(&event.entities))
            .unwrap_or_else(|| Arc::new(entities));
        let generated_items = self
            .destroy_events
            .last()
            .filter(|event| event.generated_items.as_ref() == &generated_items)
            .map(|event| Arc::clone(&event.generated_items))
            .unwrap_or_else(|| Arc::new(generated_items));
        Arc::make_mut(&mut self.destroy_events).push(GeneratedBlockDestroyEvent {
            position,
            state: state.clone(),
            world,
            registered_chunks,
            block_entities,
            entities,
            generated_items,
        });
    }

    /// Append item records to a caller-supplied complete entity snapshot.
    /// This validates identities in every supplied chunk, including passengers.
    /// The caller must include every potentially conflicting entity chunk and
    /// still coordinate the saved random streams and remaining world effects.
    pub fn merge_complete_entities(
        &self,
        original: &BTreeMap<ChunkPos, Arc<Vec<Tag>>>,
    ) -> Result<BTreeMap<ChunkPos, Arc<Vec<Tag>>>> {
        if self.records.is_empty() {
            return Ok(original.clone());
        }
        ensure!(
            self.records.keys().all(|pos| original.contains_key(pos)),
            "generated item destination entity chunk is missing"
        );
        let mut seen = BTreeSet::new();
        for records in original.values() {
            let mut pending: Vec<(&Tag, usize)> = records.iter().map(|tag| (tag, 0)).collect();
            while let Some((entity, depth)) = pending.pop() {
                ensure!(
                    depth <= 512,
                    "generated entity passenger nesting is too deep"
                );
                let Tag::Compound(fields) = entity else {
                    anyhow::bail!("generated entity must be a compound")
                };
                ensure!(
                    fields
                        .iter()
                        .filter(|(name, _)| name.matches("UUID"))
                        .count()
                        <= 1,
                    "duplicate entity UUID field"
                );
                let uuid = crate::saved_item::optional_uuid(entity, "UUID")?
                    .context("existing entity UUID is unavailable")?;
                ensure!(seen.insert(uuid), "duplicate existing entity UUID");
                let mut passengers = fields.iter().filter(|(name, _)| name.matches("Passengers"));
                if let Some((_, value)) = passengers.next() {
                    ensure!(passengers.next().is_none(), "duplicate Passengers field");
                    match value {
                        Tag::List {
                            element_type: native_persistence::nbt::TagType::Compound,
                            values,
                        } => {
                            pending.extend(values.iter().map(|tag| (tag, depth + 1)));
                        }
                        Tag::List {
                            element_type: native_persistence::nbt::TagType::End,
                            values,
                        } if values.is_empty() => {}
                        _ => anyhow::bail!("generated entity Passengers must be a compound list"),
                    }
                }
            }
        }
        let mut merged = original.clone();
        for (&chunk, additions) in self.records.iter() {
            for entity in additions.iter() {
                let item =
                    crate::saved_item::read(entity)?.context("generated drop is not an item")?;
                let uuid = item.uuid.context("generated item UUID is missing")?;
                ensure!(
                    seen.insert(uuid),
                    "generated item UUID conflicts with another entity"
                );
                ensure!(
                    item.position.is_finite()
                        && item.position.x.abs() <= 30_000_512.0
                        && item.position.z.abs() <= 30_000_512.0,
                    "generated item position is invalid"
                );
                let destination = ChunkPos {
                    x: (item.position.x.floor() as i32).div_euclid(16),
                    z: (item.position.z.floor() as i32).div_euclid(16),
                };
                ensure!(
                    destination == chunk,
                    "generated item belongs to a different chunk"
                );
            }
            Arc::make_mut(merged.get_mut(&chunk).expect("destination prechecked"))
                .extend(additions.iter().cloned());
        }
        Ok(merged)
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
    fn listener_free_entities_require_known_ids_and_no_passengers() {
        use native_persistence::nbt::TagType;
        let entity = |id: &str| Tag::Compound(vec![("id".into(), Tag::String(id.into()))]);
        for id in [
            "minecraft:chicken",
            "minecraft:cow",
            "minecraft:pig",
            "minecraft:sheep",
            "minecraft:item",
        ] {
            assert!(entities_without_listeners(&[entity(id)]), "{id}");
        }
        assert!(entities_without_listeners(&[]));
        assert!(!entities_without_listeners(&[entity("minecraft:warden")]));
        assert!(!entities_without_listeners(&[entity("example:cow")]));
        assert!(!entities_without_listeners(&[Tag::Compound(vec![])]));
        assert!(!entities_without_listeners(&[Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:cow".into())),
            ("id".into(), Tag::String("minecraft:warden".into())),
        ])]));
        for (kind, values, expected) in [
            (TagType::End, vec![], true),
            (TagType::Compound, vec![], true),
            (TagType::Compound, vec![entity("minecraft:warden")], false),
            (TagType::Int, vec![], false),
        ] {
            let record = Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:cow".into())),
                (
                    "Passengers".into(),
                    Tag::List {
                        element_type: kind,
                        values,
                    },
                ),
            ]);
            assert_eq!(entities_without_listeners(&[record]), expected);
        }
    }

    #[test]
    fn listener_free_block_entities_require_matching_blocks_and_unique_positions() {
        use native_world::loaded::{BLOCK_COUNT, Chunk, Section};
        let chunk = ChunkPos { x: -1, z: 0 };
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str| BlockState {
            name: name.into(),
            properties: BTreeMap::new(),
        };
        let chest = world.intern_state(state("minecraft:chest"));
        let spawner = world.intern_state(state("minecraft:spawner"));
        let mut blocks = Box::new([chest; BLOCK_COUNT]);
        blocks[1] = spawner;
        world.chunks.insert(
            chunk,
            Arc::new(Chunk {
                position: chunk,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
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
        let record = |id: &str, x: i32| {
            Tag::Compound(vec![
                ("id".into(), Tag::String(id.into())),
                ("x".into(), Tag::Int(x)),
                ("y".into(), Tag::Int(0)),
                ("z".into(), Tag::Int(0)),
            ])
        };
        assert!(block_entities_without_listeners(
            &world,
            chunk,
            &[
                record("minecraft:chest", -16),
                record("minecraft:mob_spawner", -15),
            ]
        ));
        assert!(!block_entities_without_listeners(
            &world,
            chunk,
            &[record("minecraft:chest", -15)]
        ));
        assert!(!block_entities_without_listeners(
            &world,
            chunk,
            &[record("minecraft:sculk_sensor", -16)]
        ));
        assert!(!block_entities_without_listeners(
            &world,
            chunk,
            &[record("minecraft:chest", 0)]
        ));
        let Tag::Compound(mut fields) = record("minecraft:chest", -16) else {
            unreachable!()
        };
        fields.push(("x".into(), Tag::Int(-16)));
        assert!(!block_entities_without_listeners(
            &world,
            chunk,
            &[Tag::Compound(fields)]
        ));
        assert!(!block_entities_without_listeners(
            &World::new("minecraft:overworld"),
            chunk,
            &[record("minecraft:chest", -16)]
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn listener_free_entity_classes_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"]
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
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("GeneratedEntityListenerOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedEntityListenerOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let checked = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedEntityListenerOracle")
            .output()?;
        ensure!(
            checked.status.success(),
            "Java listener check failed: {}",
            String::from_utf8_lossy(&checked.stderr)
        );
        let disassembled = Command::new(java.join("javap.exe"))
            .arg("-c")
            .arg("-classpath")
            .arg(&cp)
            .arg("net.minecraft.world.entity.Entity")
            .output()?;
        ensure!(
            disassembled.status.success(),
            "javap failed: {}",
            String::from_utf8_lossy(&disassembled.stderr)
        );
        let text = String::from_utf8(disassembled.stdout)?;
        let method = text
            .split("public void updateDynamicGameEventListener(")
            .nth(1)
            .context("base listener method missing")?;
        let code = method
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        ensure!(
            code.lines()
                .filter(|line| line.trim_start().starts_with(|c: char| c.is_ascii_digit()))
                .map(str::trim)
                .collect::<Vec<_>>()
                == ["0: return"],
            "base entity listener registration is not a no-op: {code}"
        );
        Ok(())
    }

    #[test]
    fn inert_destroy_proof_requires_complete_empty_radius_and_tracks_sequence() {
        use native_world::loaded::Chunk;
        let chunks: Vec<_> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos { x, z }))
            .collect();
        let mut world = World::new("minecraft:overworld");
        for &position in &chunks {
            world.chunks.insert(
                position,
                Arc::new(Chunk {
                    position,
                    status: Some("minecraft:full".into()),
                    block_light_correct: true,
                    sections: BTreeMap::new(),
                }),
            );
        }
        let block_entities = chunks
            .iter()
            .copied()
            .map(|pos| (pos, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let entities = chunks
            .iter()
            .copied()
            .map(|pos| (pos, Some(Arc::new(Vec::new()))))
            .collect::<BTreeMap<_, _>>();
        let mut drops = fixture(true);
        drops.drop_block(IVec3::new(1, 5, 1), &ladder()).unwrap();
        drops.record_destroy_event(
            [1, 5, 1],
            &ladder(),
            Arc::new(world),
            &chunks,
            block_entities,
            entities,
        );
        assert!(drops.certify_inert_events(|pos| Ok(pos.x != 1)).is_err());
        let mut unknown = drops.clone();
        let event = &mut Arc::make_mut(&mut unknown.destroy_events)[0];
        Arc::make_mut(&mut event.entities).insert(chunks[0], None);
        assert!(unknown.certify_inert_events(|_| Ok(true)).is_err());
        let mut passive = drops.clone();
        let event = &mut Arc::make_mut(&mut passive.destroy_events)[0];
        Arc::make_mut(&mut event.entities).insert(
            chunks[0],
            Some(Arc::new(vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:cow".into()),
            )])])),
        );
        assert!(passive.certify_inert_events(|_| Ok(true)).is_ok());
        let event = &mut Arc::make_mut(&mut passive.destroy_events)[0];
        Arc::make_mut(&mut event.entities).insert(
            chunks[0],
            Some(Arc::new(vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:warden".into()),
            )])])),
        );
        assert!(passive.certify_inert_events(|_| Ok(true)).is_err());
        let mut with_chest = drops.clone();
        let event = &mut Arc::make_mut(&mut with_chest.destroy_events)[0];
        let world = Arc::make_mut(&mut event.world);
        let chest = world.intern_state(BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::new(),
        });
        let center = ChunkPos { x: 0, z: 0 };
        Arc::make_mut(world.chunks.get_mut(&center).unwrap())
            .sections
            .insert(
                0,
                Arc::new(native_world::loaded::Section {
                    blocks: Some(Box::new([chest; native_world::loaded::BLOCK_COUNT])),
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                }),
            );
        Arc::make_mut(&mut event.block_entities)
            .get_mut(&center)
            .unwrap()
            .push(Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(0)),
                ("y".into(), Tag::Int(0)),
                ("z".into(), Tag::Int(0)),
            ]));
        assert!(with_chest.certify_inert_events(|_| Ok(true)).is_ok());
        let mut listener = drops.clone();
        let event = &mut Arc::make_mut(&mut listener.destroy_events)[0];
        Arc::make_mut(&mut event.block_entities)
            .get_mut(&chunks[0])
            .unwrap()
            .push(Tag::Compound(vec![]));
        assert!(listener.certify_inert_events(|_| Ok(true)).is_err());
        let mut incomplete = drops.clone();
        let event = &mut Arc::make_mut(&mut incomplete.destroy_events)[0];
        Arc::make_mut(&mut event.block_entities).remove(&chunks[0]);
        assert!(incomplete.certify_inert_events(|_| Ok(true)).is_err());
        let proof = drops.certify_inert_events(|_| Ok(true)).unwrap();
        assert!(drops.verify_inert_proof(&proof));
        let mut changed = drops.clone();
        changed.drop_block(IVec3::new(2, 5, 1), &ladder()).unwrap();
        assert!(!changed.verify_inert_proof(&proof));
        assert!(changed.certify_inert_events(|_| Ok(true)).is_err());
    }

    #[test]
    fn destroy_event_retains_only_items_and_entities_present_at_emission() {
        let mut drops = fixture(true);
        let chunk = ChunkPos { x: 0, z: 0 };
        let mut entities = Arc::new(vec![Tag::Int(1)]);
        drops.drop_block(IVec3::new(1, 5, 1), &ladder()).unwrap();
        drops.record_destroy_event(
            [1, 5, 1],
            &ladder(),
            Arc::new(World::new("minecraft:overworld")),
            &[chunk],
            BTreeMap::from([(chunk, Vec::new())]),
            BTreeMap::from([(chunk, Some(Arc::clone(&entities)))]),
        );
        drops.drop_block(IVec3::new(2, 5, 1), &ladder()).unwrap();
        Arc::make_mut(&mut entities).push(Tag::Int(2));
        let event = &drops.destroy_events()[0];
        assert_eq!(event.generated_items()[&chunk].len(), 1);
        assert_eq!(drops.records(chunk).len(), 2);
        assert_eq!(
            event.entities()[&chunk].as_deref(),
            Some(&vec![Tag::Int(1)])
        );
    }

    #[test]
    fn destroy_event_retains_emission_world_and_registered_chunk_order() {
        let mut drops = fixture(true);
        let chunk = ChunkPos { x: -1, z: 0 };
        let mut world = Arc::new(World::new("minecraft:overworld"));
        let mut registered = vec![chunk];
        drops.record_destroy_event(
            [-1, 5, 0],
            &ladder(),
            Arc::clone(&world),
            &registered,
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let earlier = drops.clone();
        Arc::make_mut(&mut world).dimension = "example:later".into();
        registered.push(ChunkPos { x: 0, z: 0 });
        drops.record_destroy_event(
            [0, 5, 0],
            &ladder(),
            world,
            &registered,
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let world = Arc::clone(drops.destroy_events()[1].world());
        drops.record_destroy_event(
            [1, 5, 0],
            &ladder(),
            world,
            &registered,
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(earlier.destroy_events().len(), 1);
        assert_eq!(
            earlier.destroy_events()[0].world().dimension,
            "minecraft:overworld"
        );
        assert_eq!(earlier.destroy_events()[0].registered_chunks(), &[chunk]);
        assert_eq!(drops.destroy_events().len(), 3);
        assert_eq!(drops.destroy_events()[1].world().dimension, "example:later");
        assert_eq!(drops.destroy_events()[1].registered_chunks(), registered);
        assert!(Arc::ptr_eq(
            &drops.destroy_events()[1].registered_chunks,
            &drops.destroy_events()[2].registered_chunks,
        ));
        assert!(Arc::ptr_eq(
            &drops.destroy_events()[1].block_entities,
            &drops.destroy_events()[2].block_entities,
        ));
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
        assert_eq!(enabled.items(chunk).len(), 1);
        assert!(disabled.records(chunk).is_empty());
        assert!(disabled.items(chunk).is_empty());
        let saved = crate::saved_item::read(&enabled.records(chunk)[0])
            .unwrap()
            .unwrap();
        assert_eq!(saved.position.to_array(), expected_position);
        assert_eq!(saved.stack.id(), "minecraft:ladder");
        assert_eq!(saved.uuid, Some(enabled.items(chunk)[0].uuid()));
        assert_eq!(saved.position, enabled.items(chunk)[0].item().position);
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
        assert_eq!(fork.items(chunk).len(), 1);
        assert!(original.records(chunk).is_empty());
        assert!(original.items(chunk).is_empty());
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
        assert_eq!(original.items(chunk)[0].uuid(), fork.items(chunk)[0].uuid());
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

    #[test]
    fn generated_items_preserve_creation_order_across_chunks_and_forks() {
        let mut drops = fixture(true);
        let positions = [
            IVec3::new(17, 5, 0),
            IVec3::new(-1, 5, 0),
            IVec3::new(18, 5, 0),
        ];
        for pos in positions {
            drops.drop_block(pos, &ladder()).unwrap();
        }
        let snapshot = drops.clone();
        let expected = [
            ChunkPos { x: 1, z: 0 },
            ChunkPos { x: -1, z: 0 },
            ChunkPos { x: 1, z: 0 },
        ];
        assert_eq!(
            drops
                .items_in_spawn_order()
                .map(|(chunk, _)| chunk)
                .collect::<Vec<_>>(),
            expected
        );
        let ids: Vec<_> = drops
            .items_in_spawn_order()
            .map(|(_, item)| item.uuid())
            .collect();
        drops.drop_block(IVec3::new(0, 5, 0), &ladder()).unwrap();
        assert_eq!(
            snapshot
                .items_in_spawn_order()
                .map(|(_, item)| item.uuid())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn entity_merge_preserves_records_and_rejects_missing_chunks_or_conflicting_passengers() {
        use native_persistence::nbt::TagType;
        let mut generated = fixture(true);
        let destination = ChunkPos { x: -1, z: -2 };
        let other = ChunkPos { x: 0, z: 0 };
        generated
            .drop_block(IVec3::new(-1, 70, -17), &ladder())
            .unwrap();
        let item_uuid =
            crate::saved_item::optional_uuid(&generated.records(destination)[0], "UUID")
                .unwrap()
                .unwrap();
        let entity = |uuid: [i32; 4]| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:pig".into())),
                ("UUID".into(), Tag::IntArray(uuid.into())),
            ])
        };
        let first = Arc::new(vec![entity([1, 2, 3, 4])]);
        let untouched = Arc::new(vec![entity([5, 6, 7, 8])]);
        let source = BTreeMap::from([
            (destination, Arc::clone(&first)),
            (other, Arc::clone(&untouched)),
        ]);
        let merged = generated.merge_complete_entities(&source).unwrap();
        assert_eq!(merged[&destination].len(), 2);
        assert_eq!(merged[&destination][0], first[0]);
        assert_eq!(merged[&destination][1], generated.records(destination)[0]);
        assert_eq!(source[&destination].len(), 1);
        assert!(Arc::ptr_eq(&merged[&other], &untouched));
        let document = native_persistence::generated_chunk::assemble_entity_chunk(
            destination,
            &merged[&destination],
        )
        .unwrap();
        let bytes = native_persistence::nbt::write(&document).unwrap();
        assert_eq!(native_persistence::nbt::read(&bytes).unwrap(), document);
        assert!(generated.merge_complete_entities(&BTreeMap::new()).is_err());
        let passenger = Tag::Compound(vec![
            ("UUID".into(), Tag::IntArray([9, 10, 11, 12].into())),
            (
                "Passengers".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![entity(item_uuid)],
                },
            ),
        ]);
        assert!(
            generated
                .merge_complete_entities(&BTreeMap::from([(
                    destination,
                    Arc::new(vec![passenger])
                )]))
                .is_err()
        );
        let missing_uuid = Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]);
        assert!(
            generated
                .merge_complete_entities(&BTreeMap::from([(
                    destination,
                    Arc::new(vec![missing_uuid])
                )]))
                .is_err()
        );
        let duplicate_uuid = Tag::Compound(vec![
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            ("UUID".into(), Tag::IntArray(vec![5, 6, 7, 8])),
        ]);
        assert!(
            generated
                .merge_complete_entities(&BTreeMap::from([(
                    destination,
                    Arc::new(vec![duplicate_uuid])
                )]))
                .is_err()
        );
        assert!(
            generated
                .merge_complete_entities(&BTreeMap::from([
                    (destination, Arc::clone(&first)),
                    (other, Arc::new(vec![entity(item_uuid)])),
                ]))
                .is_err()
        );
        assert_eq!(source[&destination].len(), 1);
    }

    #[test]
    fn entity_merge_without_items_preserves_unknown_identity_and_shares_records() {
        let generated = fixture(false);
        let chunk = ChunkPos { x: 0, z: 0 };
        let records = Arc::new(vec![Tag::Compound(vec![])]);
        let source = BTreeMap::from([(chunk, Arc::clone(&records))]);
        let merged = generated.merge_complete_entities(&source).unwrap();
        assert!(Arc::ptr_eq(&merged[&chunk], &records));
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
