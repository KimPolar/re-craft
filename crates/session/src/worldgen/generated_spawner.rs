//! Full NBT for the freshly created monster-room spawner block entity.
use anyhow::Result;
use native_persistence::{
    edit::{BlockEntityMutation, ChunkDraft},
    nbt::{Tag, TagType},
};
use native_resources::monster_room::RoomMob;

#[derive(Clone, Debug)]
pub struct GeneratedMonsterSpawner {
    pub position: [i32; 3],
    pub mob: RoomMob,
}
impl GeneratedMonsterSpawner {
    pub fn block_entity(&self) -> Tag {
        let id = match self.mob {
            RoomMob::Skeleton => "minecraft:skeleton",
            RoomMob::Zombie => "minecraft:zombie",
            RoomMob::Spider => "minecraft:spider",
        };
        fresh_spawner_tag(self.position, id)
    }
    /// Stage the entity; its corresponding block change remains the caller's responsibility.
    pub fn stage_block_entity(&self, draft: &mut ChunkDraft) -> Result<()> {
        draft.stage_block_entity_mutations(&[BlockEntityMutation {
            expected: None,
            replacement: Some(self.block_entity()),
        }])
    }
}

/// Fresh cave-spider spawner created by a mineshaft corridor.
#[derive(Clone, Debug)]
pub struct GeneratedCaveSpiderSpawner {
    pub position: [i32; 3],
}

impl GeneratedCaveSpiderSpawner {
    pub fn block_entity(&self) -> Tag {
        fresh_spawner_tag(self.position, "minecraft:cave_spider")
    }

    /// Stage the entity; the matching spawner block remains the caller's responsibility.
    pub fn stage_block_entity(&self, draft: &mut ChunkDraft) -> Result<()> {
        draft.stage_block_entity_mutations(&[BlockEntityMutation {
            expected: None,
            replacement: Some(self.block_entity()),
        }])
    }
}

fn fresh_spawner_tag([x, y, z]: [i32; 3], id: &str) -> Tag {
    Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:mob_spawner".into())),
        ("x".into(), Tag::Int(x)),
        ("y".into(), Tag::Int(y)),
        ("z".into(), Tag::Int(z)),
        ("Delay".into(), Tag::Short(20)),
        ("MinSpawnDelay".into(), Tag::Short(200)),
        ("MaxSpawnDelay".into(), Tag::Short(800)),
        ("SpawnCount".into(), Tag::Short(4)),
        ("MaxNearbyEntities".into(), Tag::Short(6)),
        ("RequiredPlayerRange".into(), Tag::Short(16)),
        ("SpawnRange".into(), Tag::Short(4)),
        (
            "SpawnData".into(),
            Tag::Compound(vec![(
                "entity".into(),
                Tag::Compound(vec![("id".into(), Tag::String(id.into()))]),
            )]),
        ),
        (
            "SpawnPotentials".into(),
            Tag::List {
                element_type: TagType::End,
                values: vec![],
            },
        ),
        ("components".into(), Tag::Compound(vec![])),
    ])
}
