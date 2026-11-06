//! Apply supported mineshaft pieces to an unpublished generation region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_resources::mineshaft::{
    CorridorBlock, MineshaftCorridorWorld, MineshaftCrossingWorld, MineshaftGraphPiece,
    MineshaftGraphPieceKind, MineshaftPlacementOutcome, MineshaftPlacementWorld,
    MineshaftSupportBlock, MineshaftSupportOutcome, MineshaftSupportWorld, MineshaftVariant,
    place_corridor_beam, place_corridor_cobweb, place_crossing, place_room_or_stairs,
    place_vertical_support,
};
use native_resources::{structure_piece::StructureFrame, structure_start::HorizontalDirection};

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
    /// Apply the corridor's direct vertical support writes to the draft.
    /// The region's block tags must include `minecraft:unstable_bottom_center`.
    pub fn place_mineshaft_vertical_support(
        &mut self,
        frame: StructureFrame,
        local: [i32; 3],
        variant: MineshaftVariant,
        clip: BoundingBox,
    ) -> Result<MineshaftSupportOutcome> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let mut unused_biome = |_: [i32; 3], _: &World| -> Result<bool> { Ok(false) };
        let mut region = MineshaftRegion {
            region: self,
            blocking_biome: &mut unused_biome,
        };
        let result = place_vertical_support(frame, local, variant, clip, &mut region)?;
        self.failed = false;
        Ok(result)
    }

    /// Apply one corridor beam, committing the random stream only on success.
    pub fn place_mineshaft_corridor_beam(
        &mut self,
        frame: StructureFrame,
        z: i32,
        variant: MineshaftVariant,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.run_mineshaft_corridor_part(random, |region, next| {
            place_corridor_beam(frame, z, variant, region, clip, next)
        })
    }

    /// Apply one corridor cobweb attempt, committing RNG only on success.
    pub fn place_mineshaft_corridor_cobweb(
        &mut self,
        frame: StructureFrame,
        local: [i32; 3],
        probability: f32,
        variant: MineshaftVariant,
        clip: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.run_mineshaft_corridor_part(random, |region, next| {
            place_corridor_cobweb(frame, local, probability, variant, region, clip, next)
        })
    }

    fn run_mineshaft_corridor_part(
        &mut self,
        random: &mut DecorationRandom,
        operation: impl FnOnce(
            &mut MineshaftRegion<'_, '_, fn([i32; 3], &World) -> Result<bool>>,
            &mut DecorationRandom,
        ) -> Result<bool>,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut next = random.clone();
        self.failed = true;
        let mut unused_biome: fn([i32; 3], &World) -> Result<bool> = |_, _| Ok(false);
        let mut region = MineshaftRegion {
            region: self,
            blocking_biome: &mut unused_biome,
        };
        let result = operation(&mut region, &mut next)?;
        *random = next;
        self.failed = false;
        Ok(result)
    }

    /// Install and resolve the cave-spider block entity in this draft.
    /// The enclosing piece controls clipping and its placed-spider flag.
    pub fn place_mineshaft_spawner(&mut self, position: [i32; 3]) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        let mut unused_biome = |_: [i32; 3], _: &World| -> Result<bool> { Ok(false) };
        let mut region = MineshaftRegion {
            region: self,
            blocking_biome: &mut unused_biome,
        };
        region.write(
            position,
            BlockState {
                name: "minecraft:spawner".into(),
                properties: BTreeMap::new(),
            },
        )?;
        Arc::make_mut(&mut region.region.monster_entities).insert(
            position,
            Some(super::super::GeneratedMonsterRoomEntity::CaveSpiderSpawner(
                super::super::GeneratedCaveSpiderSpawner { position },
            )),
        );
        Arc::make_mut(
            region
                .region
                .entity_order
                .entry(chunk(position))
                .or_default(),
        )
        .insert(position);
        self.failed = false;
        Ok(())
    }

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

impl<F> MineshaftSupportWorld for MineshaftRegion<'_, '_, F> {
    type State = BlockState;

    fn vertical_bounds(&self) -> (i32, i32) {
        (
            self.region.target.min_y,
            self.region.target.min_y + self.region.target.height - 1,
        )
    }

    fn block_state(&mut self, position: [i32; 3]) -> Result<BlockState> {
        self.block(position)
    }

    fn replaceable_by_structure(&mut self, state: &BlockState) -> Result<bool> {
        if is_air(state) || valid_lichen(state) {
            return Ok(true);
        }
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            native_world::fluid::FluidState::from_block(state)
                .map_err(anyhow::Error::msg)?
                .context("invalid mineshaft support fluid")?;
            return Ok(true);
        }
        if matches!(
            state.name.as_str(),
            "minecraft:seagrass" | "minecraft:tall_seagrass"
        ) {
            simple_plant_kind(state)?;
            return Ok(true);
        }
        ensure!(
            known_piece_block(state) || monster_block_flags(state).is_ok(),
            "unsupported mineshaft support block: {}",
            state.name
        );
        Ok(false)
    }

    fn is_lava(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name == "minecraft:lava")
    }

    fn supports_column(&mut self, _: [i32; 3], state: &BlockState) -> Result<bool> {
        if known_piece_block(state) {
            return Ok(matches!(
                state.name.as_str(),
                "minecraft:oak_planks"
                    | "minecraft:dark_oak_planks"
                    | "minecraft:oak_log"
                    | "minecraft:dark_oak_log"
            ));
        }
        generation_face_sturdy(state, [0, 1, 0])
    }

    fn supports_hanging_chain(&mut self, _: [i32; 3], state: &BlockState) -> Result<bool> {
        let unstable = ResourceId::parse("minecraft:unstable_bottom_center")?;
        if self.region.tags.contains(&unstable, &state.name)? {
            return Ok(false);
        }
        if matches!(
            state.name.as_str(),
            "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
        ) {
            return Ok(false);
        }
        if known_piece_block(state) {
            return match state.name.as_str() {
                "minecraft:oak_planks"
                | "minecraft:dark_oak_planks"
                | "minecraft:oak_log"
                | "minecraft:dark_oak_log" => Ok(true),
                "minecraft:iron_chain"
                    if state.properties.get("axis").map(String::as_str) == Some("y") =>
                {
                    Ok(true)
                }
                "minecraft:cobweb" | "minecraft:wall_torch" | "minecraft:rail" => Ok(false),
                _ => anyhow::bail!("unsupported mineshaft center support state: {state:?}"),
            };
        }
        monster_block_flags(state)?;
        vanilla_full_occlusion(&state.name)
            .with_context(|| format!("unsupported mineshaft center support {}", state.name))
    }

    fn set_support(
        &mut self,
        position: [i32; 3],
        block: MineshaftSupportBlock,
        variant: MineshaftVariant,
    ) -> Result<()> {
        let wood = match variant {
            MineshaftVariant::Normal => "oak",
            MineshaftVariant::Mesa => "dark_oak",
        };
        let (name, properties) = match block {
            MineshaftSupportBlock::Wood => (
                format!("minecraft:{wood}_log"),
                BTreeMap::from([("axis".into(), "y".into())]),
            ),
            MineshaftSupportBlock::Fence => (
                format!("minecraft:{wood}_fence"),
                ["north", "south", "east", "west", "waterlogged"]
                    .into_iter()
                    .map(|key| (key.into(), "false".into()))
                    .collect(),
            ),
            MineshaftSupportBlock::IronChain => (
                "minecraft:iron_chain".into(),
                BTreeMap::from([
                    ("axis".into(), "y".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            ),
        };
        // Vertical supports use direct WorldGenLevel writes, not StructurePiece writes.
        self.write(position, BlockState { name, properties })
    }
}

impl<F> MineshaftCorridorWorld for MineshaftRegion<'_, '_, F>
where
    F: FnMut([i32; 3], &World) -> Result<bool>,
{
    fn is_air(&mut self, position: [i32; 3]) -> Result<bool> {
        Ok(super::is_air(&self.block(position)?))
    }

    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self.region, x, z)
    }

    fn face_sturdy(&mut self, position: [i32; 3], face: [i32; 3]) -> Result<bool> {
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
        generation_face_sturdy(&state, face)
    }

    fn set_corridor_block(
        &mut self,
        position: [i32; 3],
        block: CorridorBlock,
        variant: MineshaftVariant,
    ) -> Result<()> {
        let wood = match variant {
            MineshaftVariant::Normal => "oak",
            MineshaftVariant::Mesa => "dark_oak",
        };
        let facing = |direction| match direction {
            HorizontalDirection::North => "north",
            HorizontalDirection::South => "south",
            HorizontalDirection::West => "west",
            HorizontalDirection::East => "east",
        };
        let (name, properties, mark) = match block {
            CorridorBlock::Planks => (format!("minecraft:{wood}_planks"), BTreeMap::new(), false),
            CorridorBlock::Fence { connected } => {
                let mut properties: BTreeMap<String, String> =
                    ["north", "south", "east", "west", "waterlogged"]
                        .into_iter()
                        .map(|name| (name.into(), "false".into()))
                        .collect();
                properties.insert(facing(connected).into(), "true".into());
                (format!("minecraft:{wood}_fence"), properties, true)
            }
            CorridorBlock::WallTorch { facing: direction } => (
                "minecraft:wall_torch".into(),
                BTreeMap::from([("facing".into(), facing(direction).into())]),
                true,
            ),
            CorridorBlock::Cobweb => ("minecraft:cobweb".into(), BTreeMap::new(), false),
            CorridorBlock::Rail { north_south } => (
                "minecraft:rail".into(),
                BTreeMap::from([
                    (
                        "shape".into(),
                        if north_south {
                            "north_south"
                        } else {
                            "east_west"
                        }
                        .into(),
                    ),
                    ("waterlogged".into(), "false".into()),
                ]),
                false,
            ),
        };
        self.write(position, BlockState { name, properties })?;
        if mark {
            DiskWorld::mark_postprocessing(self.region, position)?;
        }
        Ok(())
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
            !matches!(old.name.as_str(), "minecraft:chest" | "minecraft:spawner")
                && !self.region.monster_entities.contains_key(&position)
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
    struct SupportData(bool);
    impl ResourceProvider for SupportData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/block/unstable_bottom_center.json" => Some(if self.0 {
                    br#"{"values":["minecraft:stone"]}"#.to_vec()
                } else {
                    br#"{"values":[]}"#.to_vec()
                }),
                "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => Some(
                    br#"{"values":["minecraft:stone","minecraft:oak_log","minecraft:oak_fence"]}"#
                        .to_vec(),
                ),
                _ => None,
            })
        }
    }

    fn support_tags(unstable_stone: bool) -> (OreBlockTags, HeightmapTags) {
        let registry = [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "oak_log",
            "oak_fence",
            "iron_chain",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let required =
            BTreeSet::from([ResourceId::parse("minecraft:unstable_bottom_center").unwrap()]);
        let tags =
            OreBlockTags::load(&mut SupportData(unstable_stone), &registry, &required).unwrap();
        let motion = HeightmapTags::load(&mut SupportData(unstable_stone), &registry).unwrap();
        (tags, motion)
    }

    fn replace_column(
        terrain: &mut super::super::super::CarvedTerrainWorld,
        from: i32,
        to: i32,
        name: &str,
    ) {
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        for y in from..=to {
            section.blocks.as_mut().unwrap()[(4 + 4 * 16 + y * 256) as usize] = id;
        }
    }

    fn support_frame() -> (StructureFrame, BoundingBox) {
        (
            StructureFrame {
                bounds: BoundingBox::new([4, 8, 4], [6, 10, 8]).unwrap(),
                orientation: Some(HorizontalDirection::South),
            },
            BoundingBox::new([4, 7, 4], [4, 7, 4]).unwrap(),
        )
    }
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
            .then(|| {
                br#"{"values":["minecraft:stone","minecraft:oak_planks","minecraft:oak_fence"]}"#
                    .to_vec()
            }))
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

    fn corridor_motion() -> HeightmapTags {
        let registry = [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "oak_planks",
            "oak_fence",
            "wall_torch",
            "cobweb",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        HeightmapTags::load(&mut MotionData, &registry).unwrap()
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
    fn vertical_support_writes_logs_beyond_clip_to_real_terrain() {
        let (mut terrain, _, _, _) = super::super::tests::fixture();
        replace_column(&mut terrain, 4, 6, "air");
        let (tags, motion) = support_tags(false);
        let (frame, clip) = support_frame();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert_eq!(
            region
                .place_mineshaft_vertical_support(frame, [0, -1, 0], MineshaftVariant::Normal, clip)
                .unwrap(),
            MineshaftSupportOutcome::Grounded
        );
        region.finish().unwrap();
        for y in 4..=6 {
            let state = terrain.world.block(4, y, 4).unwrap();
            assert_eq!(state.name, "minecraft:oak_log");
            assert_eq!(state.properties["axis"], "y");
        }
        assert!(terrain.feature_postprocessing.is_empty());
    }

    #[test]
    fn vertical_support_hangs_only_from_stable_ceiling() {
        for unstable in [false, true] {
            let (mut terrain, _, _, _) = super::super::tests::fixture();
            replace_column(&mut terrain, 0, 6, "air");
            replace_column(&mut terrain, 11, 11, "stone");
            let (tags, motion) = support_tags(unstable);
            let (frame, clip) = support_frame();
            let mut region = terrain
                .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
                .unwrap();
            assert_eq!(
                region
                    .place_mineshaft_vertical_support(
                        frame,
                        [0, -1, 0],
                        MineshaftVariant::Normal,
                        clip
                    )
                    .unwrap(),
                if unstable {
                    MineshaftSupportOutcome::NoAnchor
                } else {
                    MineshaftSupportOutcome::Hanging
                }
            );
            region.finish().unwrap();
            if unstable {
                assert_eq!(terrain.world.block(4, 8, 4).unwrap().name, "minecraft:air");
            } else {
                assert_eq!(
                    terrain.world.block(4, 8, 4).unwrap().name,
                    "minecraft:oak_fence"
                );
                for y in 9..=10 {
                    let state = terrain.world.block(4, y, 4).unwrap();
                    assert_eq!(state.name, "minecraft:iron_chain");
                    assert_eq!(state.properties["axis"], "y");
                    assert_eq!(state.properties["waterlogged"], "false");
                }
                assert!(terrain.feature_postprocessing.is_empty());
            }
        }
    }

    #[test]
    fn corridor_beam_writes_rotated_fences_and_marks_shapes() {
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let motion = corridor_motion();
        let chunk = ChunkPos { x: 0, z: 0 };
        let frame = StructureFrame {
            bounds: BoundingBox::new([2, 2, 2], [4, 4, 6]).unwrap(),
            orientation: Some(HorizontalDirection::South),
        };
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([chunk]))
            .unwrap();
        assert!(
            region
                .place_mineshaft_corridor_beam(
                    frame,
                    2,
                    MineshaftVariant::Normal,
                    clip,
                    &mut DecorationRandom::new(1)
                )
                .unwrap()
        );
        region.finish().unwrap();
        for (x, direction) in [(2, "west"), (4, "east")] {
            for y in [2, 3] {
                let fence = terrain.world.block(x, y, 4).unwrap();
                assert_eq!(fence.name, "minecraft:oak_fence");
                assert_eq!(fence.properties[direction], "true");
                assert_eq!(fence.properties["waterlogged"], "false");
            }
        }
        let marks = &terrain.feature_postprocessing[&chunk];
        assert_eq!(&marks.sections()[0][..4], &[0x422, 0x432, 0x424, 0x434]);
    }

    #[test]
    fn corridor_beam_failure_discards_rng_and_world() {
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let motion = corridor_motion();
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
        section.blocks.as_mut().unwrap()[3 + 4 * 16 + 4 * 256] = id;
        let before = Arc::clone(&terrain.world);
        let frame = StructureFrame {
            bounds: BoundingBox::new([2, 2, 2], [4, 4, 6]).unwrap(),
            orientation: Some(HorizontalDirection::South),
        };
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let seed = (0..100)
            .find(|seed| DecorationRandom::new(*seed).next_int_bound(4) != Some(0))
            .unwrap();
        let mut random = DecorationRandom::new(seed);
        let mut expected = random.clone();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        let error = region
            .place_mineshaft_corridor_beam(frame, 2, MineshaftVariant::Normal, clip, &mut random)
            .unwrap_err();
        assert!(format!("{error:#}").contains("minecraft:beacon"));
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&terrain.world, &before));
    }

    #[test]
    fn corridor_cobweb_uses_real_neighbor_faces_and_height() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let frame = StructureFrame {
            bounds: BoundingBox::new([2, 2, 2], [4, 4, 6]).unwrap(),
            orientation: Some(HorizontalDirection::South),
        };
        let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert!(
            region
                .place_mineshaft_corridor_cobweb(
                    frame,
                    [2, 1, 2],
                    1.0,
                    MineshaftVariant::Normal,
                    clip,
                    &mut DecorationRandom::new(1)
                )
                .unwrap()
        );
        region.finish().unwrap();
        assert_eq!(
            terrain.world.block(4, 3, 4).unwrap().name,
            "minecraft:cobweb"
        );
        assert!(terrain.feature_postprocessing.is_empty());
    }

    #[test]
    fn cave_spawner_is_saved_with_its_block_and_live_order() {
        use native_persistence::nbt::{Tag, TagType};

        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = Arc::clone(&terrain.world);
        let position = [4, 3, 4];
        let chunk = ChunkPos { x: 0, z: 0 };
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([chunk]))
            .unwrap();
        region.place_mineshaft_spawner(position).unwrap();
        region.finish().unwrap();
        assert_eq!(before.block(4, 3, 4).unwrap().name, "minecraft:stone");
        assert_eq!(
            terrain.world.block(4, 3, 4).unwrap().name,
            "minecraft:spawner"
        );
        assert_eq!(
            terrain
                .generated_block_entity_data_with(chunk, |state| {
                    Some(state.name == "minecraft:spawner")
                })
                .unwrap(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![
                    super::super::super::GeneratedCaveSpiderSpawner { position }.block_entity()
                ],
            }
        );
    }

    #[test]
    fn spawner_conflict_poisoning_discards_the_entire_draft() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let before = Arc::clone(&terrain.world);
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        region.place_mineshaft_spawner([4, 3, 4]).unwrap();
        assert!(region.place_mineshaft_spawner([4, 3, 4]).is_err());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&terrain.world, &before));
        assert_eq!(terrain.generated_monster_entities().count(), 0);
    }

    #[test]
    fn untracked_existing_spawner_is_not_overwritten() {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let id = world.intern_state(BlockState {
            name: "minecraft:spawner".into(),
            properties: BTreeMap::new(),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap()[4 + 4 * 16 + 3 * 256] = id;
        let before = Arc::clone(&terrain.world);
        let mut region = terrain
            .decoration_region(&tags, &motion, BTreeSet::from([ChunkPos { x: 0, z: 0 }]))
            .unwrap();
        assert!(region.place_mineshaft_spawner([4, 3, 4]).is_err());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&terrain.world, &before));
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
        let registry = [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "oak_planks",
            "oak_fence",
        ]
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
