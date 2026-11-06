//! Apply supported mineshaft pieces to an unpublished generation region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_resources::mineshaft::{
    MineshaftCrossingWorld, MineshaftGraphPiece, MineshaftGraphPieceKind,
    MineshaftPlacementOutcome, MineshaftPlacementWorld, MineshaftVariant, place_crossing,
    place_room_or_stairs,
};

struct MineshaftRegion<'a, 'b, F> {
    region: &'a mut DecorationRegion<'b>,
    blocking_biome: &'a mut F,
}

fn known_piece_block(state: &BlockState) -> bool {
    let p = &state.properties;
    match state.name.as_str() {
        "minecraft:oak_planks" | "minecraft:dark_oak_planks" | "minecraft:cobweb" => p.is_empty(),
        "minecraft:oak_log" | "minecraft:dark_oak_log" => {
            p.len() == 1 && matches!(p.get("axis").map(String::as_str), Some("x" | "y" | "z"))
        }
        "minecraft:oak_fence" | "minecraft:dark_oak_fence" => {
            p.len() == 5
                && ["north", "south", "east", "west", "waterlogged"]
                    .iter()
                    .all(|key| matches!(p.get(*key).map(String::as_str), Some("true" | "false")))
        }
        "minecraft:iron_chain" => {
            p.len() == 2
                && matches!(p.get("axis").map(String::as_str), Some("x" | "y" | "z"))
                && matches!(
                    p.get("waterlogged").map(String::as_str),
                    Some("true" | "false")
                )
        }
        "minecraft:rail" => {
            p.len() == 2
                && matches!(
                    p.get("waterlogged").map(String::as_str),
                    Some("true" | "false")
                )
                && matches!(
                    p.get("shape").map(String::as_str),
                    Some(
                        "north_south"
                            | "east_west"
                            | "ascending_east"
                            | "ascending_west"
                            | "ascending_north"
                            | "ascending_south"
                            | "south_east"
                            | "south_west"
                            | "north_west"
                            | "north_east"
                    )
                )
        }
        "minecraft:wall_torch" => {
            p.len() == 1
                && matches!(
                    p.get("facing").map(String::as_str),
                    Some("north" | "south" | "east" | "west")
                )
        }
        _ => false,
    }
}

impl DecorationRegion<'_> {
    /// Run one room, stairs or crossing against this unpublished region.
    /// The callback resolves the active MINESHAFT_BLOCKING biome tag using the
    /// same seeded block-biome lookup as the world. It must isolate its state
    /// when cloned. Any failed operation poisons the entire region.
    pub fn place_mineshaft_static_piece<F>(
        &mut self,
        piece: &MineshaftGraphPiece,
        variant: MineshaftVariant,
        clip: BoundingBox,
        blocking_biome: &mut F,
    ) -> Result<MineshaftPlacementOutcome>
    where
        F: Clone + FnMut([i32; 3], &World) -> Result<bool>,
    {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next_biome = blocking_biome.clone();
        self.failed = true;
        let mut region = MineshaftRegion {
            region: self,
            blocking_biome: &mut next_biome,
        };
        let result = match piece.kind {
            MineshaftGraphPieceKind::Room { .. } | MineshaftGraphPieceKind::Stairs => {
                place_room_or_stairs(piece, variant, &mut region, clip)
            }
            MineshaftGraphPieceKind::Crossing { .. } => {
                place_crossing(piece, variant, &mut region, clip)
            }
            _ => anyhow::bail!("mineshaft corridor needs its complete generation context"),
        }?;
        *blocking_biome = next_biome;
        self.failed = false;
        Ok(result)
    }

    /// Resolve the blocking tag from the active biome registry and the
    /// generation region's seeded block-biome neighborhood.
    pub fn place_mineshaft_static_piece_with_biomes(
        &mut self,
        piece: &MineshaftGraphPiece,
        variant: MineshaftVariant,
        clip: BoundingBox,
        biomes: &super::super::BiomeNeighborhood,
        blocking: &native_resources::mineshaft::MineshaftBlockingBiomes,
    ) -> Result<MineshaftPlacementOutcome> {
        ensure!(
            biomes.seed == self.target.seed
                && biomes.min_y == self.target.min_y
                && biomes.height == self.target.height,
            "mineshaft biome neighborhood does not match the terrain"
        );
        let mut resolve = |position: [i32; 3], _: &World| {
            blocking.blocks(biomes.biome_at_block(position[0], position[1], position[2])?)
        };
        self.place_mineshaft_static_piece(piece, variant, clip, &mut resolve)
    }
}

impl<F> MineshaftRegion<'_, '_, F> {
    fn block(&mut self, position: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, position)
    }

    fn write(&mut self, position: [i32; 3], state: BlockState) -> Result<()> {
        ensure!(
            self.region.writable.contains(&chunk(position)),
            "mineshaft write outside writable chunks"
        );
        ensure!(
            !self.region.is_outside(position[1]),
            "mineshaft write outside build height"
        );
        let old = self.region.read(position)?;
        // Do not discard an existing block entity or unknown block side effect.
        ensure!(
            !self.region.monster_entities.contains_key(&position)
                && !self.region.bee_nests.contains_key(&position),
            "mineshaft would replace a block entity"
        );
        ensure!(
            known_piece_block(&old) || monster_block_flags(&old).is_ok(),
            "unsupported mineshaft replacement effects: {}",
            old.name
        );
        self.region.set_proto_block(position, &state)
    }
}

impl<F> MineshaftPlacementWorld for MineshaftRegion<'_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<bool>,
{
    fn blocking_biome(&mut self, position: [i32; 3]) -> Result<bool> {
        (self.blocking_biome)(position, &self.region.world)
    }
    fn liquid_block(&mut self, position: [i32; 3]) -> Result<bool> {
        let state = self.block(position)?;
        if known_piece_block(&state) {
            return Ok(false);
        }
        DiskWorld::is_liquid(self.region, &state)
    }
    fn protected_block(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<bool> {
        let state = self.block(position)?;
        Ok(state.name == "minecraft:iron_chain"
            || match variant {
                MineshaftVariant::Normal => matches!(
                    state.name.as_str(),
                    "minecraft:oak_planks" | "minecraft:oak_log" | "minecraft:oak_fence"
                ),
                MineshaftVariant::Mesa => matches!(
                    state.name.as_str(),
                    "minecraft:dark_oak_planks"
                        | "minecraft:dark_oak_log"
                        | "minecraft:dark_oak_fence"
                ),
            })
    }
    fn set_cave_air(&mut self, position: [i32; 3]) -> Result<()> {
        self.write(
            position,
            BlockState {
                name: "minecraft:cave_air".into(),
                properties: BTreeMap::new(),
            },
        )
    }
}

impl<F> MineshaftCrossingWorld for MineshaftRegion<'_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<bool>,
{
    fn is_air(&mut self, position: [i32; 3]) -> Result<bool> {
        Ok(super::is_air(&self.block(position)?))
    }
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self.region, x, z)
    }
    fn upper_face_sturdy(&mut self, position: [i32; 3]) -> Result<bool> {
        let state = self.block(position)?;
        if known_piece_block(&state) {
            return Ok(matches!(
                state.name.as_str(),
                "minecraft:oak_planks"
                    | "minecraft:dark_oak_planks"
                    | "minecraft:oak_log"
                    | "minecraft:dark_oak_log"
            ));
        }
        generation_face_sturdy(&state, [0, 1, 0])
    }
    fn set_piece_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<()> {
        self.set_floor_planks(position, variant)
    }
    fn set_floor_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<()> {
        let name = match variant {
            MineshaftVariant::Normal => "minecraft:oak_planks",
            MineshaftVariant::Mesa => "minecraft:dark_oak_planks",
        };
        self.write(
            position,
            BlockState {
                name: name.into(),
                properties: BTreeMap::new(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct BiomeData(bool);
    struct MotionData;
    impl ResourceProvider for MotionData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(matches!(
                path,
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
            )
            .then(|| br#"{"values":["minecraft:stone","minecraft:oak_planks"]}"#.to_vec()))
        }
    }
    impl ResourceProvider for BiomeData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(
                (path == "data/minecraft/tags/worldgen/biome/mineshaft_blocking.json").then(|| {
                    if self.0 {
                        br#"{"values":["minecraft:plains"]}"#.to_vec()
                    } else {
                        br#"{"values":[]}"#.to_vec()
                    }
                }),
            )
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            Ok(vec![ResourceId::parse("minecraft:plains")?])
        }
    }

    fn plains() -> super::super::super::BiomeNeighborhood {
        let mut neighborhood = super::super::super::BiomeNeighborhood::new(0, 0, 16).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                let values = vec![ResourceId::parse("minecraft:plains").unwrap(); 64];
                neighborhood
                    .insert(Arc::new(
                        super::super::super::QuartBiomeChunk::from_values([x, z], 0, 0, 16, values)
                            .unwrap(),
                    ))
                    .unwrap();
            }
        }
        neighborhood
    }

    fn room() -> MineshaftGraphPiece {
        MineshaftGraphPiece {
            bounds: BoundingBox::new([2, 1, 2], [10, 6, 10]).unwrap(),
            direction: None,
            depth: 0,
            kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
        }
    }

    #[test]
    fn room_commits_to_draft_and_preserves_previous_snapshot() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = Arc::clone(&terrain.world);
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut biome = |_: [i32; 3], _: &World| Ok(false);
        assert_eq!(
            region
                .place_mineshaft_static_piece(&room(), MineshaftVariant::Normal, clip, &mut biome)
                .unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        region.finish().unwrap();
        assert_eq!(before.block(3, 2, 3).unwrap().name, "minecraft:stone");
        assert_eq!(
            terrain.world.block(3, 2, 3).unwrap().name,
            "minecraft:cave_air"
        );
    }

    #[test]
    fn unsupported_interior_block_poisoning_keeps_terrain_unmodified() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(BlockState {
            name: "minecraft:beacon".into(),
            properties: BTreeMap::new(),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap()[(5 + 5 * 16 + 3 * 256) as usize] = id;
        let before = Arc::clone(&terrain.world);
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut biome = |_: [i32; 3], _: &World| Ok(false);
        assert!(
            region
                .place_mineshaft_static_piece(&room(), MineshaftVariant::Normal, clip, &mut biome)
                .is_err()
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&terrain.world, &before));
        assert_eq!(
            terrain.world.block(3, 2, 3).unwrap().name,
            "minecraft:stone"
        );
    }

    #[test]
    fn active_biome_tag_controls_real_region_placement() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let biomes = plains();
        let blocking =
            native_resources::mineshaft::MineshaftBlockingBiomes::load(&mut BiomeData(true))
                .unwrap();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let writable = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let mut region = terrain
            .decoration_region(&tags, &motion, writable.clone())
            .unwrap();
        assert_eq!(
            region
                .place_mineshaft_static_piece_with_biomes(
                    &room(),
                    MineshaftVariant::Normal,
                    clip,
                    &biomes,
                    &blocking
                )
                .unwrap(),
            MineshaftPlacementOutcome::Blocked
        );
        region.finish().unwrap();
        assert_eq!(
            terrain.world.block(3, 2, 3).unwrap().name,
            "minecraft:stone"
        );
        let allowed =
            native_resources::mineshaft::MineshaftBlockingBiomes::load(&mut BiomeData(false))
                .unwrap();
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        assert_eq!(
            region
                .place_mineshaft_static_piece_with_biomes(
                    &room(),
                    MineshaftVariant::Normal,
                    clip,
                    &biomes,
                    &allowed
                )
                .unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        region.finish().unwrap();
        assert_eq!(
            terrain.world.block(3, 2, 3).unwrap().name,
            "minecraft:cave_air"
        );
    }

    #[test]
    fn stairs_carve_real_terrain_and_update_heightmaps() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let piece = MineshaftGraphPiece {
            bounds: BoundingBox::new([2, 0, 2], [4, 7, 10]).unwrap(),
            direction: Some(native_resources::structure_start::HorizontalDirection::South),
            depth: 1,
            kind: MineshaftGraphPieceKind::Stairs,
        };
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert_eq!(region.final_height(0, 2, 2).unwrap(), 8);
        let mut biome = |_: [i32; 3], _: &World| Ok(false);
        assert_eq!(
            region
                .place_mineshaft_static_piece(&piece, MineshaftVariant::Normal, clip, &mut biome)
                .unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        assert_eq!(region.final_height(0, 2, 2).unwrap(), 5);
        region.finish().unwrap();
        assert_eq!(
            terrain.world.block(2, 5, 2).unwrap().name,
            "minecraft:cave_air"
        );
        assert_eq!(
            terrain.world.block(2, 4, 2).unwrap().name,
            "minecraft:stone"
        );
    }

    #[test]
    fn crossing_updates_real_blocks_and_heightmaps() {
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let registry = ["air", "cave_air", "void_air", "stone", "oak_planks"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let motion = HeightmapTags::load(&mut MotionData, &registry).unwrap();
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let piece = MineshaftGraphPiece {
            bounds: BoundingBox::new([3, 2, 3], [7, 4, 7]).unwrap(),
            direction: Some(native_resources::structure_start::HorizontalDirection::West),
            depth: 1,
            kind: MineshaftGraphPieceKind::Crossing { two_floors: false },
        };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let mut biome = |_: [i32; 3], _: &World| Ok(false);
        assert_eq!(
            region
                .place_mineshaft_static_piece(&piece, MineshaftVariant::Normal, clip, &mut biome)
                .unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        region.finish().unwrap();
        assert_eq!(
            terrain.world.block(5, 2, 5).unwrap().name,
            "minecraft:cave_air"
        );
        assert_eq!(
            terrain.world.block(4, 2, 4).unwrap().name,
            "minecraft:oak_planks"
        );
    }
}
