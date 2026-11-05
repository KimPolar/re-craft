//! Isolated swamp-hut placement using generation-world block and entity drafts.
use super::*;
use native_core::beardifier::BoundingBox;
use native_resources::{
    structure_piece::swamp_hut::{HutBlock, HutMob, SwampHutState, SwampHutWorld, place_swamp_hut},
    structure_start::{HorizontalDirection, SimpleStructureStart},
};

fn hut_block(block: HutBlock) -> BlockState {
    let (name, properties): (&str, BTreeMap<String, String>) = match block {
        HutBlock::SprucePlanks => ("spruce_planks", BTreeMap::new()),
        HutBlock::OakLog => ("oak_log", BTreeMap::from([("axis".into(), "y".into())])),
        HutBlock::OakFence => (
            "oak_fence",
            ["north", "east", "south", "west", "waterlogged"]
                .into_iter()
                .map(|name| (name.into(), "false".into()))
                .collect(),
        ),
        HutBlock::Air => ("air", BTreeMap::new()),
        HutBlock::PottedRedMushroom => ("potted_red_mushroom", BTreeMap::new()),
        HutBlock::CraftingTable => ("crafting_table", BTreeMap::new()),
        HutBlock::Cauldron => ("cauldron", BTreeMap::new()),
        HutBlock::SpruceStairs { facing, shape } => (
            "spruce_stairs",
            BTreeMap::from([
                (
                    "facing".into(),
                    match facing {
                        HorizontalDirection::North => "north",
                        HorizontalDirection::East => "east",
                        HorizontalDirection::South => "south",
                        HorizontalDirection::West => "west",
                    }
                    .into(),
                ),
                ("half".into(), "bottom".into()),
                ("shape".into(), shape.into()),
                ("waterlogged".into(), "false".into()),
            ]),
        ),
    };
    BlockState {
        name: format!("minecraft:{name}"),
        properties,
    }
}

fn known_hut_block(state: &BlockState) -> bool {
    match state.name.as_str() {
        "minecraft:spruce_planks"
        | "minecraft:air"
        | "minecraft:potted_red_mushroom"
        | "minecraft:crafting_table"
        | "minecraft:cauldron" => state.properties.is_empty(),
        "minecraft:oak_log" => {
            state.properties.len() == 1
                && state.properties.get("axis").map(String::as_str) == Some("y")
        }
        "minecraft:oak_fence" => {
            state.properties.len() == 5
                && ["north", "east", "south", "west", "waterlogged"]
                    .iter()
                    .all(|key| state.properties.get(*key).map(String::as_str) == Some("false"))
        }
        "minecraft:spruce_stairs" => {
            state.properties.len() == 4
                && matches!(
                    state.properties.get("facing").map(String::as_str),
                    Some("north" | "east" | "south" | "west")
                )
                && matches!(
                    state.properties.get("shape").map(String::as_str),
                    Some("straight" | "outer_left" | "outer_right")
                )
                && state.properties.get("half").map(String::as_str) == Some("bottom")
                && state.properties.get("waterlogged").map(String::as_str) == Some("false")
        }
        _ => false,
    }
}

struct HutRegion<'a, 'b, F> {
    region: &'a mut DecorationRegion<'b>,
    spawn: &'a mut F,
}

impl DecorationRegion<'_> {
    /// Place on this isolated region. The callback must use the active level's
    /// difficulty and entity RNG and return the exact serialized addition.
    /// On error discard the region and restore callback-owned random state.
    pub fn place_swamp_hut<F>(
        &mut self,
        piece: &mut SimpleStructureStart,
        state: &mut SwampHutState,
        clip: BoundingBox,
        mut spawn: F,
    ) -> Result<bool>
    where
        F: FnMut(HutMob, [i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
    {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next_piece = piece.clone();
        let mut next_state = *state;
        self.failed = true;
        let result = place_swamp_hut(
            &mut next_piece,
            &mut next_state,
            &mut HutRegion {
                region: self,
                spawn: &mut spawn,
            },
            clip,
        )?;
        *piece = next_piece;
        *state = next_state;
        self.failed = false;
        Ok(result)
    }
}

impl<F> HutRegion<'_, '_, F> {
    fn write(&mut self, pos: [i32; 3], block: HutBlock) -> Result<()> {
        ensure!(
            self.region.writable.contains(&chunk(pos)),
            "hut write outside writable chunks"
        );
        ensure!(
            !self.region.is_outside(pos[1]),
            "hut write outside build height"
        );
        let old = self.region.read(pos)?;
        ensure!(
            disk_safe_solid(&old.name).is_some() || known_hut_block(&old),
            "unsupported hut replacement effects: {}",
            old.name
        );
        self.region.set_proto_block(pos, &hut_block(block))
    }
}

impl<F> SwampHutWorld for HutRegion<'_, '_, F>
where
    F: FnMut(HutMob, [i32; 3], &World) -> Result<Option<super::super::GeneratedEntityRecord>>,
{
    fn motion_blocking_no_leaves(&mut self, x: i32, z: i32) -> Result<i32> {
        self.region.final_height(3, x, z)
    }
    fn place_piece_block(&mut self, pos: [i32; 3], block: HutBlock) -> Result<()> {
        self.write(pos, block)?;
        // Every hut output is dry. StructurePiece schedules the post-write
        // fluid only when one remains, and marks oak fences for shape updates.
        if block == HutBlock::OakFence {
            DiskWorld::mark_postprocessing(self.region, pos)?;
        }
        Ok(())
    }
    fn min_y(&self) -> i32 {
        self.region.target.min_y
    }
    fn support_replaceable(&mut self, pos: [i32; 3]) -> Result<bool> {
        if self.region.is_outside(pos[1]) {
            return Ok(true);
        }
        let state = self.region.read(pos)?;
        if is_air(&state)
            || matches!(
                state.name.as_str(),
                "minecraft:water"
                    | "minecraft:lava"
                    | "minecraft:seagrass"
                    | "minecraft:tall_seagrass"
            )
            || valid_lichen(&state)
        {
            return Ok(true);
        }
        ensure!(
            disk_safe_solid(&state.name).is_some() || known_hut_block(&state),
            "unsupported hut foundation block: {}",
            state.name
        );
        Ok(false)
    }
    fn place_support_log(&mut self, pos: [i32; 3]) -> Result<()> {
        self.write(pos, HutBlock::OakLog)
    }
    fn spawn_structure_mob(&mut self, mob: HutMob, pos: [i32; 3]) -> Result<()> {
        ensure!(
            self.region.writable.contains(&chunk(pos)) && !self.region.is_outside(pos[1]),
            "hut entity outside writable generation bounds"
        );
        if let Some(record) = (self.spawn)(mob, pos, &self.region.world)? {
            self.region.record_generated_entity(pos, record)?;
        }
        Ok(())
    }
}
