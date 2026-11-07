//! Apply ruined-portal terrain decoration to an unpublished generation region.
use super::super::jigsaw::{JigsawPlacementWorld, StructurePlacementWorld};
use super::*;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    ruined_portal_decoration::{PortalBlock, PortalDecorationWorld},
    structure_start::Heightmap,
    tree_leaf_update::ShapeEdgeWorld,
};
use native_world::fluid::FluidState;

// These stateless blocks have no updateShape effects; magma marks above itself.
fn portal_full(state: &BlockState) -> bool {
    state.properties.is_empty()
        && matches!(
            state.name.as_str(),
            "minecraft:stone"
                | "minecraft:stone_bricks"
                | "minecraft:mossy_stone_bricks"
                | "minecraft:cracked_stone_bricks"
                | "minecraft:chiseled_stone_bricks"
                | "minecraft:cobblestone"
                | "minecraft:mossy_cobblestone"
                | "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:gold_block"
                | "minecraft:netherrack"
                | "minecraft:magma_block"
                | "minecraft:blackstone"
                | "minecraft:polished_blackstone"
                | "minecraft:polished_blackstone_bricks"
                | "minecraft:cracked_polished_blackstone_bricks"
                | "minecraft:chiseled_polished_blackstone"
        )
}

fn portal_template_full(state: &BlockState) -> Result<bool> {
    if portal_full(state) {
        return Ok(true);
    }
    if is_air(state)
        || (state.name == "minecraft:iron_bars"
            && native_world::collision::pane_waterlogged(state).is_some())
    {
        return Ok(false);
    }
    bail!("unsupported portal template static shape: {state:?}")
}

struct PortalTemplateRegion<'r, 'a, 'g> {
    inner: super::jigsaw::JigsawRegion<'r, 'a, 'g>,
}

impl JigsawPlacementWorld for PortalTemplateRegion<'_, '_, '_> {
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(&mut self.inner, pos)
    }
    fn fluid_at(&mut self, pos: [i32; 3]) -> Result<Option<FluidState>> {
        self.inner.fluid_at(pos)
    }
    fn fluid_of_state(&mut self, state: &BlockState) -> Result<Option<FluidState>> {
        self.inner.fluid_of_state(state)
    }
    fn place_template_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&native_persistence::nbt::Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        self.inner.place_template_block(pos, state, nbt, random)
    }
    fn is_liquid_container(&mut self, state: &BlockState) -> Result<bool> {
        self.inner.is_liquid_container(state)
    }
    fn place_liquid(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        fluid: Option<FluidState>,
    ) -> Result<()> {
        self.inner.place_liquid(pos, state, fluid)
    }
    fn mark_block_entity_changed(&mut self, pos: [i32; 3]) -> Result<()> {
        self.inner.mark_block_entity_changed(pos)
    }
    fn place_template_entities(
        &mut self,
        _: &[native_persistence::nbt::Tag],
        _: [i32; 3],
        _: native_resources::structure_transform::Rotation,
        _: native_core::beardifier::BoundingBox,
    ) -> Result<()> {
        bail!("portal template entities are unsupported")
    }
}

impl ShapeEdgeWorld for PortalTemplateRegion<'_, '_, '_> {
    type State = BlockState;
    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        JigsawPlacementWorld::block(self, pos)
    }
    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        direction: [i32; 3],
        _: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&state.name, &state.properties),
            "invalid portal edge state"
        );
        if state.name == "minecraft:iron_bars"
            && native_world::collision::pane_waterlogged(state).is_some()
        {
            if state.properties.get("waterlogged").map(String::as_str) == Some("true") {
                SpringWorld::schedule_spring_tick(
                    self.inner.region,
                    pos,
                    &ResourceId::parse("minecraft:water")?,
                )?;
            }
            let face = match direction {
                [0, 0, -1] => Some("north"),
                [1, 0, 0] => Some("east"),
                [0, 0, 1] => Some("south"),
                [-1, 0, 0] => Some("west"),
                [0, -1, 0] | [0, 1, 0] => None,
                _ => bail!("invalid portal edge direction"),
            };
            if let Some(face) = face {
                ensure!(
                    self.inner
                        .registry
                        .is_canonical_state(&neighbor.name, &neighbor.properties),
                    "invalid portal neighbor state"
                );
                let connected = if portal_full(neighbor)
                    || (neighbor.name == "minecraft:iron_bars"
                        && native_world::collision::pane_waterlogged(neighbor).is_some())
                {
                    true
                } else if is_air(neighbor) {
                    false
                } else {
                    bail!("unsupported portal bars neighbor: {neighbor:?}")
                };
                let mut next = state.clone();
                next.properties.insert(face.into(), connected.to_string());
                return Ok(next);
            }
            return Ok(state.clone());
        }
        ensure!(
            portal_full(state) || is_air(state),
            "unsupported portal edge update: {state:?}"
        );
        Ok(state.clone())
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        ensure!(matches!(flags, 2 | 18), "unsupported portal edge flags");
        ensure!(
            flags != 18 || state.name != "minecraft:magma_block",
            "unsupported known-shape magma update"
        );
        let mut unused = DecorationRandom::new(0);
        self.place_structure_block(pos, state, None, &mut unused)?;
        Ok(())
    }
}

impl StructurePlacementWorld for PortalTemplateRegion<'_, '_, '_> {
    fn place_structure_block(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        nbt: Option<&native_persistence::nbt::Tag>,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(nbt.is_none(), "portal block entities are not yet supported");
        if state.name == "minecraft:iron_bars" {
            return self.inner.place_template_block(pos, state, None, random);
        }
        ensure!(
            portal_full(state) || is_air(state),
            "unsupported portal template block: {state:?}"
        );
        if !self.inner.region.writable.contains(&chunk(pos)) {
            return Ok(false);
        }
        ensure!(
            self.inner.region.world.chunks.contains_key(&chunk(pos)),
            "portal template chunk missing"
        );
        if self.inner.region.is_outside(pos[1]) {
            if state.name == "minecraft:magma_block" {
                mark_magma(self.inner.region, pos)?;
            }
            return Ok(true);
        }
        let old = self.inner.region.read(pos)?;
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&old.name, &old.properties)
                && self.inner.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some()
                    || portal_full(&old)
                    || (old.name == "minecraft:iron_bars"
                        && native_world::collision::pane_waterlogged(&old).is_some()))
                && !self.inner.region.monster_entities.contains_key(&pos)
                && !self.inner.region.bee_nests.contains_key(&pos),
            "unsupported portal template source: {old:?}"
        );
        ensure!(
            self.inner
                .registry
                .is_canonical_state(&state.name, &state.properties)
                && self.inner.registry.has_block_entity(&state.name) == Some(false),
            "invalid portal template output"
        );
        self.inner.region.magma_block_write = state.name == "minecraft:magma_block";
        let result = self.inner.region.set_proto_block(pos, state);
        self.inner.region.magma_block_write = false;
        result?;
        if state.name == "minecraft:magma_block" {
            mark_magma(self.inner.region, pos)?;
        }
        Ok(true)
    }
    fn update_from_neighbor_shapes(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
    ) -> Result<BlockState> {
        let mut result = state.clone();
        for direction in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let neighbor = std::array::from_fn(|i| pos[i].wrapping_add(direction[i]));
            let other = ShapeEdgeWorld::block(self, neighbor)?;
            result = self.update_shape(pos, &result, direction, neighbor, &other)?;
        }
        Ok(result)
    }
    fn update_neighbors_at(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        Ok(())
    }
}

struct PortalRegion<'r, 'a, 'g> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'g GeneratedBlockRegistry,
    protected: &'g BTreeSet<String>,
}

fn mark_magma(region: &mut DecorationRegion<'_>, pos: [i32; 3]) -> Result<()> {
    DiskWorld::mark_postprocessing(region, [pos[0], pos[1].wrapping_add(1), pos[2]])
}

impl PortalDecorationWorld for PortalRegion<'_, '_, '_> {
    type State = BlockState;

    fn block(&mut self, position: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, position)
    }
    fn id<'a>(&self, state: &'a BlockState) -> &'a str {
        &state.name
    }
    fn is_air(&self, state: &BlockState) -> bool {
        super::is_air(state)
    }
    fn is_protected(&self, state: &BlockState) -> Result<bool> {
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal block state: {state:?}"
        );
        Ok(self.protected.contains(&state.name))
    }
    fn height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
        match map {
            Heightmap::WorldSurface => self.region.world_surface_height(x, z),
            Heightmap::OceanFloor => OreVeinWorld::ocean_floor_height(self.region, x, z),
        }
    }
    fn collision_face_full(&mut self, pos: [i32; 3], direction: [i32; 3]) -> Result<bool> {
        let state = self.block(pos)?;
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal collision state"
        );
        let shape = native_world::collision::block_shape(&state)
            .with_context(|| format!("unsupported portal collision shape for {}", state.name))?;
        Ok(native_world::collision::covers_full_face(
            &shape,
            glam::IVec3::from_array(direction),
        ))
    }
    fn set_block(&mut self, pos: [i32; 3], block: PortalBlock) -> Result<()> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "portal block chunk missing"
        );
        if self.region.is_outside(pos[1]) {
            if block == PortalBlock::Magma {
                mark_magma(self.region, pos)?;
            }
            return Ok(());
        }
        let old = self.region.read(pos)?;
        let portal_state = matches!(
            old.name.as_str(),
            "minecraft:gold_block"
                | "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:stone_bricks"
                | "minecraft:cracked_stone_bricks"
                | "minecraft:mossy_stone_bricks"
                | "minecraft:stone_brick_stairs"
                | "minecraft:mossy_stone_brick_stairs"
                | "minecraft:stone_slab"
                | "minecraft:stone_brick_slab"
                | "minecraft:mossy_stone_brick_slab"
                | "minecraft:stone_brick_wall"
                | "minecraft:mossy_stone_brick_wall"
                | "minecraft:iron_bars"
                | "minecraft:vine"
                | "minecraft:jungle_leaves"
        );
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some() || portal_state)
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported portal replacement source at {pos:?}: {old:?}"
        );
        let (name, properties) = match block {
            PortalBlock::Netherrack => ("minecraft:netherrack", BTreeMap::new()),
            PortalBlock::Magma => ("minecraft:magma_block", BTreeMap::new()),
            PortalBlock::PersistentJungleLeaves => (
                "minecraft:jungle_leaves",
                BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "true".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            ),
            PortalBlock::Vine(direction) => {
                let face = match direction {
                    [0, 0, -1] => "north",
                    [1, 0, 0] => "east",
                    [0, 0, 1] => "south",
                    [-1, 0, 0] => "west",
                    _ => bail!("invalid portal vine face"),
                };
                let mut props: BTreeMap<String, String> = ["up", "north", "south", "east", "west"]
                    .into_iter()
                    .map(|name| (name.into(), "false".into()))
                    .collect();
                props.insert(face.into(), "true".into());
                ("minecraft:vine", props)
            }
        };
        let state = BlockState {
            name: name.into(),
            properties,
        };
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(false),
            "unverified portal output state"
        );
        self.region.magma_block_write = block == PortalBlock::Magma;
        let result = self.region.set_proto_block(pos, &state);
        self.region.magma_block_write = false;
        result?;
        if block == PortalBlock::Magma {
            mark_magma(self.region, pos)?;
        }
        Ok(())
    }
}

impl DecorationRegion<'_> {
    /// Place and decorate a portal whose center lies in this decoration clip.
    /// Failures invalidate the entire unpublished region and preserve inputs.
    pub fn place_ruined_portal(
        &mut self,
        start: &super::super::ruined_portal::PreparedStart,
        registry: &GeneratedBlockRegistry,
        processors: &native_persistence::structure_processors::PreparedProcessors,
        protected: &BTreeSet<String>,
        clip: &mut native_core::beardifier::BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        ensure!(!self.failed, "generation attempt previously failed");
        ensure!(
            (0..3).all(|i| clip.min[i] <= clip.max[i]),
            "invalid portal clip"
        );
        let bounds = start.piece.bounds;
        let mut center = [0; 3];
        for axis in 0..3 {
            let span = bounds.max[axis]
                .checked_sub(bounds.min[axis])
                .and_then(|v| v.checked_add(1))
                .context("portal bounds overflow")?;
            ensure!(span > 0, "invalid portal bounds");
            center[axis] = bounds.min[axis]
                .checked_add(span / 2)
                .context("portal center overflow")?;
        }
        if !(0..3).all(|i| (clip.min[i]..=clip.max[i]).contains(&center[i])) {
            return Ok(false);
        }
        let mut next_clip = *clip;
        for axis in 0..3 {
            next_clip.min[axis] = next_clip.min[axis].min(bounds.min[axis]);
            next_clip.max[axis] = next_clip.max[axis].max(bounds.max[axis]);
        }
        let mut next_random = random.clone();
        self.failed = true;
        start.place_template(
            &mut PortalTemplateRegion {
                inner: super::jigsaw::JigsawRegion {
                    region: self,
                    registry,
                },
            },
            registry,
            processors,
            next_clip,
            portal_template_full,
            &mut next_random,
        )?;
        start.piece.decorate(
            &mut PortalRegion {
                region: self,
                registry,
                protected,
            },
            &mut next_random,
        )?;
        *clip = next_clip;
        *random = next_random;
        self.failed = false;
        Ok(true)
    }

    /// Run the piece's post-template decoration on this discardable region.
    /// `protected` must be resolved from the same active block registry and packs.
    pub fn decorate_ruined_portal(
        &mut self,
        piece: &super::super::ruined_portal::SavedRuinedPortalPiece,
        registry: &GeneratedBlockRegistry,
        protected: &BTreeSet<String>,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        piece.decorate(
            &mut PortalRegion {
                region: self,
                registry,
                protected,
            },
            random,
        )?;
        self.failed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{
        heightmap_tags::{HeightmapTags, resolve_block_tag},
        packs::ResourceStack,
        ruined_portal::{GenerationPoint, InitialSelection, Setup, VerticalPlacement},
        structure_transform::{Mirror, Rotation, bounding_box},
    };

    fn piece() -> Result<super::super::super::ruined_portal::SavedRuinedPortalPiece> {
        let size = [5, 4, 5];
        let pos = [6, 7, 6];
        let pivot = [2, 0, 2];
        let selection = InitialSelection {
            setup_index: 0,
            air_pocket: false,
            template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
            rotation: Rotation::None,
            mirror: Mirror::None,
        };
        let setup = Setup {
            placement: VerticalPlacement::OnLandSurface,
            air_pocket_probability: 0.0,
            mossiness: 0.0,
            overgrown: false,
            vines: false,
            can_be_cold: false,
            replace_with_blackstone: false,
            weight: 1.0,
        };
        let bounds = bounding_box(size, pos, selection.mirror, selection.rotation, pivot)?;
        let point = GenerationPoint {
            position: pos,
            pivot,
            bounds,
        };
        super::super::super::ruined_portal::SavedRuinedPortalPiece::capture(
            &selection, &setup, &point, size, false,
        )
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn portal_template_and_decoration_share_one_transaction() -> Result<()> {
        use super::super::super::ruined_portal::PreparedStart;
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_persistence::{
            nbt::{NamedTag, Tag},
            structure_template::{StructureTemplate, TemplateBlock},
        };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let protected = resolve_block_tag(
            &mut resources,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            &ids,
        )?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let piece = piece()?;
        let processors = piece.prepare_processors(&mut resources, &ids)?;
        let template = StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [5, 4, 5],
            palettes: vec![vec![
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:obsidian".into()),
                )]),
                Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:iron_bars".into()),
                )]),
            ]],
            blocks: vec![
                TemplateBlock {
                    position: [0, 1, 0],
                    state: 0,
                    nbt: None,
                },
                TemplateBlock {
                    position: [1, 1, 0],
                    state: 1,
                    nbt: None,
                },
            ],
            entities: vec![],
        };
        let point = GenerationPoint {
            position: piece.position,
            pivot: [2, 0, 2],
            bounds: piece.bounds,
        };
        let start = PreparedStart {
            chunk: [0, 0],
            point,
            terrain_adjustment: TerrainAdjustment::None,
            template: Arc::new(template),
            piece,
        };
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut clip = BoundingBox::new([8, 9, 8], [8, 9, 8]).unwrap();
        let mut random = DecorationRandom::new(42);
        assert!(region.place_ruined_portal(
            &start,
            &registry,
            &processors,
            &protected,
            &mut clip,
            &mut random
        )?);
        assert_eq!(clip, start.piece.bounds);
        region.finish()?;
        let world = terrain.snapshot();
        assert!(matches!(
            world.block(6, 8, 6).unwrap().name.as_str(),
            "minecraft:obsidian" | "minecraft:crying_obsidian"
        ));
        let bars = world.block(7, 8, 6).unwrap();
        assert_eq!(bars.name, "minecraft:iron_bars");
        assert_eq!(bars.properties["west"], "true");
        assert!(
            (-6..=22).any(
                |x| (-6..=22).any(|z| world.block(x, 7, z).is_some_and(|s| matches!(
                    s.name.as_str(),
                    "minecraft:netherrack" | "minecraft:magma_block"
                )))
            )
        );

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut outside = BoundingBox::new([0, 0, 0], [1, 1, 1]).unwrap();
        let old_clip = outside;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(!region.place_ruined_portal(
            &start,
            &registry,
            &processors,
            &protected,
            &mut outside,
            &mut random
        )?);
        assert_eq!(outside, old_clip);
        assert_eq!(random.next_long(), expected.next_long());
        region.finish()?;
        assert_eq!(
            terrain.snapshot().block(6, 8, 6).unwrap().name,
            "minecraft:air"
        );
        let mut unsupported = start.clone();
        let template = Arc::make_mut(&mut unsupported.template);
        template.palettes[0].push(Tag::Compound(vec![(
            "id".into(),
            Tag::String("minecraft:chest".into()),
        )]));
        template.blocks.push(TemplateBlock {
            position: [2, 1, 0],
            state: 2,
            nbt: Some(Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:chest".into()),
            )])),
        });
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut clip = BoundingBox::new([8, 9, 8], [8, 9, 8]).unwrap();
        let old_clip = clip;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        assert!(
            region
                .place_ruined_portal(
                    &unsupported,
                    &registry,
                    &processors,
                    &protected,
                    &mut clip,
                    &mut random
                )
                .is_err()
        );
        assert_eq!(clip, old_clip);
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn magma_writes_mark_the_block_above_in_order() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let protected = BTreeSet::new();
        {
            let mut world = PortalRegion {
                region: &mut region,
                registry: &registry,
                protected: &protected,
            };
            PortalDecorationWorld::set_block(&mut world, [4, 7, 4], PortalBlock::Magma)?;
            PortalDecorationWorld::set_block(&mut world, [4, 7, 4], PortalBlock::Magma)?;
        }
        {
            let mut world = PortalTemplateRegion {
                inner: super::super::jigsaw::JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                },
            };
            let state = BlockState {
                name: "minecraft:magma_block".into(),
                properties: BTreeMap::new(),
            };
            world.place_structure_block([5, 7, 4], &state, None, &mut DecorationRandom::new(0))?;
        }
        let marks = &region.feature_marks[&ChunkPos { x: 0, z: 0 }];
        assert_eq!(marks.sections()[0], [0x484, 0x484, 0x485]);
        let (name, mut properties) = registry.resolve(&serde_json::json!("minecraft:iron_bars"))?;
        properties.insert("waterlogged".into(), "true".into());
        let bars = BlockState { name, properties };
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        {
            let mut world = PortalTemplateRegion {
                inner: super::super::jigsaw::JigsawRegion {
                    region: &mut region,
                    registry: &registry,
                },
            };
            for _ in 0..2 {
                assert_eq!(
                    world.update_shape([6, 8, 4], &bars, [0, 1, 0], [6, 9, 4], &air)?,
                    bars
                );
            }
        }
        let ticks = &region.world.saved_fluid_ticks[&ChunkPos { x: 0, z: 0 }];
        assert_eq!(
            ticks
                .iter()
                .filter(|tick| tick.position == [6, 8, 4] && tick.fluid == "minecraft:water")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn portal_decoration_updates_draft_and_rolls_back_unsupported_entity() -> Result<()> {
        use native_world::loaded::BlockStateId;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let protected = resolve_block_tag(
            &mut resources,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            &ids,
        )?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        region.decorate_ruined_portal(&piece()?, &registry, &protected, &mut random)?;
        assert_eq!(region.world_surface_height(8, 8)?, 8);
        region.finish()?;
        assert!((-6..=22).any(|x| (-6..=22).any(|z| {
            terrain.snapshot().block(x, 7, z).is_some_and(|state| {
                matches!(
                    state.name.as_str(),
                    "minecraft:netherrack" | "minecraft:magma_block"
                )
            })
        })));

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let chest = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        let id: BlockStateId = world.intern_state(chest);
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 8 * 16 + 7 * 256] = id;
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        // A data pack may remove chests from the protected tag. The world
        // adapter must then reject an unsupported block-entity replacement.
        let mut unprotected = protected.clone();
        assert!(unprotected.remove("minecraft:chest"));
        assert!(
            region
                .decorate_ruined_portal(&piece()?, &registry, &unprotected, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }
}
