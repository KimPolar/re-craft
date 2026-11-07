use anyhow::{Context, Result, ensure};
use native_core::{
    beardifier::{BoundingBox, TerrainAdjustment},
    random::DecorationRandom,
};
use native_persistence::{
    nbt::{NamedTag, Tag},
    structure_template::{StructureTemplate, TemplateBlock},
};
use native_resources::{
    packs::ResourceStack,
    shipwreck::{HeightWorld, MarkerWorld, Piece},
    structure_start::Heightmap,
    structure_transform::Rotation,
    tree_leaf_update::ShapeEdgeWorld,
};
use native_session::worldgen::{
    jigsaw::{JigsawPlacementWorld, StructurePlacementWorld},
    shipwreck::{PreparedStart, ShipwreckPlacementWorld},
};
use native_world::{fluid::FluidState, loaded::BlockState};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Default)]
struct Draft {
    blocks: BTreeMap<[i32; 3], BlockState>,
    loot: Option<([i32; 3], String, i64)>,
    fail: bool,
}
fn air() -> BlockState {
    BlockState {
        name: "minecraft:air".into(),
        properties: BTreeMap::new(),
    }
}
impl HeightWorld for Draft {
    fn max_y(&self) -> i32 {
        319
    }
    fn height(&mut self, map: Heightmap, _: i32, _: i32) -> Result<i32> {
        assert_eq!(map, Heightmap::OceanFloor);
        Ok(60)
    }
}
impl MarkerWorld for Draft {
    fn is_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(self
            .blocks
            .get(&pos)
            .is_some_and(|v| v.name == "minecraft:chest"))
    }
    fn set_loot_table(&mut self, pos: [i32; 3], table: &str, seed: i64) -> Result<()> {
        self.loot = Some((pos, table.into(), seed));
        Ok(())
    }
}
impl JigsawPlacementWorld for Draft {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        Ok(self.blocks.get(&pos).cloned().unwrap_or_else(air))
    }
    fn fluid_at(&mut self, _: [i32; 3]) -> Result<Option<FluidState>> {
        Ok(None)
    }
    fn fluid_of_state(&mut self, _: &BlockState) -> Result<Option<FluidState>> {
        Ok(None)
    }
    fn place_template_block(
        &mut self,
        _: [i32; 3],
        _: &BlockState,
        _: Option<&Tag>,
        _: &mut DecorationRandom,
    ) -> Result<bool> {
        anyhow::bail!("unexpected jigsaw placement")
    }
    fn is_liquid_container(&mut self, _: &BlockState) -> Result<bool> {
        Ok(false)
    }
    fn place_liquid(&mut self, _: [i32; 3], _: &BlockState, _: Option<FluidState>) -> Result<()> {
        Ok(())
    }
    fn mark_block_entity_changed(&mut self, _: [i32; 3]) -> Result<()> {
        Ok(())
    }
    fn place_template_entities(
        &mut self,
        _: &[Tag],
        _: [i32; 3],
        _: Rotation,
        _: BoundingBox,
    ) -> Result<()> {
        anyhow::bail!("unexpected jigsaw entities")
    }
}
impl ShapeEdgeWorld for Draft {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(self, pos)
    }
    fn update_shape(
        &mut self,
        _: [i32; 3],
        state: &BlockState,
        _: [i32; 3],
        _: [i32; 3],
        _: &BlockState,
    ) -> Result<BlockState> {
        Ok(state.clone())
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, _: i32) -> Result<()> {
        self.blocks.insert(pos, state.clone());
        Ok(())
    }
}
impl StructurePlacementWorld for Draft {
    fn place_structure_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        _: Option<&Tag>,
        _: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.fail, "injected placement failure");
        self.blocks.insert(pos, state.clone());
        Ok(true)
    }
    fn update_from_neighbor_shapes(
        &mut self,
        _: [i32; 3],
        state: &BlockState,
    ) -> Result<BlockState> {
        Ok(state.clone())
    }
    fn update_neighbors_at(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        Ok(())
    }
}
impl ShipwreckPlacementWorld for Draft {}

#[test]
#[ignore = "requires installed Minecraft 26.3"]
fn shipwreck_post_process_places_marker_and_preserves_staged_state_on_error() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let palette = |id: &str| Tag::Compound(vec![("id".into(), Tag::String(id.into()))]);
    let marker = Tag::Compound(vec![
        ("mode".into(), Tag::String("DATA".into())),
        ("metadata".into(), Tag::String("map_chest".into())),
    ]);
    let template = Arc::new(StructureTemplate {
        document: NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        },
        data_version: Some(5023),
        size: [3, 2, 1],
        palettes: vec![vec![
            palette("minecraft:stone"),
            palette("minecraft:chest"),
            palette("minecraft:structure_block"),
            palette("minecraft:air"),
        ]],
        blocks: vec![
            TemplateBlock {
                position: [0, 0, 0],
                state: 0,
                nbt: None,
            },
            TemplateBlock {
                position: [1, 0, 0],
                state: 1,
                nbt: Some(Tag::Compound(vec![])),
            },
            TemplateBlock {
                position: [1, 1, 0],
                state: 2,
                nbt: Some(marker),
            },
            TemplateBlock {
                position: [2, 0, 0],
                state: 3,
                nbt: None,
            },
        ],
        entities: vec![],
    });
    let piece = Piece {
        template: "minecraft:shipwreck/with_mast".into(),
        position: [0, 90, 0],
        rotation: Rotation::None,
        beached: false,
    };
    let bounds = piece.bounds(template.size)?;
    let mut start = PreparedStart {
        chunk: [0, 0],
        generation_point: [0, 60, 0],
        terrain_adjustment: TerrainAdjustment::None,
        piece,
        template,
        bounds,
        height_adjusted: false,
    };
    let clip = BoundingBox::new([-100, -64, -100], [100, 319, 100]).map_err(anyhow::Error::msg)?;
    let mut random = DecorationRandom::new(42);
    let mut expected = random.clone();
    let mut failed = Draft {
        fail: true,
        ..Default::default()
    };
    assert!(
        start
            .post_process(
                &mut failed,
                &registry,
                clip,
                |s| Ok(s.name == "minecraft:stone"),
                &mut random
            )
            .is_err()
    );
    assert_eq!(start.piece.position, [0, 90, 0]);
    assert!(!start.height_adjusted);
    assert_eq!(random.next_long(), expected.next_long());
    let mut draft = Draft::default();
    assert!(start.post_process(
        &mut draft,
        &registry,
        clip,
        |s| Ok(s.name == "minecraft:stone"),
        &mut random
    )?);
    assert_eq!(start.piece.position, [0, 60, 0]);
    assert!(start.height_adjusted);
    assert_eq!(draft.blocks.len(), 2);
    assert_eq!(draft.blocks[&[0, 60, 0]].name, "minecraft:stone");
    assert_eq!(draft.blocks[&[1, 60, 0]].name, "minecraft:chest");
    let (pos, table, _) = draft.loot.context("missing loot")?;
    assert_eq!(pos, [1, 60, 0]);
    assert_eq!(table, "minecraft:chests/shipwreck_map");
    Ok(())
}
