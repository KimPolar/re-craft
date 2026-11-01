//! Deferred-loot chest data for intermediate structure-generation drafts.
use anyhow::Result;
use native_persistence::{
    edit::{BlockChange, BlockEntityMutation, ChunkDraft},
    nbt::Tag,
    palette::BlockState,
};
use native_resources::{
    packs::ResourceId, structure_piece::BuriedTreasureOutcome, structure_start::HorizontalDirection,
};

/// A chest that has not yet generated its loot. Its seed must not be replaced
/// by an empty inventory or used to generate items before Java would do so.
#[derive(Clone, Debug)]
pub struct GeneratedLootChest {
    pub position: [i32; 3],
    pub facing: HorizontalDirection,
    loot_table: ResourceId,
    pub loot_seed: i64,
}
impl GeneratedLootChest {
    /// Convert a successfully created chest entity into its deferred-loot data.
    pub fn from_buried_treasure(outcome: BuriedTreasureOutcome) -> Option<Self> {
        match outcome {
            BuriedTreasureOutcome::Found {
                position,
                chest_placed: true,
                facing: Some(facing),
                loot_seed: Some(loot_seed),
            } => Some(Self {
                position,
                facing,
                loot_table: ResourceId::parse("minecraft:chests/buried_treasure")
                    .expect("valid built-in loot table"),
                loot_seed,
            }),
            _ => None,
        }
    }
    pub fn new(
        position: [i32; 3],
        facing: HorizontalDirection,
        loot_table: &str,
        loot_seed: i64,
    ) -> Result<Self> {
        Ok(Self {
            position,
            facing,
            loot_table: ResourceId::parse(loot_table)?,
            loot_seed,
        })
    }
    /// Registry state of Java's default single, non-waterlogged chest.
    pub fn block_state(&self) -> BlockState {
        let facing = match self.facing {
            HorizontalDirection::North => "north",
            HorizontalDirection::East => "east",
            HorizontalDirection::South => "south",
            HorizontalDirection::West => "west",
        };
        BlockState {
            name: "minecraft:chest".into(),
            properties: vec![
                ("facing".into(), facing.into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ],
        }
    }
    pub fn block_change(&self, expected: BlockState) -> BlockChange {
        let [x, y, z] = self.position;
        BlockChange {
            x,
            y,
            z,
            expected,
            replacement: self.block_state(),
        }
    }
    /// 26.3 full block-entity NBT. Java omits a zero loot seed.
    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let table = format!("{}:{}", self.loot_table.namespace, self.loot_table.path);
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("LootTable".into(), Tag::String(table.as_str().into())),
            ("components".into(), Tag::Compound(Vec::new())),
        ];
        if self.loot_seed != 0 {
            fields.push(("LootTableSeed".into(), Tag::Long(self.loot_seed)));
        }
        Tag::Compound(fields)
    }
    /// Add the entity to an incomplete chunk draft. Its block change, lighting,
    /// heightmaps and other dependent data still need a coordinated save.
    pub fn stage_block_entity(&self, draft: &mut ChunkDraft) -> Result<()> {
        draft.stage_block_entity_mutations(&[BlockEntityMutation {
            expected: None,
            replacement: Some(self.block_entity()),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::{
        edit::draft_chunk_blocks,
        nbt::{NamedTag, TagType},
        world::ChunkPos,
    };
    #[test]
    fn stages_deferred_entity_without_losing_loot_data() {
        let chest = GeneratedLootChest::new(
            [-15, 64, 32],
            HorizontalDirection::West,
            "minecraft:chests/buried_treasure",
            -7,
        )
        .unwrap();
        let stone = BlockState {
            name: "minecraft:stone".into(),
            properties: vec![],
        };
        let change = chest.block_change(stone.clone());
        assert_eq!((change.x, change.y, change.z), (-15, 64, 32));
        assert_eq!(change.expected, stone);
        assert!(
            change
                .replacement
                .properties
                .iter()
                .any(|(k, v)| k.matches("facing") && v.matches("west"))
        );
        assert_eq!(
            chest.block_entity().get("LootTableSeed"),
            Some(&Tag::Long(-7))
        );
        let source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(-1)),
                ("zPos".into(), Tag::Int(2)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("Y".into(), Tag::Byte(4)),
                            (
                                "block_states".into(),
                                Tag::Compound(vec![(
                                    "palette".into(),
                                    Tag::List {
                                        element_type: TagType::String,
                                        values: vec![Tag::String("minecraft:stone".into())],
                                    },
                                )]),
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let mut draft = draft_chunk_blocks(&source, ChunkPos { x: -1, z: 2 }, &[change]).unwrap();
        chest.stage_block_entity(&mut draft).unwrap();
        let sections =
            native_persistence::palette::decode_sections(&draft.document().value).unwrap();
        assert_eq!(
            sections[0].blocks.as_ref().unwrap().block(1, 0, 0),
            Some(&chest.block_state())
        );
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_entities") else {
            panic!()
        };
        assert_eq!(values, &vec![chest.block_entity()]);
        let before = draft.document().clone();
        assert!(chest.stage_block_entity(&mut draft).is_err());
        assert_eq!(draft.document(), &before);
        let zero = GeneratedLootChest::new(
            [0, 0, 0],
            HorizontalDirection::North,
            "minecraft:chests/buried_treasure",
            0,
        )
        .unwrap();
        assert!(zero.block_entity().get("LootTableSeed").is_none());
        assert!(
            GeneratedLootChest::new(
                [0, 0, 0],
                HorizontalDirection::North,
                "BAD NAMESPACE:table",
                0
            )
            .is_err()
        );
        let placed = BuriedTreasureOutcome::Found {
            position: [-15, 64, 32],
            chest_placed: true,
            facing: Some(HorizontalDirection::West),
            loot_seed: Some(-7),
        };
        let converted = GeneratedLootChest::from_buried_treasure(placed).unwrap();
        assert_eq!(converted.block_entity(), chest.block_entity());
        assert_eq!(converted.block_state(), chest.block_state());
        assert!(
            GeneratedLootChest::from_buried_treasure(BuriedTreasureOutcome::NoFoundation).is_none()
        );
        assert!(
            GeneratedLootChest::from_buried_treasure(BuriedTreasureOutcome::Found {
                position: [0, 0, 0],
                chest_placed: true,
                facing: Some(HorizontalDirection::North),
                loot_seed: None
            })
            .is_none()
        );
    }
}
